//! Slice 11-F T17: validate is additive. Both levels become observable via a
//! real mutation round-trip.
//!
//! Test 1: the template is missing from the registry → Rejected with error_code
//!         "template_missing".
//! Test 2: the template exists, but the cell.type from its config.json has no
//!         registered CellFactory → Rejected with error_code "unknown_cell_type".

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;

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

/// Scannt ein Templates-Verzeichnis via RescanTemplates.
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
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

/// Test 1: the template name is not in the registry → error_code "template_missing".
///
/// The colony starts with an empty templates registry (no rescan).
/// A mutation add_nodes with template="ghost" is sent.
/// Expectation: Rejected { error_code: "template_missing" }.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_with_missing_template_is_rejected_with_template_missing() {
    let h = ColonyHandle::new_with_echo();

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "ghost_cell", "template": "ghost"}]
            }
        }),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "template_missing",
                "expected template_missing, got {error_code}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

/// Test 2: the template exists in the registry, but the cell.type from its
/// config.json has no registered CellFactory → error_code "unknown_cell_type".
///
/// Setup:
///   - A templates directory with `mystery/template.json` + `mystery/config.json`
///     (cell.type = "totally_unknown_type", which has no factory).
///   - RescanTemplates loads the template into the registry.
///   - A mutation add_nodes with template="mystery".
/// Expectation: Rejected { error_code: "unknown_cell_type" }.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_with_unregistered_cell_type_is_rejected_with_unknown_cell_type() {
    let h = ColonyHandle::new_with_echo();
    let root = h.tempdir_path();
    let templates_root = root.join("templates");

    // Template anlegen: name="mystery", cell.type="totally_unknown_type"
    let tpl_dir = templates_root.join("mystery");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"mystery"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"totally_unknown_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    rescan_templates(&h, templates_root).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "mystery_cell", "template": "mystery"}]
            }
        }),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "unknown_cell_type",
                "expected unknown_cell_type, got {error_code}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}
