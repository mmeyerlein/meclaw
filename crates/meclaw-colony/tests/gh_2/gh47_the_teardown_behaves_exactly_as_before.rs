//! GH #47: the shutdown teardown is about to grow a phase in front of it. What
//! it does once it runs must not move a millimetre, so this file pins today's
//! behaviour BEFORE the change, the way a sealed surface is extended.
//!
//! Five properties, each measured through a live colony:
//!   1. a shutdown acks and the task ends
//!   2. dead letters pushed before the shutdown are in `colony.db` afterwards
//!   3. dead letters produced DURING the teardown drain are persisted too
//!      (the W6d flush that has no post-select drain behind it) — measured with
//!      the drain phase switched OFF, see that test's own comment
//!   4. a rescued mailbox with no successor lands in the DLQ, not in the void
//!   5. the colony.db writer thread is joined, so the file is closed and
//!      readable by a fresh connection
//!
//! Every assertion is a POSITIVE receipt: an ack that arrived, a join that
//! returned `Ok`, a DLQ row read back with the reason it must carry, a fresh
//! connection that opened the file and answered a query. Nothing here concludes
//! anything from an absence.

use meclaw_colony::ColonyMsg;
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// One persisted dead letter, as a fresh reader sees it.
#[derive(Debug, PartialEq, Eq)]
struct DlqRow {
    error_code: String,
    resolved_target: String,
    message_json: String,
}

/// Read the whole `dead_letters` table through a connection opened AFTER the
/// shutdown — which is itself part of the claim: the writer thread was joined,
/// so the file is closed and a second reader gets at it.
fn dlq_rows(db_path: &std::path::Path) -> Vec<DlqRow> {
    let conn = rusqlite::Connection::open(db_path)
        .expect("a fresh connection must open the file the joined writer left behind");
    let mut stmt = conn
        .prepare("SELECT error_code, resolved_target, message_json FROM dead_letters ORDER BY id")
        .expect("the dead_letters table must exist in colony.db");
    stmt.query_map([], |r| {
        Ok(DlqRow {
            error_code: r.get(0)?,
            resolved_target: r.get(1)?,
            message_json: r.get(2)?,
        })
    })
    .expect("query the persisted dead letters")
    .collect::<Result<Vec<_>, _>>()
    .expect("every dead-letter row must decode")
}

/// Register a cell whose mailbox holds exactly ONE message and which never
/// drains: the receiver is handed back to the test and never read by the cell.
///
/// Same construction as `gh162_a_full_mailbox_names_itself`. A mailbox of one
/// makes the wedge exact rather than statistical — the first delivery fills it,
/// the second parks the colony INSIDE its route arm, which is what lets a test
/// queue messages behind a `Shutdown` in a fixed order.
async fn register_never_draining(h: &ColonyHandle, path: Path) -> mpsc::Receiver<Message> {
    let (sender, receiver) = mpsc::channel::<Message>(1);
    let (peace_tx, peace_rx) = oneshot::channel();
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _peace_keep = peace_tx;
        std::future::pending::<()>().await;
    });
    let (ack, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Register {
            path,
            sender,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn: Box::new(|| unreachable!("this cell never dies")),
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "never-drains".into(),
            active: true,
            ack,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
    receiver
}

/// Property 1. The ack is sent and the colony task returns — both read as
/// values, not inferred from a hang that did not happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_acks_and_ends_the_task() {
    let h = ColonyHandle::new();
    let db_path = h.tempdir_path().join("colony.db");

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the shutdown ack must arrive within the failure marker")
        .expect("the teardown must SEND the ack, not drop the sender");

    tokio::time::timeout(MARKER, h.join_result())
        .await
        .expect("the colony task must end within the failure marker")
        .expect("the teardown ends the task normally, never by panic");

    assert!(db_path.exists(), "the writer must have closed a real file");
}

/// Property 2. A dead letter produced in the normal loop is in `colony.db`
/// after the teardown — read back with the reason it must carry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letters_written_before_the_shutdown_survive_it() {
    let h = ColonyHandle::new();
    let db_path = h.tempdir_path().join("colony.db");

    let doomed = MessageBuilder::new("/nowhere").build();
    let doomed_id = doomed.id;
    h.send_from(Path::new("/"), doomed).await;
    // Barrier: the colony inbox is FIFO and the loop is one task, so an ack for
    // a LATER message proves the route above was handled BEFORE the shutdown.
    h.add_hive_scope(Path::new("/barrier")).await;

    h.shutdown().await;

    let rows = dlq_rows(&db_path);
    assert_eq!(rows.len(), 1, "exactly the one dead letter, got: {rows:?}");
    assert_eq!(rows[0].error_code, "unresolved_path");
    assert_eq!(rows[0].resolved_target, "/nowhere");
    assert!(
        rows[0].message_json.contains(&doomed_id.to_string()),
        "the persisted envelope must be THIS message: {}",
        rows[0].message_json
    );
}

/// Property 3. A dead letter produced INSIDE the teardown's `try_recv` drain is
/// persisted as well — the W6d flush, which has no post-select drain behind it.
///
/// Determinism comes from the wedge: the colony parks inside its route arm on a
/// full mailbox, so everything queued after that point sits in the inbox in send
/// order. `Shutdown` goes in first, the doomed message behind it — so when the
/// colony is released it takes the `Shutdown`, closes the inbox, and finds the
/// doomed message in the `try_recv` loop of the teardown, not in the normal one.
///
/// **Why `shutdown_drain_timeout_ms: 0`.** The drain phase this lane added sits
/// in FRONT of the teardown and leaves the inbox open, so a message queued
/// behind the `Shutdown` is now taken by the ordinary loop and persisted by the
/// top-of-loop flush. That is correct behaviour — and it means this test would
/// silently stop measuring the property its name states: green, but about some
/// other code path. `0` is the documented off switch (ruling O7), the pre-#47
/// teardown byte for byte: the `Shutdown` closes the inbox in the same work
/// item it is taken in, so by the time the doomed message can be looked at, the
/// only reader left in the process is the `try_recv` loop of the teardown. The
/// branch is not inferred from the row it wrote — the two branches route
/// through the same `route_with_log` and would write the same row — it is
/// forced by the inbox being shut before the message is ever read. As a
/// by-product this pins the off switch itself, and the test stops paying a
/// drain deadline it never needed: measured, 10.19 s with the default budget
/// against 0.21 s with the drain off — the wedged cell keeps the colony from
/// ever reaching quiescence, so the drain ran to its deadline every time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letters_produced_inside_the_teardown_drain_survive_it() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_path = td.path().join("colony.db");
    // Read by `ColonyHandle` at construction time, so it has to exist first.
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"shutdown_drain_timeout_ms": 0}"#,
    )
    .expect("write colony.json");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    let mut held = register_never_draining(&h, Path::new("/wedged")).await;

    // Fills the single slot; the ack barrier says the delivery is done.
    h.send_from(Path::new("/"), MessageBuilder::new("/wedged").build())
        .await;
    h.add_hive_scope(Path::new("/barrier")).await;

    // This delivery finds capacity 0 and parks the colony in the route arm.
    h.send_from(Path::new("/"), MessageBuilder::new("/wedged").build())
        .await;

    // Queued behind the wedge, in this order: the shutdown, then the message
    // whose routing must therefore happen inside the teardown drain.
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    let doomed = MessageBuilder::new("/nowhere").build();
    let doomed_id = doomed.id;
    h.send_from(Path::new("/"), doomed).await;

    // Release the wedge. The received message is also the positive receipt that
    // the mailbox really was full — it is the one occupying the single slot.
    let held_first = tokio::time::timeout(MARKER, held.recv())
        .await
        .expect("the wedged mailbox must hand out its message within the marker")
        .expect("the wedged mailbox sender is alive");
    assert_eq!(
        held_first.target.as_str(),
        "/wedged",
        "the slot was occupied by the delivery that filled it"
    );
    // And the delivery the colony was parked on completes into the freed slot:
    // proof that the loop really was inside the route arm while the shutdown and
    // the doomed message were queued behind it.
    let held_second = tokio::time::timeout(MARKER, held.recv())
        .await
        .expect("the parked delivery must complete once the slot is free")
        .expect("the wedged mailbox sender is alive");
    assert_eq!(
        held_second.target.as_str(),
        "/wedged",
        "the parked delivery is the second message to the wedged cell"
    );

    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the shutdown ack must arrive within the failure marker")
        .expect("the teardown must SEND the ack, not drop the sender");
    tokio::time::timeout(MARKER, h.join_result())
        .await
        .expect("the colony task must end within the failure marker")
        .expect("the teardown ends the task normally, never by panic");

    let rows = dlq_rows(&db_path);
    assert_eq!(
        rows.len(),
        1,
        "exactly the one dead letter from the drain, got: {rows:?}"
    );
    assert_eq!(rows[0].error_code, "unresolved_path");
    assert_eq!(rows[0].resolved_target, "/nowhere");
    assert!(
        rows[0].message_json.contains(&doomed_id.to_string()),
        "the message routed inside the teardown drain is the one persisted: {}",
        rows[0].message_json
    );
}

/// Property 4. A mailbox rescued from a dying cell whose `CellDied` never
/// arrived has no successor to wait for. The teardown drains that map into the
/// DLQ as `cell_inactive` instead of letting it die with the task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescued_mailbox_without_a_successor_lands_in_the_dlq() {
    let h = ColonyHandle::new();
    let db_path = h.tempdir_path().join("colony.db");

    let orphan = MessageBuilder::new("/gone").build();
    let orphan_id = orphan.id;
    h.inbox_tx
        .send(ColonyMsg::MailboxRescued {
            path: Path::new("/gone"),
            messages: vec![orphan],
        })
        .await
        .expect("colony inbox closed");
    // Barrier: the ack proves the rescue was parked in `rescued_mailboxes`
    // before the shutdown arrived — no `CellDied` ever follows it.
    h.add_hive_scope(Path::new("/barrier")).await;

    h.shutdown().await;

    let rows = dlq_rows(&db_path);
    assert_eq!(
        rows.len(),
        1,
        "the rescued message is preserved as one dead letter, got: {rows:?}"
    );
    assert_eq!(rows[0].error_code, "cell_inactive");
    assert_eq!(
        rows[0].resolved_target, "/gone",
        "the entry names the cell whose mailbox was rescued"
    );
    assert!(
        rows[0].message_json.contains(&orphan_id.to_string()),
        "the rescued envelope itself is what got persisted: {}",
        rows[0].message_json
    );
}

/// Property 5. `colony_db.shutdown_async()` joins the writer thread, so the
/// file is closed: a fresh connection opens it and reads back a row that was
/// only ever enqueued on the writer channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_thread_is_joined_and_the_file_reads_back() {
    let h = ColonyHandle::new();
    h.spawn(Path::new("/x"), || EchoMockCell::new(Path::new("/x")))
        .await;
    let db_path = h.tempdir_path().join("colony.db");

    h.shutdown().await;

    let conn = rusqlite::Connection::open(&db_path)
        .expect("a fresh connection must open the file the joined writer left behind");
    let path: String = conn
        .query_row("SELECT path FROM registry WHERE path='/x'", [], |r| {
            r.get(0)
        })
        .expect("the registry row enqueued before the shutdown must be readable");
    assert_eq!(path, "/x", "the flushed row is the one the register wrote");
}
