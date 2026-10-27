//! GH #47: the shutdown teardown is about to grow a phase in front of it. What
//! it does once it runs must not move a millimetre, so this file pins today's
//! behaviour BEFORE the change, the way a sealed surface is extended.
//!
//! Five properties, each measured through a live colony:
//!   1. a shutdown acks and the task ends
//!   2. dead letters pushed before the shutdown are in the store afterwards
//!   3. dead letters produced DURING the teardown drain are persisted too
//!      (the W6d flush that has no post-select drain behind it) — measured with
//!      the drain phase switched OFF, see that test's own comment
//!   4. a rescued mailbox with no successor lands in the DLQ, not in the void
//!   5. the store's writer thread is joined, so the store is closed and a
//!      fresh colony over the same root reads it back
//!
//! Every assertion is a POSITIVE receipt: an ack that arrived, a join that
//! returned `Ok`, a DLQ row read back with the reason it must carry, a fresh
//! colony that booted over the root and answered a read door. Nothing here
//! concludes anything from an absence. No test opens the store file itself
//! (docs/development-rules.md § 11, ban `rs-colony-db`): every read goes
//! through the doors of a second colony over the same root (GH #1330).

use meclaw_colony::{ColonyMsg, MutationOutcome, set_term_timeout_ms_for_test};
use meclaw_core::serde_json::json;
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::deadline::{HANG, HANG_MS};
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use tokio::sync::{mpsc, oneshot};

/// One persisted dead letter, as a fresh colony's read door returns it.
#[derive(Debug, PartialEq, Eq)]
struct DlqRow {
    error_code: String,
    resolved_target: String,
    /// The id parsed out of the persisted envelope.
    message_id: Option<String>,
}

/// The limit of the one dead-letter read in [`dlq_rows`]; a page that reaches
/// it is not the whole DLQ.
const DLQ_PAGE: usize = 1000;

/// Read every persisted dead letter through a colony booted AFTER the
/// shutdown over the same root — which is itself part of the claim: the
/// writer thread was joined, so the store is closed and a second colony opens
/// it and answers. The second colony is shut down before the rows are judged.
async fn dlq_rows(td: &tempfile::TempDir) -> Vec<DlqRow> {
    let h2 = ColonyHandle::new_with_factories_at(td, vec![]);
    let (ack, rx) = oneshot::channel();
    h2.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: DLQ_PAGE,
            ack,
        })
        .await
        .expect("the fresh colony's inbox");
    let rows = tokio::time::timeout(HANG, rx)
        .await
        .expect("the dead-letter read must be answered within the failure marker")
        .expect("dead-letter read ack")
        .entries
        .into_iter()
        .map(|e| DlqRow {
            error_code: e.error_code,
            resolved_target: e.resolved_target,
            message_id: e.message_id,
        })
        .collect::<Vec<DlqRow>>();
    h2.shutdown().await;
    // One read without a cursor is the whole DLQ only while it is short of its
    // limit (809, K8 "a reader takes one page for the whole").
    assert!(
        rows.len() < DLQ_PAGE,
        "one page of {DLQ_PAGE} dead letters is full: the read is not the whole DLQ"
    );
    rows
}

/// The colony, blocked inside a disconnect mutation on the death ack of
/// `/parker`. Nothing in the inbox is read until `death_ack` is sent.
struct Parked {
    death_ack: oneshot::Sender<()>,
    mutation: tokio::task::JoinHandle<MutationOutcome>,
    _mailbox: mpsc::Receiver<Message>,
}

/// Park the colony in its inline death-ack wait (the only place the loop
/// blocks on something a test holds). Two positive receipts say it is there:
/// the peace-stop reached `/parker`, and the colony fired its test-only signal
/// right before blocking. The term timeout is a hang marker here, never the
/// way the park ends: the park mutation must come back `Committed`.
async fn park_the_colony(h: &ColonyHandle, parked: &mut mpsc::Receiver<()>) -> Parked {
    set_term_timeout_ms_for_test(HANG_MS);
    let (sender, mailbox) = mpsc::channel::<Message>(8);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (death_ack, death_ack_rx) = oneshot::channel::<()>();
    let (peace_tx, peace_rx) = oneshot::channel();
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _peace_keep = peace_tx;
        std::future::pending::<()>().await;
    });
    let (ack, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Register {
            path: Path::new("/parker"),
            sender,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: Some(stop_tx),
            death_ack_rx: Some(death_ack_rx),
            respawn: Box::new(|| unreachable!("this cell never dies")),
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "parker".into(),
            active: true,
            ack,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
    h.add_edge(Uuid::now_v7(), Path::new("/parker"), Path::new("/anchor"))
        .await;

    let inbox_tx = h.inbox_tx.clone();
    let mutation = tokio::spawn(async move {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Mutation {
                payload: json!({"scope": "/", "diff": {"remove_edges": [
                    {"match": {"from": "parker", "to": "anchor"}}
                ]}}),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("park mutation ack")
    });
    tokio::time::timeout(HANG, stop_rx)
        .await
        .expect("the disconnect must peace-stop /parker")
        .expect("the colony sends the stop, it does not drop it");
    tokio::time::timeout(HANG, parked.recv())
        .await
        .expect("the colony must reach its inline death-ack wait")
        .expect("the death-ack-wait signal channel is open");
    Parked {
        death_ack,
        mutation,
        _mailbox: mailbox,
    }
}

/// Hand over the death ack: the park mutation completes and the loop reads
/// the inbox again, in send order.
async fn release(parked: Parked) {
    parked.death_ack.send(()).expect("the colony still waits");
    let outcome = tokio::time::timeout(HANG, parked.mutation)
        .await
        .expect("the park mutation must be answered")
        .expect("the park mutation task");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the park ends by the death ack, not by the term timeout: {outcome:?}"
    );
}

/// Property 1. The ack is sent and the colony task returns — both read as
/// values, not inferred from a hang that did not happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_acks_and_ends_the_task() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("the shutdown ack must arrive within the failure marker")
        .expect("the teardown must SEND the ack, not drop the sender");

    tokio::time::timeout(HANG, h.join_result())
        .await
        .expect("the colony task must end within the failure marker")
        .expect("the teardown ends the task normally, never by panic");

    // The writer closed a real store: a fresh colony boots over the root and
    // its read door answers (the answer itself, not its content, is the receipt).
    let _ = dlq_rows(&td).await;
}

/// Property 2. A dead letter produced in the normal loop is in the store
/// after the teardown — read back with the reason it must carry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letters_written_before_the_shutdown_survive_it() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let doomed = MessageBuilder::new("/nowhere").build();
    let doomed_id = doomed.id;
    h.send_from(Path::new("/"), doomed).await;
    // Barrier: the colony inbox is FIFO and the loop is one task, so an ack for
    // a LATER message proves the route above was handled BEFORE the shutdown.
    h.add_hive_scope(Path::new("/barrier")).await;

    h.shutdown().await;

    let rows = dlq_rows(&td).await;
    assert_eq!(rows.len(), 1, "exactly the one dead letter, got: {rows:?}");
    assert_eq!(rows[0].error_code, "unresolved_path");
    assert_eq!(rows[0].resolved_target, "/nowhere");
    assert_eq!(
        rows[0].message_id,
        Some(doomed_id.to_string()),
        "the persisted envelope must be THIS message"
    );
}

/// Property 3. A dead letter produced INSIDE the teardown's `try_recv` drain is
/// persisted as well — the W6d flush, which has no post-select drain behind it.
///
/// Determinism comes from the colony's one inline wait (GH #1330): a
/// disconnect mutation parks the loop on the death ack of `/parker`, and the
/// colony's own signal says it is there. Everything queued while it is parked
/// sits in the inbox in send order. `Shutdown` goes in first, the doomed
/// message behind it — so when the death ack is handed over, the colony takes
/// the `Shutdown`, closes the inbox, and finds the doomed message in the
/// `try_recv` loop of the teardown, not in the normal one. (The old wedge — a
/// never-read mailbox of one — stopped holding with the mailbox overflow, GH
/// #850: a full mailbox no longer stops the loop, so the `Shutdown` could
/// close the inbox before the doomed message was sent.)
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
/// against 0.21 s with the drain off — a cell that never drains keeps the colony from
/// ever reaching quiescence, so the drain ran to its deadline every time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letters_produced_inside_the_teardown_drain_survive_it() {
    let td = tempfile::TempDir::new().expect("tempdir");
    // Read by `ColonyHandle` at construction time, so it has to exist first.
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"shutdown_drain_timeout_ms": 0}"#,
    )
    .expect("write colony.json");
    let (h, mut parked_rx) = ColonyHandle::new_with_factories_and_death_ack_signal_at(&td, vec![]);
    let parked = park_the_colony(&h, &mut parked_rx).await;

    // Queued while the colony is parked, in this order: the shutdown, then the
    // message whose routing must therefore happen inside the teardown drain.
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    let doomed = MessageBuilder::new("/nowhere").build();
    let doomed_id = doomed.id;
    h.send_from(Path::new("/"), doomed).await;

    release(parked).await;

    tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("the shutdown ack must arrive within the failure marker")
        .expect("the teardown must SEND the ack, not drop the sender");
    tokio::time::timeout(HANG, h.join_result())
        .await
        .expect("the colony task must end within the failure marker")
        .expect("the teardown ends the task normally, never by panic");

    let rows = dlq_rows(&td).await;
    assert_eq!(
        rows.len(),
        1,
        "exactly the one dead letter from the drain, got: {rows:?}"
    );
    assert_eq!(rows[0].error_code, "unresolved_path");
    assert_eq!(rows[0].resolved_target, "/nowhere");
    assert_eq!(
        rows[0].message_id,
        Some(doomed_id.to_string()),
        "the message routed inside the teardown drain is the one persisted"
    );
}

/// Property 4. A mailbox rescued from a dying cell whose `CellDied` never
/// arrived has no successor to wait for. The teardown drains that map into the
/// DLQ as `cell_inactive` instead of letting it die with the task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescued_mailbox_without_a_successor_lands_in_the_dlq() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

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

    let rows = dlq_rows(&td).await;
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
    assert_eq!(
        rows[0].message_id,
        Some(orphan_id.to_string()),
        "the rescued envelope itself is what got persisted"
    );
}

/// Property 5. `colony_db.shutdown_async()` joins the writer thread, so the
/// store is closed: a fresh colony over the same root boots from it and its
/// message-log door returns a row that was only ever enqueued on the writer
/// channel of the first colony. (The row is a message-log row, not the
/// registry row this property read before GH #1330: a rebooted colony keeps
/// no registry door over rows it did not hydrate, and the log door is the one
/// the testing rules name — the claim, a joined writer and a store that reads
/// back, is the same.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_thread_is_joined_and_the_file_reads_back() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    h.spawn(Path::new("/x"), || EchoMockCell::new(Path::new("/x")))
        .await;
    let probe = MessageBuilder::new("/x").build();
    let probe_id = probe.id;
    h.send_from(Path::new("/"), probe).await;
    // Barrier: the colony inbox is FIFO and the loop is one task, so this ack
    // proves the route above (and its log enqueue) came BEFORE the shutdown.
    h.add_hive_scope(Path::new("/barrier")).await;

    h.shutdown().await;

    let h2 = ColonyHandle::new_with_factories_at(&td, vec![]);
    let rows: Vec<(String, String)> = h2
        .read_log(meclaw_colony::api_dto::MessageLogFilter {
            id: Some(probe_id.to_string()),
            ..Default::default()
        })
        .await
        .into_iter()
        .map(|r| (r.id, r.to_path))
        .collect();
    h2.shutdown().await;
    assert_eq!(
        rows,
        vec![(probe_id.to_string(), "/x".to_string())],
        "the flushed row is the one the route enqueued"
    );
}
