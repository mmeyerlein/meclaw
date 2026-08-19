//! GH #18 — messages buffered in a cell's mailbox must survive the cell's death.
//!
//! Three pins, one per link of the chain:
//!
//! 1. a stateful cell panics with messages still queued → the dying task hands
//!    them to the colony instead of dropping them on the unwind;
//! 2. the live evidence from the hardening wave — a long-running cell whose I/O
//!    sub-task ends first gets its handler aborted, mailbox and all, and the
//!    buffered message must survive that abort;
//! 3. end to end through the real `handle_cell_died` corridor: a cell that
//!    panics with N messages waiting processes all N after the respawn.
//!
//! The corridor itself (`route()`, `handle_cell_died`) is byte-frozen and is
//! not touched by any of this — the rescue travels on the colony inbox and is
//! delivered at the `CellDied` call site, after the corridor returned.

use meclaw_colony::{
    CellFactory, ColonyMsg, ContractView, DbConn, SpawnedCellKind, cell_task_long_running,
    cell_task_stateful,
};
use meclaw_core::{CellEmission, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::mocks::{PersistMockCell, ReceiptMockLongRunningCell};
use meclaw_testing::wait::{wait_for_cell_db_value, wait_for_spawn_count};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// Await the next colony message and return it, failing loudly on silence.
async fn next_colony_msg(rx: &mut mpsc::Receiver<ColonyMsg>) -> ColonyMsg {
    tokio::time::timeout(MARKER, rx.recv())
        .await
        .expect("no colony message within the failure marker")
        .expect("colony inbox closed")
}

/// A panicking stateful cell must not take its unread mailbox with it.
///
/// Two messages are buffered BEFORE the task is spawned, so the queue state at
/// the moment of the panic is a fact of the setup, not a race: message one
/// drives the cell into its panic, messages two and three were never polled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_stateful_cell_hands_its_buffered_messages_to_the_colony() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let db = DbConn::wrap(conn, None);
    let cell = PersistMockCell::from_params(&json!({"panic_after": 1, "terminal": true})).unwrap();

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (colony_tx, mut colony_rx) = mpsc::channel::<ColonyMsg>(8);

    let doomed = MessageBuilder::new(Path::new("/s")).build();
    let queued_a = MessageBuilder::new(Path::new("/s")).build();
    let queued_b = MessageBuilder::new(Path::new("/s")).build();
    let (id_a, id_b) = (queued_a.id, queued_b.id);
    in_tx.send(doomed).await.unwrap();
    in_tx.send(queued_a).await.unwrap();
    in_tx.send(queued_b).await.unwrap();

    let join = tokio::spawn(cell_task_stateful(
        Path::new("/s"),
        in_rx,
        out_tx,
        cell,
        db,
        None, // idle_timeout
        None, // message_timeout
        None, // peace_tx
        None, // backstop_tx
        Some(colony_tx),
        0,    // cell_timeout
        None, // stop_rx
        None, // death_ack
        None, // blob_store
        None, // consumes
        Default::default(),
    ));

    let outcome = tokio::time::timeout(MARKER, join)
        .await
        .expect("cell task hung");
    assert!(
        outcome.expect_err("the cell must panic").is_panic(),
        "the doomed message must panic the cell task"
    );

    match next_colony_msg(&mut colony_rx).await {
        ColonyMsg::MailboxRescued { path, messages } => {
            assert_eq!(path, Path::new("/s"));
            let ids: Vec<_> = messages.iter().map(|m| m.id).collect();
            assert_eq!(
                ids,
                vec![id_a, id_b],
                "both unread messages must be rescued, oldest first"
            );
        }
        _ => panic!("expected a mailbox rescue, got a different colony message"),
    }
}

/// The live evidence from the hardening wave: when `run_io` ends, the outer
/// `select!` aborts the surviving handler task — mailbox and all.
///
/// Sequencing makes the abort the deterministic winner: the handler is parked
/// inside a long `handle()` when the I/O side ends, so the second message
/// provably never reached a poll.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_io_end_abort_does_not_take_the_buffered_message_with_it() {
    let (mut cell, inject) = ReceiptMockLongRunningCell::new();
    // Long enough that the abort provably lands while `handle()` is still in
    // flight; the test never waits it out (the abort ends the task).
    cell.sleep_in_handle_ms = 30_000;
    let handle_calls = cell.handle_calls.clone();

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (colony_tx, mut colony_rx) = mpsc::channel::<ColonyMsg>(8);
    let db = DbConn::wrap(rusqlite::Connection::open_in_memory().unwrap(), None);

    let in_flight = MessageBuilder::new(Path::new("/lr")).build();
    let queued = MessageBuilder::new(Path::new("/lr")).build();
    let queued_id = queued.id;
    in_tx.send(in_flight).await.unwrap();
    in_tx.send(queued).await.unwrap();

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        64,
        cell,
        db,
        None, // peace_tx
        Some(colony_tx),
        None, // stop_rx
        None, // death_ack
        None, // blob_store
        None, // consumes
        Default::default(),
    ));

    // Wait until the handler is inside `handle()` — only then is the second
    // message provably still buffered.
    let deadline = std::time::Instant::now() + MARKER;
    while handle_calls.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "handle() was never dispatched within the failure marker"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // End the I/O side → the outer select aborts the handler mid-`handle()`.
    drop(inject);
    drop(in_tx);

    match next_colony_msg(&mut colony_rx).await {
        ColonyMsg::MailboxRescued { path, messages } => {
            assert_eq!(path, Path::new("/lr"));
            let ids: Vec<_> = messages.iter().map(|m| m.id).collect();
            assert_eq!(ids, vec![queued_id], "the buffered message must survive");
        }
        _ => panic!("expected a mailbox rescue, got a different colony message"),
    }

    tokio::time::timeout(MARKER, join)
        .await
        .expect("outer task hung")
        .expect("io-end abort is not a panic");
}

/// The issue's done-when, end to end: a cell panics with N messages waiting and
/// the successor processes all N.
///
/// Determinism comes from the lazy (Dormant) spawn path: the factory hands back
/// a parked mailbox pair with NO task behind it, so the three messages pushed
/// into it are buffered by construction. The fourth message wakes the cell; the
/// first one it polls drives it into the panic, and the other three are the
/// queue that must survive. `counter` in `cell.db` is the positive receipt —
/// it reaches 4 only if every single message was handled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_panics_with_queued_messages_processes_them_all_after_the_respawn() {
    let h = ColonyHandle::new();
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("s");
    std::fs::create_dir(&cell_dir).unwrap();

    let factory = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let spawn_count = factory.spawn_count.clone();
    let spawned = factory
        .clone()
        .spawn_cell(
            Path::new("/s"),
            json!({"panic_after": 1, "terminal": true}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();

    // Pre-fill the parked mailbox — the lazy factory has not spawned a task, so
    // nothing can consume these before the wake.
    let SpawnedCellKind::Dormant { ref sender, .. } = spawned else {
        panic!("the lazy stateful factory must park the cell");
    };
    let prefill = sender.clone();
    for _ in 0..3 {
        prefill
            .send(MessageBuilder::new(Path::new("/s")).build())
            .await
            .unwrap();
    }
    drop(prefill);

    h.register_spawned(Path::new("/s"), spawned).await;

    // Wake-on-message: this fourth message spawns the cell task, which then
    // polls message one and panics with three still queued.
    h.send(MessageBuilder::new(Path::new("/s")).build()).await;

    // Restart barrier: build #1 = wake, build #2 = respawn through the corridor.
    wait_for_spawn_count(&spawn_count, 2, MARKER).await;

    // Positive receipt: the counter only reaches 4 if the three queued messages
    // reached the successor.
    wait_for_cell_db_value(&cell_dir, "counter", "4", MARKER).await;
}
