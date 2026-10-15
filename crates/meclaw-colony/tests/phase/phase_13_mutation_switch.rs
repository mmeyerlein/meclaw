//! Phase-13 step 13-L-1 mutation-spawn switch:
//! stateful cells instantiated via `ColonyMsg::Mutation { add_nodes }` land, as
//! on the bootstrap path, as `SpawnedCellKind::Dormant` →
//! `lifecycle_status == "NotYetSpawned"` directly after
//! `MutationOutcome::Committed`.
//!
//! The switch was already wired in 13-K-2 in `colony.rs::handle_mutation`
//! (a match on `SpawnedCellKind`); this test is the proof that the
//! mutation-spawn path delivers the behaviour correctly.
//!
//! **Phase-13.5 update (task 7, A2)**: the former hardcode limitation
//! (`idle_timeout = DEFAULT_IDLE_TIMEOUT_MS`, `cell_timeout = 0`) is fixed —
//! `StagedDir` now carries `cell_timeout` + `idle_timeout_ms` from the
//! substituted `config.json`, and the mutation spawn uses the same `match`
//! mapping logic as `bootstrap_apply.rs`. This test covers only the default
//! case (a template without `cell.timeout` → 0 → idle-default dormant); the
//! `cell.timeout = -1` path is covered in
//! `phase_13_5_lifecycle_3b_reconnect.rs` (demo h).

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Helper: send a mutation and await the outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
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
        .unwrap();
    ack_rx.await.unwrap()
}

/// Scannt ein Templates-Verzeichnis via RescanTemplates.
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// Phase-13-L-1: stateful cells created via an `add_nodes` mutation must appear
/// in the registry as `NotYetSpawned` directly after `MutationOutcome::Committed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_nodes_stateful_registers_dormant() {
    let td = tempfile::TempDir::new().unwrap();

    // FS tree: ONLY the root hive. The /persist cell only comes into being via the mutation.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template `persist_mock` for add_nodes(template=…).
    let tpl_dir = td.path().join("templates").join("persist_mock");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    // A high idle_timeout so the cell does not drift into idle sleep on its own
    // during the assertion window. `terminal: true` → the cell emits no outputs
    // (anti-cascade, no /sink needed).
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // Boot the colony with the PersistCellFactory under "persist_mock".
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory)],
    );

    // Templates-Scan.
    rescan_templates(&h, td.path().join("templates")).await;

    // Mutation: add_nodes(name="added", template="persist_mock") under scope "/".
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{
                    "name": "added",
                    "template": "persist_mock"
                }]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "mutation must be Committed; got {outcome:?}"
    );

    // ReadRegistry: /added must be registered as NotYetSpawned (dormant).
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
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
    let reply = ack_rx.await.unwrap();
    let e = reply
        .entries
        .into_iter()
        .find(|e| e.path == "/added")
        .expect("/added must be registered after add_nodes mutation");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "stateful cell via add_nodes-mutation must register as Dormant (NotYetSpawned)"
    );

    h.shutdown().await;
}
