//! Phase-10a integration tests: `cell_task_long_running` substrate behavior.
//!
//! Step 4 (T4.1): mailbox → handle dispatch + clean exit.
//! Step 5 (T5.1–T5.4): event dispatch, reconfig roundtrip, graceful shutdown,
//! endless-I/O abort-on-mailbox-close.

use meclaw_colony::{DbConn, cell_task_long_running};
use meclaw_core::{CellEmission, MessageBuilder, Path};
use meclaw_testing::mocks::ReceiptMockLongRunningCell;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_dispatches_mailbox_to_handle() {
    let (cell, _inject_tx) = ReceiptMockLongRunningCell::new();
    let handle_calls = cell.handle_calls.clone();

    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        128,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();

    // Deterministic synchronization (Regel A, mirrors T5.1/T5.2): wait until
    // handle() has actually processed the mailbox message BEFORE closing the
    // channels. Dropping `in_tx`+`_inject_tx` first lets `run_io` end (events
    // source closed) → the outer `select!` may win on the io-arm and abort the
    // handler before it dispatches the queued message — a TEST-side race (the
    // io-finish-with-pending-mailbox state is production-unreachable: real
    // run_io is endless / panics, never returns spontaneously while a live
    // handler holds a queued message). Polling the atomic first closes it.
    let start = std::time::Instant::now();
    loop {
        if handle_calls.load(Ordering::SeqCst) >= 1 {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(30) {
            panic!("handle() was not called within 30s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(in_tx);
    drop(_inject_tx);

    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("did not exit in 30s")
        .unwrap();
    assert_eq!(handle_calls.load(Ordering::SeqCst), 1);
}

// ── Step 5 ────────────────────────────────────────────────────────────────────

/// T5.1: event injected via inject_tx → handle_event called before channels
/// close. Polls an atomic counter with hard timeout as failure marker (Regel A).
/// Does NOT assert "event survives mailbox-close" — Cursor-Replay covers that
/// (Regel B). Plan-T4 substrat: handler_loop breaks on mailbox-None; outer
/// aborts sibling on first finish (Regel C).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_dispatches_io_event_to_handle_event() {
    use meclaw_colony::{DbConn, cell_task_long_running};
    use meclaw_core::{CellEmission, Path};
    use meclaw_testing::mocks::{MockEvent, ReceiptMockLongRunningCell};
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
    let event_calls = cell.event_calls.clone();

    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        64,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    // 1. Inject event.
    inject_tx.send(MockEvent("fired".into())).await.unwrap();

    // 2. Wait for the event to be processed by handle_event BEFORE closing
    //    any channels. Poll the atomic with a hard timeout as failure marker.
    let start = std::time::Instant::now();
    loop {
        if event_calls.load(Ordering::SeqCst) >= 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("handle_event was not called within 30s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 3. Now close channels and wait for graceful shutdown.
    drop(in_tx);
    drop(inject_tx);

    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("cell_task_long_running did not exit within 30s")
        .unwrap();
    assert_eq!(event_calls.load(Ordering::SeqCst), 1);
}

/// T5.2: reconfig hint sent by handle() is received by run_io. Uses an
/// `AtomicUsize` counter (no async Mutex) for deterministic polling (Regel A).
/// Plan-T4 substrat: when mailbox closes, handler_loop breaks (None => break),
/// outer aborts run_io sibling (Regel C).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_routes_reconfig_from_handle_to_run_io() {
    use meclaw_colony::{DbConn, LongRunningCell, cell_task_long_running};
    use meclaw_core::{CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path};
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Clone)]
    struct ReconfigMock(Arc<AtomicUsize>);
    struct ReconfigIo(Arc<AtomicUsize>);

    impl LongRunningCell for ReconfigMock {
        type Event = ();
        type Reconfig = String;
        type Io = ReconfigIo;

        fn split_io(&mut self) -> Self::Io {
            ReconfigIo(self.0.clone())
        }

        // Explizites `+ Send` ist load-bearing — AFIT bindet kein `Send`,
        // `cell_task_long_running` braucht es. `clippy::manual_async_fn`
        // ist hier False-Positive (siehe long_running_cell.rs § run_io).
        #[allow(clippy::manual_async_fn)]
        fn run_io(
            io: Self::Io,
            events_tx: mpsc::Sender<()>,
            mut reconfig_rx: mpsc::Receiver<String>,
        ) -> impl Future<Output = ()> + Send {
            async move {
                // Keep events_tx alive inside the future so that handler_loop's
                // events_rx.recv() does not return None prematurely (which would
                // trigger None => break and abort this task before reconfig is
                // received). Dropped at end of scope when run_io is aborted by
                // the outer (Plan-T4: outer aborts sibling on handler-exit).
                let _events_tx = events_tx;
                while reconfig_rx.recv().await.is_some() {
                    // Atomic increment — no async yield, no Mutex contention
                    // under concurrent test load.
                    io.0.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: Message,
            _sink: &'a OutputSink,
            _db: &'a mut DbConn,
            reconfig_tx: &'a mpsc::Sender<String>,
        ) -> impl Future<Output = ()> + Send + 'a {
            async move {
                let _ = reconfig_tx.send("schedule_changed".into()).await;
            }
        }

        #[allow(clippy::manual_async_fn)]
        fn handle_event<'a>(
            &'a mut self,
            _e: (),
            _s: &'a OriginSink,
            _d: &'a mut DbConn,
        ) -> impl Future<Output = ()> + Send + 'a {
            async {}
        }
    }

    let reconfig_count = Arc::new(AtomicUsize::new(0));
    let cell = ReconfigMock(reconfig_count.clone());

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    // 1. Send message → handle() sends reconfig hint.
    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();

    // 2. Wait until run_io observed the reconfig BEFORE closing the mailbox.
    //    Poll the atomic with a hard timeout as failure marker (Regel A).
    let start = std::time::Instant::now();
    loop {
        if reconfig_count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("run_io did not observe reconfig within 30s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 3. Now close mailbox; outer aborts run_io (per Plan-T4 substrat).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("did not exit in 30s")
        .unwrap();

    assert_eq!(reconfig_count.load(Ordering::SeqCst), 1);
}

/// T5.3: graceful shutdown via both channels closing — mailbox closed +
/// inject_tx closed. handler_loop breaks on mailbox-None (Plan-T4); outer
/// aborts run_io sibling. No hang expected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_exits_gracefully_when_both_channels_close() {
    use meclaw_colony::{DbConn, cell_task_long_running};
    use meclaw_core::{CellEmission, Path};
    use meclaw_testing::mocks::ReceiptMockLongRunningCell;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    drop(in_tx);
    drop(inject_tx);

    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("cell_task_long_running hung after both channels closed")
        .unwrap();
}

/// T6: panic in run_io must propagate through cell_task_long_running's outer
/// task as JoinError::is_panic() == true (so the supervisor's
/// handle_cell_died sees was_panic=true and one_for_one triggers).
/// Without `std::panic::resume_unwind` in cell_task_long_running, the outer
/// would terminate cleanly and one_for_one would NOT restart the cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_propagates_io_task_panic_to_outer_join() {
    use meclaw_colony::{DbConn, cell_task_long_running};
    use meclaw_core::{CellEmission, Path};
    use meclaw_testing::mocks::ReceiptMockLongRunningCell;
    use tokio::sync::mpsc;

    let (cell, _inject_tx) = ReceiptMockLongRunningCell::new();
    let cell = cell.with_panic_in_run_io();

    let (_in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    // Deterministic rendezvous: outer task terminates via resume_unwind when
    // run_io panics; join.await returns JoinError. No timing backstop —
    // Twin of T7 fix (8a2b136) for the same 5s-Backstop-Flake pattern.
    let join_err = join
        .await
        .expect_err("outer must return JoinError when sub-task panics");
    assert!(
        join_err.is_panic(),
        "supervisor must see was_panic=true (one_for_one trigger); got {join_err:?}"
    );
}

/// T7: panic in handle() (handler sub-task) must propagate through
/// cell_task_long_running's outer task as JoinError::is_panic() == true.
/// Symmetric to T6 (io-panic) — both select! arms have independent panic
/// propagation paths.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_propagates_handler_task_panic_to_outer_join() {
    use meclaw_colony::{DbConn, cell_task_long_running};
    use meclaw_core::{CellEmission, MessageBuilder, Path};
    use meclaw_testing::mocks::ReceiptMockLongRunningCell;
    use tokio::sync::mpsc;

    let (cell, _inject_tx) = ReceiptMockLongRunningCell::new();
    let cell = cell.with_panic_in_handle_after(1);

    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    // Send one message → handle() increments counter to 1 → panic_in_handle_after(1)
    // triggers panic.
    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();

    let join_err = join
        .await
        .expect_err("outer must return JoinError when handler panics");
    assert!(
        join_err.is_panic(),
        "supervisor must see was_panic=true; got {join_err:?}"
    );
}

/// T5.4: abort-on-mailbox-close-Pfad — the I/O task runs endlessly (as real
/// proxy/timer/mcp cells do); only the mailbox is closed. The outer MUST abort
/// the endless I/O task, otherwise the Cell-Despawn hangs forever. With the
/// reverted Plan-T4 form (always abort sibling) this terminates in
/// milliseconds; with bd4683a's natural-end-wait logic it would timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_task_long_running_aborts_endless_io_on_mailbox_close() {
    use meclaw_colony::{DbConn, LongRunningCell, cell_task_long_running};
    use meclaw_core::{CellEmission, Message, OriginSink, OutputSink, Path};
    use std::future::Future;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// I/O task runs endlessly (like real Long-Running-Cells): only returns
    /// via Outer-abort, never by itself.
    #[derive(Clone)]
    struct EndlessIoMock;
    struct EndlessIo;

    impl LongRunningCell for EndlessIoMock {
        type Event = ();
        type Reconfig = ();
        type Io = EndlessIo;

        fn split_io(&mut self) -> Self::Io {
            EndlessIo
        }

        // Explizites `+ Send` ist load-bearing — AFIT bindet kein `Send`,
        // `cell_task_long_running` braucht es. `clippy::manual_async_fn`
        // ist hier False-Positive (siehe long_running_cell.rs § run_io).
        #[allow(clippy::manual_async_fn)]
        fn run_io(
            _io: Self::Io,
            events_tx: mpsc::Sender<()>,
            reconfig_rx: mpsc::Receiver<()>,
        ) -> impl Future<Output = ()> + Send {
            // Endless — like real Long-Running-Cells (long-poll, sleep_until
            // loop, SSE stream read). Explicitly bind both channels into the
            // async-move scope so they stay alive for the whole future
            // lifetime: without `let _ = …;` the unused fn-args would be
            // dropped at future-construction time, closing events_tx
            // immediately → handler_loop's events_rx returns None → break,
            // and the test would terminate via the events-arm instead of via
            // the mailbox-close-abort path it claims to verify (False-Green).
            async move {
                let _events_tx = events_tx;
                let _reconfig_rx = reconfig_rx;
                std::future::pending::<()>().await
            }
        }

        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: Message,
            _sink: &'a OutputSink,
            _db: &'a mut DbConn,
            _reconfig_tx: &'a mpsc::Sender<()>,
        ) -> impl Future<Output = ()> + Send + 'a {
            async {}
        }

        #[allow(clippy::manual_async_fn)]
        fn handle_event<'a>(
            &'a mut self,
            _e: (),
            _s: &'a OriginSink,
            _d: &'a mut DbConn,
        ) -> impl Future<Output = ()> + Send + 'a {
            async {}
        }
    }

    let cell = EndlessIoMock;
    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None,
        None, // death_ack
        None,
    ));

    // Only close the mailbox — inject/I/O are endless.
    drop(in_tx);

    // Outer MUST abort the endless I/O task, otherwise the Despawn hangs.
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect(
            "Outer hung — endless I/O sub-task was NOT aborted on mailbox-close \
             (Plan-T4-Substrat-Bruch)",
        )
        .unwrap();
}
