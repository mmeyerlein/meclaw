//! #1233 (OR-programm-tm.H3.6): the seed wait table has a deadline.
//!
//! The door answers `committed` for a `seed_rows` into a RUNNING store only
//! after that store's receipt on its seed lane. A store that never answers — a
//! dead or failed one, an order that landed in the DLQ — is normal operation,
//! not an edge case, and without a deadline the knock would never be answered.
//! The topology is committed and durable by then (OR-programm-tm.H3.3), so the
//! answer stays `committed` and the missing receipt enters the ledger as an
//! error with its own code.
//!
//! The store here is a stateful cell registered as type `store` that takes
//! every message and emits nothing, so its lane is never answered. Its short
//! `cell.message_timeout` is configuration (the base of the wait), never a
//! verdict: the assertions are the answer arriving and the ledger naming the
//! code; the only clock is the hang marker.

use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::HANG;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const NODE: &str = "keeper";

/// A `store` that declares one table and answers nothing: `terminal` keeps
/// the stateful mock silent, so the seed lane stays unanswered.
fn store_config() -> Value {
    json!({
        "cell": {"type": "store", "message_timeout": 200},
        "params": {"terminal": true, "schema": {"policy": {"rule_id": "text"}}},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("the door never answered: the seed wait has no deadline")
        .expect("mutation ack")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_that_never_answers_its_seed_lane_still_gets_a_committed_answer() {
    let td = tempfile::TempDir::new().expect("tempdir");
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
    let grown = mutate(
        &h,
        json!({"scope": "/", "diff": {"add_nodes": [{"name": NODE, "template": NODE}]}}),
    )
    .await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing the store must commit: {grown:?}"
    );

    // A second mutation: the store is running now, so its rows travel on the
    // lane and the door waits for a receipt that never comes.
    let out = mutate(
        &h,
        json!({"scope": "/", "diff": {"seed_rows": [
            {"target": NODE, "table": "policy", "rows": [{"rule_id": "r1"}]}
        ]}}),
    )
    .await;
    let id = match &out {
        MutationOutcome::Committed { id, .. } => id.clone(),
        other => panic!("the topology is committed, so is the answer: {other:?}"),
    };

    let row = h
        .mutations_audit(None)
        .await
        .into_iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("the audit names no mutation {id}"));
    assert_eq!(row.status, "committed");
    let payload: Value =
        meclaw_core::serde_json::from_str(&row.payload_json).expect("payload json");
    let receipts = payload["seed_receipts"]
        .as_array()
        .unwrap_or_else(|| panic!("the ledger names no seed receipt: {payload}"));
    assert_eq!(receipts.len(), 1, "one entry per running store: {payload}");
    let r = &receipts[0];
    assert_eq!(r["target"], json!(format!("/{NODE}")), "{r}");
    assert_eq!(r["table"], json!("policy"), "{r}");
    assert_eq!(r["error_code"], json!("seed_receipt_timeout"), "{r}");
    h.shutdown().await;
}

/// #1315 (OR-agent-2.N2.6): a store that is INACTIVE when the mutation leaves
/// it — here because its hive has no external edge, as `/os` had none in the
/// gh1310 member colony — can never answer: routing dead-letters its seed
/// order as `cell_inactive` without waking it. The door therefore books the
/// refusal itself at once (`seed_target_inactive`) instead of waiting out the
/// store's backstop. The marker is the code: the deadline path writes
/// `seed_receipt_timeout`, so `seed_target_inactive` in the ledger means the
/// deadline never ran. The store's backstop is set far beyond the hang marker,
/// so a door that still waited for it would not answer within it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_that_is_inactive_gets_its_refusal_without_a_wait() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let main = td.path().join("main");
    std::fs::create_dir_all(main.join("h").join(NODE)).expect("cell dirs");
    std::fs::write(
        main.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("root config");
    // The hive serves its store but nothing outside reaches the hive: the unit
    // has no external edge, so the boot derives it inactive.
    std::fs::write(
        main.join("h/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"./keeper"}]}}}"#,
    )
    .expect("hive config");
    let mut cfg = store_config();
    cfg["cell"]["message_timeout"] = json!(3_600_000);
    std::fs::write(
        main.join("h").join(NODE).join("config.json"),
        cfg.to_string(),
    )
    .expect("store config");
    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory.clone())]);
    let mut boot = meclaw_colony::CellFactoryRegistry::new();
    boot.insert("store".into(), factory);
    meclaw_colony::bootstrap_from_filesystem(td.path(), &boot, &h.runtime())
        .await
        .expect("the boot from the tree");

    let out = mutate(
        &h,
        json!({"scope": "/", "diff": {"seed_rows": [
            {"target": format!("h/{NODE}"), "table": "policy", "rows": [{"rule_id": "r1"}]}
        ]}}),
    )
    .await;
    let id = match &out {
        MutationOutcome::Committed { id, .. } => id.clone(),
        other => panic!("the answer stays committed: {other:?}"),
    };
    let row = h
        .mutations_audit(None)
        .await
        .into_iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("the audit names no mutation {id}"));
    let payload: Value =
        meclaw_core::serde_json::from_str(&row.payload_json).expect("payload json");
    let receipts = payload["seed_receipts"]
        .as_array()
        .unwrap_or_else(|| panic!("the ledger names no seed receipt: {payload}"));
    assert_eq!(receipts.len(), 1, "{payload}");
    let r = &receipts[0];
    assert_eq!(r["target"], json!(format!("/h/{NODE}")), "{r}");
    assert_eq!(r["error_code"], json!("seed_target_inactive"), "{r}");
    h.shutdown().await;
}
