//! #1233 (OR-programm-tm.H3.orch-1): the shutdown teardown takes no mutation.
//!
//! Rows into a RUNNING store travel on the store's seed lane, and the door
//! answers only after the store's receipt. The teardown cannot wait for one:
//! its inbox is closed before the drain of what was buffered, so an order sent
//! there never arrives, and the only other way to land the rows would be a
//! second connection into a live cell's `cell.db` (cell boundaries are hard).
//! So the teardown refuses every mutation the way the drain does (GH #47),
//! before anything is staged — at all three doors: the `Mutation` message, the
//! `MutationDoor` message (`--apply`/HTTP) and a cell's emission to
//! `/colony/mutations`.
//!
//! Order comes from the colony's one inline wait, not from a full mailbox
//! (GH #1330): since the mailbox overflow (GH #850, delivery via `try_send`)
//! a full mailbox no longer stops the loop, so the old wedge let the colony
//! take the `Shutdown` and close its inbox while the test was still knocking
//! (`colony inbox closed` at the `MutationDoor` send, once in CI). Here a
//! disconnect mutation peace-stops a registered cell and the colony blocks on
//! its death ack; the colony's own signal (`with_death_ack_wait_signal`) says
//! it is there. Only then are the `Shutdown` and the three knocks queued, in
//! send order, and only then does the test hand over the death ack. With the
//! drain switched off (`shutdown_drain_timeout_ms: 0`) the `Shutdown` closes
//! the inbox in the work item it is taken in, so the knocks are read only by
//! the teardown's drain.
//! Every assertion is read after the colony task has ended (quiescence): the
//! two answers, the dead letter of the emission, and the store's table, whose
//! birth row proves the read looked at the right `cell.db`.

use meclaw_colony::{
    CellFactory, ColonyMsg, MutationDoorOutcome, MutationOutcome, set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::deadline::{HANG, HANG_MS};
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::{mpsc, oneshot};

const NODE: &str = "keeper";

/// A `store` that declares one table and answers nothing (`terminal`).
fn store_config() -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {"terminal": true, "schema": {"policy": {"rule_id": "text"}}},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

fn seed(rule_id: &str) -> Value {
    json!({"seed_rows": [{"target": NODE, "table": "policy", "rows": [{"rule_id": rule_id}]}]})
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

/// Every `rule_id` in the store's `policy` table, read after the colony ended.
fn store_rows(root: &std::path::Path) -> Vec<String> {
    let templates = root.join("templates");
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir == templates {
            continue;
        }
        for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                if entry.file_name() == NODE && p.join("cell.db").is_file() {
                    found.push(p.join("cell.db"));
                }
                stack.push(p);
            }
        }
    }
    assert_eq!(found.len(), 1, "exactly one store cell.db: {found:?}");
    let conn = rusqlite::Connection::open(&found[0]).expect("open the store's cell.db");
    let mut stmt = conn
        .prepare("SELECT rule_id FROM policy ORDER BY rule_id")
        .expect("the birth seed created the policy table");
    stmt.query_map([], |r| r.get::<_, String>(0))
        .expect("query policy")
        .map(|r| r.expect("read a policy row"))
        .collect()
}

fn assert_refused_by_the_drain(outcome: &MutationOutcome, door: &str) {
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "shutdown_draining",
            "the teardown refuses the {door} with the drain's code: {outcome:?}"
        ),
        other => panic!("the teardown must refuse the {door}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_teardown_refuses_every_mutation_door_and_no_row_lands() {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"shutdown_drain_timeout_ms": 0}"#,
    )
    .expect("write colony.json");
    std::fs::create_dir_all(td.path().join("main")).expect("root cell dir");
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("root cell config");
    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let (h, mut parked_rx) = ColonyHandle::new_with_factories_and_death_ack_signal_at(
        &td,
        vec![("store".to_string(), factory)],
    );

    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(&tpl).expect("template dir");
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).expect("write");
    std::fs::write(tpl.join("config.json"), store_config().to_string()).expect("write");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("the rescan must not have aborted");

    // Grow the store with a birth row: written into the staged cell.db (birth
    // is substrate right), and the anchor that the read below looks at the
    // right database.
    let mut grow = seed("r0");
    grow["add_nodes"] = json!([{"name": NODE, "template": NODE}]);
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": grow}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    let grown = tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("the grow mutation must be answered")
        .expect("mutation ack");
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing the store must commit: {grown:?}"
    );

    let parked = park_the_colony(&h, &mut parked_rx).await;

    // Queued while the colony is parked, in this order: the shutdown, then the
    // three knocks, each seeding the now RUNNING store. The inbox is open —
    // only the shutdown, once taken, closes it.
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Shutdown { ack: shutdown_tx })
        .await
        .expect("colony inbox closed");
    let (mutation_tx, mutation_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": seed("r1")}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: mutation_tx,
        })
        .await
        .expect("colony inbox closed");
    let (door_tx, door_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload: json!({"scope": "/", "diff": seed("r2")}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: door_tx,
        })
        .await
        .expect("colony inbox closed");
    h.send_from(
        Path::new("/parker"),
        MessageBuilder::new("/colony/mutations")
            .with_inline_content(json!({"messages": [], "scope": "/", "diff": seed("r3")}))
            .build(),
    )
    .await;

    release(parked).await;

    tokio::time::timeout(HANG, shutdown_rx)
        .await
        .expect("the shutdown ack must arrive")
        .expect("the teardown must SEND the ack");
    tokio::time::timeout(HANG, h.join_result())
        .await
        .expect("the colony task must end")
        .expect("the teardown ends the task normally, never by panic");

    let single = tokio::time::timeout(HANG, mutation_rx)
        .await
        .expect("the mutation must be answered")
        .expect("the teardown answers the mutation, it does not drop it");
    assert_refused_by_the_drain(&single, "Mutation message");
    match tokio::time::timeout(HANG, door_rx)
        .await
        .expect("the mutation door must be answered")
        .expect("the teardown answers the door, it does not drop it")
    {
        MutationDoorOutcome::Single(outcome) => {
            assert_refused_by_the_drain(&outcome, "MutationDoor message")
        }
        other => panic!("the door's refusal is the single form, got {other:?}"),
    }
    // The emitted knock's dead letter, asked through the door of a fresh
    // colony over the same root (the teardown flushed it before the join).
    let h2 = ColonyHandle::new_with_factories_at(&td, vec![]);
    let (dl_tx, dl_rx) = oneshot::channel();
    h2.inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: Some("shutdown_draining".into()),
            limit: 100,
            ack: dl_tx,
        })
        .await
        .expect("colony inbox");
    let recorded: Vec<(String, String)> = tokio::time::timeout(HANG, dl_rx)
        .await
        .expect("the dead-letter read must be answered")
        .expect("dead-letter read ack")
        .entries
        .into_iter()
        .map(|e| (e.sender_path, e.resolved_target))
        .collect();
    assert_eq!(
        recorded,
        vec![("/parker".to_string(), "/colony/mutations".to_string())],
        "the emitted knock is dead-lettered with the drain's code"
    );
    h2.shutdown().await;

    assert_eq!(
        store_rows(td.path()),
        vec!["r0".to_string()],
        "only the birth row is in the store: no knock of the teardown wrote into its cell.db"
    );
}
