//! Phase-10 Slice A — substrate demo for `LongRunningCell` /
//! `cell_task_long_running`. Proves dual-task dispatch and output
//! backpressure. Panic propagation is covered by `phase_10a_cell_task_lr.rs`
//! (T6/T7); RespawnFn-Round-Trip via the real `handle_cell_died` corridor
//! is covered by `phase_10_a_round_trip.rs` (T9).

use meclaw_colony::{DbConn, cell_task_long_running};
use meclaw_core::{CellEmission, MessageBuilder, Path};
use meclaw_testing::mocks::{MockEvent, ReceiptMockLongRunningCell};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10_a_demo_mailbox_message_reaches_handle() {
    let (cell, _inject) = ReceiptMockLongRunningCell::new();
    let counter = cell.handle_calls.clone();

    let (in_tx, in_rx) = mpsc::channel(8);
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

    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();
    // Wait for the dispatch BEFORE ending the I/O side. When run_io ends, the
    // outer select aborts the surviving handler task, mailbox and all.
    // Dropping `_inject` right after the send races the handler's first poll
    // against that abort, and on a loaded runner the abort can win (seen once
    // on the 2-core CI).
    //
    // The mailbox-loss half of that race is closed since GH #18: the aborted
    // handler hands its buffered remainder to the colony (`MailboxGuard`). The
    // sequencing stays anyway, and deliberately — this task is spawned WITHOUT
    // a colony inbox (`colony_inbox_tx: None`), so there is nowhere to hand the
    // message to, and the assert below pins DISPATCH ("handle() ran exactly
    // once"), which no preservation mechanism can win for it. Preservation has
    // its own pins in `gh18_mailbox_preservation.rs`.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while counter.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "handle() was never dispatched within the failure marker"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    drop(in_tx);
    drop(_inject);

    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("demo task hung")
        .unwrap();
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "handle() must be called exactly once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10_a_demo_io_event_reaches_handle_event() {
    let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
    let counter = cell.event_calls.clone();

    let (in_tx, in_rx) = mpsc::channel(8);
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

    inject_tx.send(MockEvent("tick".into())).await.unwrap();

    // Wait for handle_event to fire BEFORE closing channels (Regel A from
    // T5: no inject+close race).
    let start = std::time::Instant::now();
    loop {
        if counter.load(Ordering::SeqCst) >= 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("handle_event was not called within 30s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(inject_tx);
    drop(in_tx);

    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("demo task hung")
        .unwrap();
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "handle_event() must be called exactly once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10_a_demo_outputs_backpressure_blocks_handler_emit() {
    use meclaw_colony::LongRunningCell;
    use meclaw_core::{CellOutput, JsonValue, Message, OriginSink, OutputSink};
    use std::future::Future;

    #[derive(Clone)]
    struct EmitOnEventMock;
    struct EmitOnEventIo;

    impl LongRunningCell for EmitOnEventMock {
        type Event = ();
        type Reconfig = ();
        type Io = EmitOnEventIo;

        fn split_io(&mut self) -> Self::Io {
            EmitOnEventIo
        }

        // The explicit `+ Send` is load-bearing — AFIT does not bind `Send`,
        // and `cell_task_long_running` needs it. `clippy::manual_async_fn`
        // is a false positive here (see long_running_cell.rs § run_io).
        #[allow(clippy::manual_async_fn)]
        fn run_io(
            _io: Self::Io,
            events_tx: mpsc::Sender<()>,
            mut reconfig_rx: mpsc::Receiver<()>,
        ) -> impl Future<Output = ()> + Send {
            async move {
                // Two events: handler emits one CellEmission each. With outputs
                // capacity=1, the second emit blocks until test drains the first.
                let _ = events_tx.send(()).await;
                let _ = events_tx.send(()).await;
                // Park: keep events_tx alive (otherwise events_rx returns None
                // and handler_loop breaks via events-None-arm instead of via
                // mailbox-close — see T5.4 lesson). Returns only when
                // reconfig_tx drops (outer abort or shutdown).
                while reconfig_rx.recv().await.is_some() {}
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
            sink: &'a OriginSink,
            _d: &'a mut DbConn,
        ) -> impl Future<Output = ()> + Send + 'a {
            async move {
                sink.emit(CellOutput {
                    target: Path::new("/sink"),
                    content: JsonValue::Null,
                })
                .await
                .unwrap();
            }
        }
    }

    // outputs-Mailbox cap=1 → second emit blocks until first is drained.
    let (in_tx, in_rx) = mpsc::channel::<Message>(4);
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(1);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);
    let cell = EmitOnEventMock;

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

    let first = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("first emission must arrive")
        .expect("outputs channel closed");
    assert_eq!(first.target.as_str(), "/sink");
    assert_eq!(
        first.parent_message_id, None,
        "source emission: parent IS None"
    );

    let second = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("second emission must arrive after first is drained")
        .expect("outputs channel closed");
    assert_eq!(second.target.as_str(), "/sink");
    assert_eq!(second.parent_message_id, None);
    assert_ne!(
        first.trace_id, second.trace_id,
        "fresh trace per source emit"
    );

    // Shutdown: drop mailbox → handler exits → outer aborts run_io.
    drop(in_tx);
    drop(out_rx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("demo task hung on shutdown")
        .unwrap();
}
