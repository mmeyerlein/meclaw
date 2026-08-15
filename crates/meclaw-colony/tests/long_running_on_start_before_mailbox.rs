//! Regression lock: `LongRunningCell::on_start` completes BEFORE the handler
//! serves its first mailbox message — and does not depend on the I/O sub-task.
//!
//! The class of bug this pins: startup work that a cell must do about its own
//! previous life (crash recovery over `cell.db`) used to be driven by the first
//! event from `run_io`. The handler's `select!` orders mailbox and events not
//! at all, so under load a message won, and the recovery then ran against a
//! state the message had already changed (P8/P10 harness flake: a live task's
//! row swept into `unknown`, a false outcome emitted, the real one arriving as
//! a second outcome under the same id).
//!
//! The mock's `run_io` never sends anything — so this also pins the other half:
//! a silent I/O sub-task must not wedge the startup phase.

use meclaw_colony::{DbConn, LongRunningCell, cell_task_long_running};
use meclaw_core::{
    CellEmission, CellOutput, JsonValue, Message, MessageBuilder, OriginSink, OutputSink, Path,
    serde_json::json,
};
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// Records whether startup ran, and reports that fact when it is asked to
/// handle a message. State lives in the cell — no lock, per the concurrency
/// model.
struct StartupOrderMock {
    started: bool,
}

struct SilentIo;

impl LongRunningCell for StartupOrderMock {
    type Event = ();
    type Reconfig = ();
    type Io = SilentIo;

    fn split_io(&mut self) -> Self::Io {
        SilentIo
    }

    // The explicit `+ Send` is load-bearing — AFIT does not bind `Send`, and
    // `cell_task_long_running` needs it (see long_running_cell.rs § run_io).
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        _io: Self::Io,
        _events_tx: mpsc::Sender<()>,
        mut reconfig_rx: mpsc::Receiver<()>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            // Never speaks, never returns while the cell lives (A1'): the
            // handler must come up regardless.
            while reconfig_rx.recv().await.is_some() {}
            std::future::pending::<()>().await
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // An await inside startup, as real recovery has (a db call): the
            // guarantee is completion before service, not synchronicity.
            tokio::task::yield_now().await;
            self.started = true;
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<()>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/sink"),
                    content: json!({"startup_done": self.started}),
                })
                .await;
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: (),
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async {}
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_start_finishes_before_the_first_mailbox_message() {
    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    let db = DbConn::wrap(conn, None);

    // The message is queued BEFORE the cell task is spawned: the widest
    // possible window for a message to overtake startup.
    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .expect("queue the first message");

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        64,
        StartupOrderMock { started: false },
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    let emission = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("the handler never answered — startup wedged on a silent io task")
        .expect("outputs channel closed");
    assert_eq!(
        emission.content["startup_done"],
        JsonValue::Bool(true),
        "a message was served before on_start had finished"
    );

    join.abort();
}
