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
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::mocks::{PersistMockCell, ReceiptMockLongRunningCell};
use meclaw_testing::wait::wait_for_spawn_count;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// Await the next colony message and return it, failing loudly on silence.
///
/// GH #47: a cell now returns a `WorkDone` ticket for every delivery it took,
/// so the mailbox-rescue traffic this file is about is no longer the first
/// thing in the inbox. Tickets are bookkeeping, not lifecycle — skip them.
async fn next_colony_msg(rx: &mut mpsc::Receiver<ColonyMsg>) -> ColonyMsg {
    loop {
        let msg = tokio::time::timeout(MARKER, rx.recv())
            .await
            .expect("no colony message within the failure marker")
            .expect("colony inbox closed");
        if !matches!(msg, ColonyMsg::WorkDone { .. }) {
            return msg;
        }
    }
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
    panics_with_queued_messages_and_counts_all(false).await;
}

/// GH #1068: the message that WAKES the cell can reach its mailbox only after
/// the cell already died of the first one. The colony wakes the cell and sends
/// right after; under host load the colony's thread lost the CPU in between,
/// the cell polled message one, panicked and closed its mailbox, and the send
/// met a closed mailbox — the message was gone without a dead letter (2/20 on
/// a build host carrying other runs). Here the wake itself waits until the cell
/// is dead, so the send ALWAYS meets the closed mailbox; the successor must
/// still handle all four.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_sent_while_the_cell_dies_reaches_the_successor() {
    panics_with_queued_messages_and_counts_all(true).await;
}

/// The shared body. `wake_returns_after_death` wraps the factory's WakeFn so
/// it returns only once the woken task has closed its mailbox (bounded wait).
async fn panics_with_queued_messages_and_counts_all(wake_returns_after_death: bool) {
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
    let mut ids = Vec::new();
    for i in 0..3 {
        let m = numbered("/s", i);
        ids.push(m.id);
        prefill.send(m).await.unwrap();
    }
    drop(prefill);

    let spawned = if wake_returns_after_death {
        hold_wake_until_the_mailbox_closes(spawned)
    } else {
        spawned
    };
    h.register_spawned(Path::new("/s"), spawned).await;

    // Wake-on-message: this fourth message spawns the cell task, which then
    // polls message one and panics with three still queued.
    let wake = numbered("/s", 3);
    ids.push(wake.id);
    let t0 = std::time::Instant::now();
    h.send(wake).await;

    // Restart barrier: build #1 = wake, build #2 = respawn through the corridor.
    wait_for_spawn_count(&spawn_count, 2, MARKER).await;
    let respawned_after = t0.elapsed();

    // Positive receipt: the counter only reaches 4 if the three queued messages
    // reached the successor.
    let deadline = std::time::Instant::now() + MARKER;
    let mut last = String::from("<none>");
    loop {
        last = cell_db_slot(&cell_dir, "counter").unwrap_or(last);
        if last == "4" {
            // Review F3: the count alone does not say WHERE the late message
            // went. Rescued mail was queued before it, so it is handled last:
            // the successor's last input is the wake (#3), behind #1 and #2.
            assert_eq!(
                last_input_index(&cell_dir),
                Some(3),
                "GH #1068: the message that met the closed mailbox must reach the successor \
                 BEHIND the rescued mailbox (last input {:?})",
                cell_db_slot_last_input(&cell_dir),
            );
            return;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // GH #1068: one red in a full-suite gate under host load (last_seen=3 after
    // 30 s), 0/20 alone. A bare timeout cannot say WHICH message went where, so
    // the red run names it: every dead letter (ours by index 0-2 = queued, 3 =
    // the wake), the cell's lifecycle in the registry, the build count and the
    // time the respawn took. The assertion is the old one.
    let dead: Vec<String> = h
        .drain_dead_letters()
        .await
        .iter()
        .map(|d| {
            let which = ids
                .iter()
                .position(|id| *id == d.message.id)
                .map_or("foreign".to_string(), |i| format!("msg {i}"));
            format!(
                "{which} -> {:?} at {}",
                d.reason,
                d.resolved_target.as_str()
            )
        })
        .collect();
    let entry = registry_entry(&h, "/s").await;
    panic!(
        "GH #18: the counter stopped at {last}, expected 4 within {MARKER:?} after the respawn \
         (respawn seen {respawned_after:?} after the wake; builds {}; registry /s {entry}; \
         last_input {:?}; dead letters {dead:?})",
        spawn_count.load(Ordering::SeqCst),
        cell_db_slot_last_input(&cell_dir),
    );
}

/// GH #1068 review F2/F3: the death leaves no successor (restart limit 0):
/// the rescued mailbox and the message that met the closed mailbox are
/// dead-lettered as `cell_inactive`, rescue first, the late message behind it.
/// The late message was routed, so the log holds it as an open delivery; a
/// dead letter is an end, never a replay -- the next life of the same
/// colony.db replays nothing (a fresh cell at the path handles a probe as its
/// FIRST message).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_death_without_a_successor_dead_letters_rescue_and_late_mail_in_order_and_never_replays_them()
 {
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    let cell_dir = td.path().join("s");
    std::fs::create_dir(&cell_dir).unwrap();
    let factory = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
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
    let SpawnedCellKind::Dormant { ref sender, .. } = spawned else {
        panic!("the lazy stateful factory must park the cell");
    };
    let mut ids = Vec::new();
    for i in 0..3 {
        let m = numbered("/s", i);
        ids.push(m.id);
        sender.send(m).await.unwrap();
    }
    register_dormant(
        &h,
        "/s",
        hold_wake_until_the_mailbox_closes(spawned),
        Some(0),
    )
    .await;
    let late = numbered("/s", 3);
    let late_id = late.id;
    ids.push(late_id);
    h.send(late).await;

    // #0 panicked the cell; #1, #2 were rescued, #3 met the closed mailbox.
    let mut dead = Vec::new();
    let deadline = std::time::Instant::now() + MARKER;
    while dead.len() < 3 && std::time::Instant::now() < deadline {
        dead.extend(h.drain_dead_letters().await);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let got: Vec<_> = dead.iter().map(|d| d.message.id).collect();
    assert_eq!(
        got,
        ids[1..].to_vec(),
        "rescue (#1, #2) first, the late message (#3) behind it"
    );
    assert!(
        dead.iter()
            .all(|d| d.reason == meclaw_colony::DeadLetterReason::CellInactive),
        "no successor: cell_inactive ({:?})",
        dead.iter().map(|d| &d.reason).collect::<Vec<_>>()
    );
    h.shutdown().await;
    assert_eq!(
        open_deliveries(td.path(), late_id),
        0,
        "GH #1068 F2: the dead-lettered late message's delivery must be closed, or the next \
         boot replays a dead letter"
    );

    // Second life of the same colony.db: a fresh cell at /s, then the replay
    // point (InitialApply), then a probe. Anything replayed would be handled
    // before the probe.
    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    let fresh_dir = td.path().join("s2");
    std::fs::create_dir(&fresh_dir).unwrap();
    let spawned = factory
        .clone()
        .spawn_cell(
            Path::new("/s"),
            json!({"terminal": true}),
            h.runtime().outputs_tx,
            fresh_dir.clone(),
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    register_dormant(&h, "/s", spawned, None).await;
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::InitialApply {
            edges: Vec::new(),
            hive_scopes: Vec::new(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.expect("initial apply ack");
    h.send(numbered("/s", 9)).await;
    let deadline = std::time::Instant::now() + MARKER;
    while last_input_index(&fresh_dir) != Some(9) {
        assert!(
            std::time::Instant::now() < deadline,
            "the probe never arrived (last input {:?})",
            cell_db_slot_last_input(&fresh_dir)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        cell_db_slot(&fresh_dir, "counter").as_deref(),
        Some("1"),
        "GH #1068 F2: the probe must be the first delivery of the new life -- a dead letter \
         is never replayed"
    );
    h.shutdown().await;
}

/// GH #1068 review F3: a message that met a dying cell's closed mailbox and
/// whose death never reached the colony before the shutdown is dead-lettered
/// by the shutdown flush (`cell_inactive`) -- and its open delivery is closed,
/// so the next boot does not replay it (F2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_message_whose_death_never_reports_is_dead_lettered_at_shutdown_and_closed() {
    let td = tempfile::TempDir::new().unwrap();
    // Held mail keeps the drain from settling: keep its deadline short.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"shutdown_drain_timeout_ms": 200}"#,
    )
    .unwrap();
    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    let (sender, receiver) = mpsc::channel::<Message>(8);
    // The wake "spawns" a task that dies at once -- its mailbox closes -- and
    // whose death is never reported: no watcher, no `CellDied`.
    let wake: meclaw_colony::WakeFn = Box::new(|mailbox| {
        drop(mailbox);
        let (stop_tx, _) = tokio::sync::oneshot::channel();
        let (_, death_ack_rx) = tokio::sync::oneshot::channel();
        (stop_tx, death_ack_rx)
    });
    let respawn: meclaw_colony::RespawnFn = Box::new(|| unreachable!("no death is reported"));
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RegisterDormant {
            path: Path::new("/d"),
            sender,
            receiver,
            wake: Some(wake),
            respawn,
            restart_limit: None,
            cell_id: meclaw_core::Uuid::now_v7(),
            cell_type: "test-mock".into(),
            active: true,
            failed: false,
            dormant: false,
            eager_on_reconnect: false,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
    let late = numbered("/d", 0);
    let late_id = late.id;
    h.send(late).await;
    h.shutdown().await;

    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let dead: Vec<String> = conn
        .prepare("SELECT error_code FROM dead_letters WHERE message_json LIKE ?1")
        .unwrap()
        .query_map([format!("%{late_id}%")], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        dead,
        vec!["cell_inactive".to_string()],
        "the shutdown flush dead-letters the late message"
    );
    assert_eq!(
        open_deliveries(td.path(), late_id),
        0,
        "GH #1068 F2: and closes its open delivery -- no replay of a dead letter"
    );
}

/// A message to `target` whose body carries its index `i`.
fn numbered(target: &str, i: u64) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(json!({ "messages": [], "i": i })))
        .build()
}

/// The index of the last message a `PersistMockCell` handled, from its
/// `last_input` row.
fn last_input_index(cell_dir: &std::path::Path) -> Option<u64> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let body: String = conn
        .query_row("SELECT message_json FROM last_input", [], |r| r.get(0))
        .ok()?;
    meclaw_core::serde_json::from_str::<meclaw_core::serde_json::Value>(&body)
        .ok()?
        .get("i")?
        .as_u64()
}

/// Open `delivery_open` rows of `id` in the colony.db under `dir`.
fn open_deliveries(dir: &std::path::Path, id: meclaw_core::Uuid) -> i64 {
    rusqlite::Connection::open(dir.join("colony.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM delivery_open WHERE message_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
}

/// Register a Dormant cell with an explicit restart limit.
async fn register_dormant(
    h: &ColonyHandle,
    path: &str,
    spawned: SpawnedCellKind,
    restart_limit: Option<u32>,
) {
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        respawn,
        ..
    } = spawned
    else {
        panic!("the lazy stateful factory must park the cell");
    };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RegisterDormant {
            path: Path::new(path),
            sender,
            receiver,
            wake: Some(wake),
            respawn,
            restart_limit,
            cell_id: meclaw_core::Uuid::now_v7(),
            cell_type: "test-mock".into(),
            active: true,
            failed: false,
            dormant: false,
            eager_on_reconnect: false,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// Wrap a Dormant cell's WakeFn: wake as before, then block (the WakeFn runs
/// synchronously on the colony's thread, as a descheduled thread would) until
/// the woken task has closed its mailbox — the panic's `MailboxGuard` does that.
fn hold_wake_until_the_mailbox_closes(spawned: SpawnedCellKind) -> SpawnedCellKind {
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        stop_tx,
        death_ack_rx,
        respawn,
    } = spawned
    else {
        panic!("the lazy stateful factory must park the cell");
    };
    let watch = sender.clone();
    let wake: meclaw_colony::WakeFn = Box::new(move |recv| {
        let wired = wake(recv);
        let deadline = std::time::Instant::now() + MARKER;
        while !watch.is_closed() {
            assert!(
                std::time::Instant::now() < deadline,
                "the woken cell never closed its mailbox (no panic?)"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        wired
    });
    SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        stop_tx,
        death_ack_rx,
        respawn,
    }
}

/// One `system` slot of a `cell.db`, read-only from outside; `None` while the
/// file or its table does not exist yet (the cell opens it at the wake).
fn cell_db_slot(cell_dir: &std::path::Path, slot: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM system WHERE slot_path = ?",
        [slot],
        |r| r.get(0),
    )
    .ok()
}

/// The `last_input` row of a `cell.db` (diagnosis only).
fn cell_db_slot_last_input(cell_dir: &std::path::Path) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT message_json || ' @' || received_at FROM last_input",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// The registry's view of `path` as one diagnosis string.
async fn registry_entry(h: &ColonyHandle, path: &str) -> String {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    match ack_rx.await {
        Ok(r) => {
            r.entries
                .into_iter()
                .find(|e| e.path == path)
                .map_or("<absent>".to_string(), |e| {
                    format!(
                        "lifecycle {} active {} failed {}",
                        e.lifecycle_status, e.active, e.failed
                    )
                })
        }
        Err(_) => "<no answer>".to_string(),
    }
}
