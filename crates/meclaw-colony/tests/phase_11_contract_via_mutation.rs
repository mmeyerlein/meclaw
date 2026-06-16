//! T23: Mutation-Pfad reicht contract.multi_send_capable aus der kopierten
//! Template-config.json bis in den spawn_cell-Aufruf durch.
//!
//! Beweis-Kette:
//!   1. Template-Verzeichnis mit config.json contract.multi_send_capable=true.
//!   2. Colony + RescanTemplates.
//!   3. Mutation add_nodes.
//!   4. TrackingFactory zeichnet den ContractView-Wert auf.
//!   5. Assert: multi_send_capable==true im aufgezeichneten ContractView.
//!
//! TrackingFactory ist Test-Instrumentation only — kein Production-State.

use meclaw_colony::{CellFactory, ColonyMsg, ContractView, MutationOutcome, SpawnedCellKind};
use meclaw_core::{CellEmission, JsonValue, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::{Arc, Mutex};

/// Test-only factory wrapper that records the ContractView passed to spawn_cell.
struct TrackingFactory {
    inner: Arc<EchoCellFactory>,
    captured: Arc<Mutex<Option<ContractView>>>,
}

impl CellFactory for TrackingFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        self.inner.validate_params(params)
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Record the ContractView (test instrumentation only).
        *self.captured.lock().unwrap() = Some(contract.clone());
        self.inner.clone().spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            None,
            mailbox_capacity,
        )
    }
}

/// Helper: send a mutation and await the outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
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

/// Helper: trigger a templates rescan.
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contract_view_propagates_from_template_config_json_to_spawn() {
    let td = tempfile::TempDir::new().unwrap();

    // 1. Template mit contract.multi_send_capable=true.
    let tpl_dir = td.path().join("templates").join("tracked");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        r#"{"name":"tracked","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/dev/null"},"contract":{"version":"0.1.0","settings":{},"consumes":{},"multi_send_capable":true}}"#,
    )
    .unwrap();

    // 2. TrackingFactory vorbereiten.
    let captured: Arc<Mutex<Option<ContractView>>> = Arc::new(Mutex::new(None));
    let tracking_factory = Arc::new(TrackingFactory {
        inner: Arc::new(EchoCellFactory),
        captured: captured.clone(),
    }) as Arc<dyn CellFactory>;

    // 3. Colony mit TrackingFactory hochfahren.
    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo".to_string(), tracking_factory)]);

    // Templates scannen.
    rescan_templates(&h, td.path().join("templates")).await;

    // 4. Mutation: add_nodes mit Template "tracked@1.0.0".
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{
                    "name": "tracked",
                    "template": "tracked@1.0.0"
                }]
            }
        }),
    )
    .await;

    // 5. Mutation muss Committed sein.
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Mutation muss Committed sein, got {outcome:?}"
    );

    h.shutdown().await;

    // 6. Beweis: captured ContractView hat multi_send_capable==true.
    let cv = captured
        .lock()
        .unwrap()
        .clone()
        .expect("TrackingFactory muss einmal aufgerufen worden sein");
    assert!(
        cv.multi_send_capable,
        "ContractView.multi_send_capable muss true sein (aus Template-config.json)"
    );
}
