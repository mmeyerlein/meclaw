//! Phase-13 Step 13-L-1 Mutation-Spawn-Switch:
//! stateful Cells, die via `ColonyMsg::Mutation { add_nodes }` instanziiert
//! werden, landen analog Bootstrap als `SpawnedCellKind::Dormant` →
//! `lifecycle_status == "NotYetSpawned"` direkt nach `MutationOutcome::Committed`.
//!
//! Der Switch wurde in 13-K-2 bereits in `colony.rs::handle_mutation`
//! verdrahtet (Match auf `SpawnedCellKind`); dieser Test ist der Nachweis,
//! dass der Mutation-Spawn-Pfad das Verhalten korrekt liefert.
//!
//! **Phase-13.5-Update (Task 7, A2)**: die frühere Hardcode-Limitation
//! (`idle_timeout = DEFAULT_IDLE_TIMEOUT_MS`, `cell_timeout = 0`) ist behoben —
//! `StagedDir` trägt jetzt `cell_timeout` + `idle_timeout_ms` aus der
//! substituierten `config.json`, und der Mutation-Spawn nutzt dieselbe
//! `match`-Mapping-Logik wie `bootstrap_apply.rs`. Dieser Test deckt nur den
//! Default-Fall ab (Template ohne `cell.timeout` → 0 → Idle-Default-Dormant);
//! der `cell.timeout = -1`-Pfad ist in
//! `phase_13_5_lifecycle_3b_reconnect.rs` (Demo h) abgedeckt.

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
    ack_rx.await.unwrap();
}

/// Phase-13-L-1: Stateful Cells via `add_nodes`-Mutation müssen direkt nach
/// `MutationOutcome::Committed` als `NotYetSpawned` in der Registry stehen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_nodes_stateful_registers_dormant() {
    let td = tempfile::TempDir::new().unwrap();

    // FS-Tree: NUR root-hive. Die /persist-Cell entsteht erst durch die Mutation.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Template `persist_mock` für add_nodes(template=…).
    let tpl_dir = td.path().join("templates").join("persist_mock");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    // High idle_timeout, damit die Cell während des Assertion-Fensters nicht
    // von alleine in Idle-Sleep wandert. `terminal: true` → Cell emittiert
    // keine Outputs (Anti-Cascade, kein /sink nötig).
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // Colony hochfahren mit PersistCellFactory unter "persist_mock".
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

    // Mutation: add_nodes(name="added", template="persist_mock") unter scope "/".
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
        "Mutation muss Committed sein; got {outcome:?}"
    );

    // ReadRegistry: /added muss als NotYetSpawned (Dormant) registriert sein.
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
