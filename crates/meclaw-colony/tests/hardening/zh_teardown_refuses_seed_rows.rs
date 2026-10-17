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
//! Determinism comes from the wedge of `gh47_the_teardown_behaves_exactly_as_before`:
//! the colony parks in its route arm on a full mailbox, the `Shutdown` and the
//! three knocks queue behind it in send order, and with the drain switched off
//! (`shutdown_drain_timeout_ms: 0`) the `Shutdown` closes the inbox in the work
//! item it is taken in, so the knocks are read only by the teardown's drain.
//! Every assertion is read after the colony task has ended (quiescence): the
//! two answers, the dead letter of the emission, and the store's table, whose
//! birth row proves the read looked at the right `cell.db`.

use meclaw_colony::{CellFactory, ColonyMsg, MutationDoorOutcome, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::deadline::HANG;
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

/// Register a cell whose mailbox holds one message and is never read, so the
/// second delivery parks the colony inside its route arm (same construction as
/// `gh47_the_teardown_behaves_exactly_as_before`).
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
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory)]);

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

    // The wedge: the first delivery fills the single slot, the second parks
    // the colony inside its route arm.
    let mut held = register_never_draining(&h, Path::new("/wedged")).await;
    h.send_from(Path::new("/"), MessageBuilder::new("/wedged").build())
        .await;
    h.add_hive_scope(Path::new("/barrier")).await;
    h.send_from(Path::new("/"), MessageBuilder::new("/wedged").build())
        .await;

    // Queued behind the wedge, in this order: the shutdown, then the three
    // knocks, each seeding the now RUNNING store.
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
        Path::new("/wedged"),
        MessageBuilder::new("/colony/mutations")
            .with_inline_content(json!({"messages": [], "scope": "/", "diff": seed("r3")}))
            .build(),
    )
    .await;

    // Release the wedge: both deliveries come out, the parked one completes.
    for _ in 0..2 {
        let m = tokio::time::timeout(HANG, held.recv())
            .await
            .expect("the wedged mailbox must hand out its messages")
            .expect("the wedged mailbox sender is alive");
        assert_eq!(m.target.as_str(), "/wedged");
    }

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
        vec![("/wedged".to_string(), "/colony/mutations".to_string())],
        "the emitted knock is dead-lettered with the drain's code"
    );
    h2.shutdown().await;

    assert_eq!(
        store_rows(td.path()),
        vec!["r0".to_string()],
        "only the birth row is in the store: no knock of the teardown wrote into its cell.db"
    );
}
