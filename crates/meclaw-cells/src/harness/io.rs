//! The I/O sub-task: one child process at a time, and an idle wait in between.
//!
//! Handler → I/O uses [`HarnessReconfig`], I/O → handler uses [`HarnessEvent`].
//! Both channels are the ones `cell_task_long_running` already creates; this
//! module only decides what travels on them.
//!
//! The shape is a cycle, not a line:
//!
//! ```text
//!   Booted → [idle] --Start--> spawn → first turn → first frame (A-timeout)
//!                ^                                          │
//!                └──── Exited ←── stream (turns) ───────────┘
//! ```
//!
//! Children run strictly one after the other. A `Start` that arrives while a
//! child still runs (the handler sent `Shutdown` first, or the child is about
//! to end) is HELD, together with every command behind it, and runs after that
//! child's `Exited`; the held commands then reach the new child in order
//! (D57 V6, OR-57-24). Nothing is dropped on the way.
//!
//! `run_io` never returns while the cell is alive (A1′). When its command
//! channel closes it parks — a clean return would let the outer `select!` win
//! on the I/O side and abort a healthy handler.

use crate::stdio_child::{
    ChildCommand, ChildEvent, ChildExit, ChildSpec, ServeConfig, StdioChild, StdioChildError,
    serve::serve_child_until_exit,
};
use serde_json::Value as JsonValue;
use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

/// Everything the I/O sub-task needs to start one task's child.
#[derive(Debug)]
pub struct StartTask {
    /// The task this child belongs to, echoed back in failure events.
    pub task_id: String,
    /// How to start the child, including workspace and containment.
    pub spec: ChildSpec,
    /// The first turn, written right after the spawn and BEFORE the startup
    /// read: the CLI says nothing until its first input (D57 S0 1), so a
    /// child without it would always run into the startup timeout. `None`
    /// writes nothing — for children that speak on their own.
    pub first_turn: Option<JsonValue>,
    /// A-timeout for the child's FIRST frame. The run itself is unbounded.
    pub startup_timeout: Duration,
    /// Write timeout and kill grace for the serve loop.
    pub serve: ServeConfig,
}

/// Handler → I/O.
#[derive(Debug)]
pub enum HarnessReconfig {
    /// Start a child. Arriving while a child runs, it is held until that
    /// child's exit and never dropped.
    Start(StartTask),
    /// A command for the running child: a follow-up turn, a control response,
    /// or the shutdown that implements cancellation.
    Child(ChildCommand),
    /// Arm the idle deadline of the running child (D57 V10). There is one
    /// deadline; a new arm replaces it, every `Send` to the child disarms it,
    /// and when it passes the handler gets one [`HarnessEvent::IdleExpired`].
    /// Only the handler arms it — it alone knows that no turn is running.
    ArmIdle(Duration),
}

impl TryFrom<HarnessReconfig> for ChildCommand {
    type Error = ();
    fn try_from(r: HarnessReconfig) -> Result<Self, ()> {
        match r {
            HarnessReconfig::Child(c) => Ok(c),
            // Not addressable to a child — the serve loop skips it rather than
            // reading it as a shutdown.
            HarnessReconfig::Start(_) | HarnessReconfig::ArmIdle(_) => Err(()),
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
    /// The idle deadline armed by [`HarnessReconfig::ArmIdle`] passed without
    /// a `Send` to the child. Sent once per arm; whether the child closes is
    /// the handler's call (D57 V10, OR-57-18).
    IdleExpired,
    /// A follow-up write to the live child failed (the kernel reports it only
    /// through the `reply` of the `Send`, D57 V10, OR-57-31 M5).
    WriteFailed {
        /// What went wrong, for the turn's outcome.
        detail: String,
    },
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

        // Commands taken off the channel but not yet delivered: a held
        // `Start` at the front, the commands behind it after.
        let mut pending: VecDeque<HarnessReconfig> = VecDeque::new();
        loop {
            let next = match pending.pop_front() {
                Some(cmd) => Some(cmd),
                None => reconfig_rx.recv().await,
            };
            match next {
                Some(HarnessReconfig::Start(start)) => {
                    pending = run_one_task(start, &events_tx, &mut reconfig_rx, pending).await;
                }
                Some(HarnessReconfig::Child(_)) => {
                    // A control response or shutdown with no child running:
                    // late, harmless, and not worth ending anything over.
                    tracing::warn!("harness: child command with no child running, dropped");
                }
                Some(HarnessReconfig::ArmIdle(_)) => {
                    // No child, nothing to idle out.
                    tracing::debug!("harness: idle arm with no child running, dropped");
                }
                // Nobody can send any more. Park (A1′).
                None => std::future::pending::<()>().await,
            }
        }
    }
}

/// Spawn one child, write its first turn, wait for it to say hello, then
/// stream it until it ends.
///
/// Returns when the child is gone — which is the ORDINARY end of a task here,
/// not a cell failure. Whether that outcome is success, crash or cancellation
/// is the handler's judgement: it has the frames and the task table. The
/// return value is every command that was taken off the channel but not
/// delivered: a `Start` held behind this child, and what came after it.
async fn run_one_task(
    start: StartTask,
    events_tx: &mpsc::Sender<HarnessEvent>,
    cmds: &mut mpsc::Receiver<HarnessReconfig>,
    pending: VecDeque<HarnessReconfig>,
) -> VecDeque<HarnessReconfig> {
    let StartTask {
        task_id,
        spec,
        first_turn,
        startup_timeout,
        serve,
    } = start;

    let mut child = match StdioChild::spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            fail(events_tx, &task_id, "spawn_failed", e.detail()).await;
            return pending;
        }
    };

    // The first turn goes out BEFORE the startup read, under the write
    // A-timeout (rule 12): the CLI is silent until its first input.
    if let Some(turn) = first_turn
        && let Err(e) = child.write_frame(&turn, serve.write_timeout).await
    {
        let exit = child.terminate(serve.kill_grace).await;
        fail(
            events_tx,
            &task_id,
            "harness_crashed",
            format!(
                "the first turn could not be written: {} ({})",
                e.detail(),
                exit.detail()
            ),
        )
        .await;
        return pending;
    }

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
            return pending;
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
            return pending;
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
            return pending;
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
                return pending;
            }
        }
    }

    // The kernel's serve loop reads an inner channel; the forwarding layer in
    // front of it decides what reaches this child. The kernel stays untouched
    // (shared with `mcp`, OR-57-18).
    let (inner_tx, mut inner_rx) = mpsc::channel::<ChildCommand>(16);
    let (gone_tx, gone_rx) = oneshot::channel::<()>();
    let serving = async move {
        // No correlation: `stream-json` carries no request ids, so every
        // frame is an unsolicited one. That is exactly the case P7 kept the
        // correlator injectable for.
        let exit = serve_child_until_exit(child, |_| None, serve, &mut inner_rx, events_tx).await;
        let _ = gone_tx.send(());
        exit
    };
    let (_exit, rest) = tokio::join!(
        serving,
        forward_commands(pending, cmds, inner_tx, gone_rx, events_tx)
    );
    rest
}

/// The forwarding layer in front of the kernel's serve loop (OR-57-18/28):
/// channels and tokio timers only, no child, so its logic is testable on its
/// own in virtual time.
///
/// It delivers `pending` first, then the outer channel, to `inner` — until a
/// `Start` arrives. That `Start` is HELD with every command behind it (taken
/// off the channel so the handler is never blocked by a closing child) and
/// comes back, in order, once `child_gone` fires (D57 V6, OR-57-24). `inner`
/// lives until then: a dropped sender is a `Shutdown` to the kernel. A closed
/// outer channel drops it on purpose — nobody will talk to this child again.
///
/// It also keeps the child's one idle deadline (D57 V10): `ArmIdle` sets or
/// replaces it, every `Send` disarms it, and its passing is reported once as
/// [`HarnessEvent::IdleExpired`]. Every `Send` without a reply channel gets
/// one here, because the kernel only logs a failed fire-and-forget write
/// (`serve.rs`); a failure comes back as [`HarnessEvent::WriteFailed`].
pub(crate) async fn forward_commands(
    mut pending: VecDeque<HarnessReconfig>,
    outer: &mut mpsc::Receiver<HarnessReconfig>,
    inner: mpsc::Sender<ChildCommand>,
    mut child_gone: oneshot::Receiver<()>,
    events: &mpsc::Sender<HarnessEvent>,
) -> VecDeque<HarnessReconfig> {
    let mut held: VecDeque<HarnessReconfig> = VecDeque::new();
    let mut inner = Some(inner);
    let mut idle: Option<Instant> = None;
    // Outcomes of the writes handed to the kernel, oldest first: the kernel
    // writes in order, so they resolve in order. A dropped sender = written.
    type Written = oneshot::Receiver<Result<JsonValue, StdioChildError>>;
    let mut writes: VecDeque<Written> = VecDeque::new();
    loop {
        let next = if held.is_empty() {
            pending.pop_front()
        } else {
            // Once a `Start` is held, everything behind it stays behind it.
            held.extend(pending.drain(..));
            None
        };
        let cmd = match next {
            Some(cmd) => cmd,
            None => tokio::select! {
                _ = &mut child_gone => return held,
                cmd = outer.recv(), if inner.is_some() => match cmd {
                    Some(cmd) => cmd,
                    None => {
                        inner = None;
                        continue;
                    }
                },
                _ = tokio::time::sleep_until(idle.unwrap_or_else(Instant::now)), if idle.is_some() => {
                    idle = None;
                    let _ = events.send(HarnessEvent::IdleExpired).await;
                    continue;
                }
                written = async {
                    match writes.front_mut() {
                        Some(rx) => rx.await,
                        None => std::future::pending().await,
                    }
                }, if !writes.is_empty() => {
                    writes.pop_front();
                    if let Ok(Err(e)) = written {
                        let _ = events
                            .send(HarnessEvent::WriteFailed { detail: e.detail() })
                            .await;
                    }
                    continue;
                }
            },
        };
        match cmd {
            HarnessReconfig::Start(_) => held.push_back(cmd),
            other if !held.is_empty() => held.push_back(other),
            HarnessReconfig::ArmIdle(after) => idle = Some(Instant::now() + after),
            HarnessReconfig::Child(c) => {
                let Some(tx) = &inner else { continue };
                let c = match c {
                    ChildCommand::Send {
                        line,
                        correlate,
                        reply: None,
                    } => {
                        // Talking to the child is the opposite of idling.
                        idle = None;
                        let (reply, written) = oneshot::channel();
                        writes.push_back(written);
                        ChildCommand::Send {
                            line,
                            correlate,
                            reply: Some(reply),
                        }
                    }
                    send @ ChildCommand::Send { .. } => {
                        idle = None;
                        send
                    }
                    other => other,
                };
                tokio::select! {
                    // The child is gone before it could take the command: a
                    // late answer or shutdown, harmless (the idle loop
                    // would drop it too).
                    _ = &mut child_gone => {
                        held.extend(pending);
                        return held;
                    }
                    _ = tx.send(c) => {}
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdio_child::ChildSpec;

    fn start(task_id: &str) -> HarnessReconfig {
        HarnessReconfig::Start(StartTask {
            task_id: task_id.to_string(),
            spec: ChildSpec::default(),
            first_turn: None,
            startup_timeout: Duration::from_secs(1),
            serve: ServeConfig {
                write_timeout: Duration::from_secs(1),
                kill_grace: Duration::from_secs(1),
            },
        })
    }

    /// D57 V6 / OR-57-24: a command before a `Start` reaches the running
    /// child; the `Start` and everything behind it are held and come back in
    /// order once the child is gone — nothing is dropped, nothing overtakes.
    #[tokio::test]
    async fn a_held_start_and_the_commands_behind_it_come_back_in_order() {
        let (outer_tx, mut outer_rx) = mpsc::channel::<HarnessReconfig>(8);
        let (inner_tx, mut inner_rx) = mpsc::channel::<ChildCommand>(8);
        let (gone_tx, gone_rx) = oneshot::channel::<()>();

        let answer = ChildCommand::Send {
            line: serde_json::json!({"type": "control_response"}),
            correlate: None,
            reply: None,
        };
        outer_tx
            .send(HarnessReconfig::Child(answer))
            .await
            .expect("send");
        outer_tx.send(start("t-2")).await.expect("send");
        outer_tx
            .send(HarnessReconfig::Child(ChildCommand::Shutdown))
            .await
            .expect("send");
        outer_tx.send(start("t-3")).await.expect("send");
        drop(outer_tx);

        let forward = tokio::spawn(async move {
            let (events, _events_rx) = mpsc::channel::<HarnessEvent>(8);
            forward_commands(VecDeque::new(), &mut outer_rx, inner_tx, gone_rx, &events).await
        });

        assert!(
            matches!(inner_rx.recv().await, Some(ChildCommand::Send { .. })),
            "the command before the Start belongs to the running child"
        );
        // The closed outer channel drops the inner sender once everything was
        // read: the Shutdown behind the held Start never reached this child.
        assert!(inner_rx.recv().await.is_none());

        gone_tx.send(()).expect("gone");
        let rest: Vec<String> = forward
            .await
            .expect("join")
            .into_iter()
            .map(|c| match c {
                HarnessReconfig::Start(s) => format!("start {}", s.task_id),
                HarnessReconfig::Child(ChildCommand::Shutdown) => "shutdown".to_string(),
                HarnessReconfig::Child(_) => "send".to_string(),
                HarnessReconfig::ArmIdle(_) => "arm".to_string(),
            })
            .collect();
        assert_eq!(rest, vec!["start t-2", "shutdown", "start t-3"]);
    }

    /// The forwarding layer of one child, driven by the test in virtual time.
    struct Layer {
        outer: mpsc::Sender<HarnessReconfig>,
        inner: mpsc::Receiver<ChildCommand>,
        events: mpsc::Receiver<HarnessEvent>,
        _gone: oneshot::Sender<()>,
    }

    fn layer() -> Layer {
        let (outer, mut outer_rx) = mpsc::channel::<HarnessReconfig>(8);
        let (inner_tx, inner) = mpsc::channel::<ChildCommand>(8);
        let (gone, gone_rx) = oneshot::channel::<()>();
        let (events_tx, events) = mpsc::channel::<HarnessEvent>(8);
        tokio::spawn(async move {
            forward_commands(
                VecDeque::new(),
                &mut outer_rx,
                inner_tx,
                gone_rx,
                &events_tx,
            )
            .await
        });
        Layer {
            outer,
            inner,
            events,
            _gone: gone,
        }
    }

    impl Layer {
        async fn send(&self, cmd: HarnessReconfig) {
            self.outer.send(cmd).await.expect("layer alive");
        }

        /// The next event; in virtual time this waits exactly until it comes.
        async fn next_event(&mut self) -> HarnessEvent {
            self.events.recv().await.expect("layer alive")
        }

        fn idle_expired(&mut self) -> usize {
            let mut n = 0;
            while let Ok(ev) = self.events.try_recv() {
                if matches!(ev, HarnessEvent::IdleExpired) {
                    n += 1;
                }
            }
            n
        }
    }

    fn turn() -> HarnessReconfig {
        HarnessReconfig::Child(ChildCommand::Send {
            line: serde_json::json!({"type": "user"}),
            correlate: None,
            reply: None,
        })
    }

    /// D57 V10 / OR-57-28: an armed deadline fires once, exactly when due.
    #[tokio::test(start_paused = true)]
    async fn the_idle_deadline_fires_exactly_once() {
        let mut l = layer();
        let t0 = Instant::now();
        l.send(HarnessReconfig::ArmIdle(Duration::from_secs(10)))
            .await;
        assert!(matches!(l.next_event().await, HarnessEvent::IdleExpired));
        assert_eq!(t0.elapsed(), Duration::from_secs(10), "exactly when due");
        // Virtual time: everything due on the way has run after.
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        assert_eq!(l.idle_expired(), 0, "and never again");
    }

    /// D57 V10: a `Send` to the child disarms the deadline — talking is not
    /// idling. The `Send` reaches the kernel with a reply channel, through
    /// which a failed write comes back as `WriteFailed` (OR-57-31 M5).
    #[tokio::test(start_paused = true)]
    async fn a_send_disarms_the_deadline() {
        let mut l = layer();
        l.send(HarnessReconfig::ArmIdle(Duration::from_secs(10)))
            .await;
        // Virtual time: everything due on the way has run after.
        tokio::time::sleep(Duration::from_secs(5)).await;
        l.send(turn()).await;
        let Some(ChildCommand::Send {
            reply: Some(reply), ..
        }) = l.inner.recv().await
        else {
            panic!("the turn reaches the child with a reply channel");
        };
        // Virtual time: everything due on the way has run after.
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        assert_eq!(l.idle_expired(), 0, "the send disarmed the deadline");

        reply
            .send(Err(StdioChildError::Timeout))
            .expect("layer waits for the write");
        assert!(matches!(
            l.next_event().await,
            HarnessEvent::WriteFailed { .. }
        ));
    }

    /// D57 V10: there is one deadline; a new arm replaces the old one.
    #[tokio::test(start_paused = true)]
    async fn a_new_arm_replaces_the_deadline() {
        let mut l = layer();
        let t0 = Instant::now();
        l.send(HarnessReconfig::ArmIdle(Duration::from_secs(10)))
            .await;
        // Virtual time: everything due on the way has run after.
        tokio::time::sleep(Duration::from_secs(5)).await;
        l.send(HarnessReconfig::ArmIdle(Duration::from_secs(10)))
            .await;
        assert!(matches!(l.next_event().await, HarnessEvent::IdleExpired));
        assert_eq!(
            t0.elapsed(),
            Duration::from_secs(15),
            "the second deadline, not the first"
        );
        // Virtual time: everything due on the way has run after.
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        assert_eq!(l.idle_expired(), 0);
    }
}
