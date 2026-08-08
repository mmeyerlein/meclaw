//! The I/O sub-task: one child process per task, and an idle wait in between.
//!
//! Handler → I/O uses [`HarnessReconfig`], I/O → handler uses [`HarnessEvent`].
//! Both channels are the ones `cell_task_long_running` already creates; this
//! module only decides what travels on them.
//!
//! The shape is a cycle, not a line:
//!
//! ```text
//!   Booted → [idle] --Start--> spawn → first frame (A-timeout)
//!                ^                          │
//!                └──── Exited ←── stream ───┘
//! ```
//!
//! `run_io` never returns while the cell is alive (A1′). When its command
//! channel closes it parks — a clean return would let the outer `select!` win
//! on the I/O side and abort a healthy handler.

use crate::stdio_child::{
    ChildCommand, ChildEvent, ChildExit, ChildSpec, ServeConfig, StdioChild,
    serve::serve_child_until_exit,
};
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// Everything the I/O sub-task needs to start one task's child.
#[derive(Debug)]
pub struct StartTask {
    /// The task this child belongs to, echoed back in failure events.
    pub task_id: String,
    /// How to start the child, including workspace and containment.
    pub spec: ChildSpec,
    /// A-timeout for the child's FIRST frame. The run itself is unbounded.
    pub startup_timeout: Duration,
    /// Write timeout and kill grace for the serve loop.
    pub serve: ServeConfig,
}

/// Handler → I/O.
#[derive(Debug)]
pub enum HarnessReconfig {
    /// Start a task. Only meaningful while no child is running; the handler
    /// enforces that, and the serve loop skips it if it ever slips through.
    Start(StartTask),
    /// A command for the running child: a control response, or the shutdown
    /// that implements cancellation.
    Child(ChildCommand),
}

impl TryFrom<HarnessReconfig> for ChildCommand {
    type Error = ();
    fn try_from(r: HarnessReconfig) -> Result<Self, ()> {
        match r {
            HarnessReconfig::Child(c) => Ok(c),
            // Not addressable to a child — the serve loop skips it rather than
            // reading it as a shutdown.
            HarnessReconfig::Start(_) => Err(()),
        }
    }
}

/// I/O → handler.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    /// The I/O sub-task is up. The handler answers this with restart recovery:
    /// any task still marked running belongs to a previous life of this cell.
    Booted,
    /// A task could not be started, or died before it said anything.
    TaskFailed {
        /// Which task.
        task_id: String,
        /// One of the cell's closed error-code set.
        error_code: &'static str,
        /// What went wrong, for the emission.
        detail: String,
    },
    /// Something happened on the running child's connection.
    Child(ChildEvent),
}

impl From<ChildEvent> for HarnessEvent {
    fn from(e: ChildEvent) -> Self {
        HarnessEvent::Child(e)
    }
}

/// The I/O sub-task's birth state — deliberately empty.
///
/// Unlike `mcp`, this cell has nothing to connect to at startup: it holds no
/// client, no endpoint and no child until a task arrives. Everything a run
/// needs travels with the `Start` command, which also keeps runtime-tunable
/// timeouts correct without any live re-reading.
#[derive(Debug)]
pub struct HarnessIo;

/// I/O sub-task. Never returns voluntarily (A1′).
///
/// The `impl Future + Send` return type is required because the generic
/// `tokio::spawn` in `cell_task_long_running` needs `Send`.
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    _io: HarnessIo,
    events_tx: mpsc::Sender<HarnessEvent>,
    reconfig_rx: mpsc::Receiver<HarnessReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        // Phase-10-A lesson: bind both channels explicitly into the scope so
        // neither is dropped early.
        let events_tx = events_tx;
        let mut reconfig_rx = reconfig_rx;

        if events_tx.send(HarnessEvent::Booted).await.is_err() {
            // The handler is already gone; there is nothing to serve, but
            // returning would still be an A1′ violation.
            std::future::pending::<()>().await;
        }

        loop {
            match reconfig_rx.recv().await {
                Some(HarnessReconfig::Start(start)) => {
                    run_one_task(start, &events_tx, &mut reconfig_rx).await;
                }
                Some(HarnessReconfig::Child(_)) => {
                    // A control response or shutdown with no task running:
                    // late, harmless, and not worth ending anything over.
                    tracing::warn!("harness: child command with no task running, dropped");
                }
                // Nobody can send any more. Park (A1′).
                None => std::future::pending::<()>().await,
            }
        }
    }
}

/// Spawn one child, wait for it to say hello, then stream it until it ends.
///
/// Returns when the child is gone — which is the ORDINARY end of a task here,
/// not a cell failure. Whether that outcome is success, crash or cancellation
/// is the handler's judgement: it has the frames and the task table.
async fn run_one_task(
    start: StartTask,
    events_tx: &mpsc::Sender<HarnessEvent>,
    cmds: &mut mpsc::Receiver<HarnessReconfig>,
) {
    let StartTask {
        task_id,
        spec,
        startup_timeout,
        serve,
    } = start;

    let mut child = match StdioChild::spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            fail(events_tx, &task_id, "spawn_failed", e.detail()).await;
            return;
        }
    };

    // A-timeout (rule 12) on the ONE bounded operation of a task: saying
    // hello. Everything after this is the harness working, and working
    // legitimately takes minutes.
    match tokio::time::timeout(startup_timeout, child.read_frame()).await {
        Err(_) => {
            let exit = child.terminate(serve.kill_grace).await;
            fail(
                events_tx,
                &task_id,
                "startup_timeout",
                format!("no output within the startup budget ({})", exit.detail()),
            )
            .await;
            return;
        }
        Ok(Err(e)) => {
            let exit = child.terminate(serve.kill_grace).await;
            fail(
                events_tx,
                &task_id,
                "harness_crashed",
                format!("{} ({})", e.detail(), exit.detail()),
            )
            .await;
            return;
        }
        Ok(Ok(None)) => {
            let exit = child.terminate(serve.kill_grace).await;
            fail(
                events_tx,
                &task_id,
                "harness_crashed",
                format!("ended before saying anything ({})", exit.detail()),
            )
            .await;
            return;
        }
        Ok(Ok(Some(first))) => {
            // The first frame was consumed to bound the startup; hand it on
            // exactly as the serve loop would have.
            let event = match first {
                crate::stdio_child::Frame::Json(v) => ChildEvent::Frame(v),
                crate::stdio_child::Frame::Malformed(raw) => ChildEvent::Malformed { raw },
            };
            if events_tx.send(HarnessEvent::Child(event)).await.is_err() {
                let _: ChildExit = child.terminate(serve.kill_grace).await;
                return;
            }
        }
    }

    // No correlation: `stream-json` carries no request ids, so every frame is
    // an unsolicited one. That is exactly the case P7 kept the correlator
    // injectable for.
    serve_child_until_exit(child, |_| None, serve, cmds, events_tx).await;
}

/// Report a task that never got going. Uses the same channel as every other
/// event so ordering is preserved.
async fn fail(
    events_tx: &mpsc::Sender<HarnessEvent>,
    task_id: &str,
    error_code: &'static str,
    detail: String,
) {
    let _ = events_tx
        .send(HarnessEvent::TaskFailed {
            task_id: task_id.to_string(),
            error_code,
            detail,
        })
        .await;
}
