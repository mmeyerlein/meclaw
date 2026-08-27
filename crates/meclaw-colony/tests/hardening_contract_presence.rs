//! Hardening slice 4 (task 4.2): presence enforcement of the mandatory keys
//! `contract.version`/`settings`/`consumes` on the mutation path
//! (config.md § contract, enforcement levels).
//!
//! (1) `add_nodes` with a template whose `contract` block does NOT declare
//!     `settings` is rejected pre-destructively —
//!     `error_code == "contract_incomplete"` is the builder-feedback contract.
//! (2) A template with all three mandatory keys is committed.
//!
//! Pattern: phase_11_contract_via_mutation.rs (ColonyHandle + RescanTemplates
//! + add_nodes, no filesystem bootstrap needed).

use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

/// Creates a template directory with the given `config.json` body.
fn write_template(td: &TempDir, name: &str, config_body: &str) {
    let tpl_dir = td.path().join("templates").join(name);
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    )
    .unwrap();
    std::fs::write(tpl_dir.join("config.json"), config_body).unwrap();
}

/// Sends a mutation and reads the outcome via the ack oneshot
/// (pattern: hardening_header_locality.rs).
async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack within 30s")
        .expect("ack sender not dropped")
}

/// Templates-Rescan triggern (Muster: phase_11_contract_via_mutation.rs).
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("rescan ack within 30s")
        .expect("ack sender not dropped")
        .expect("GH #440: the rescan must not have aborted");
}

/// (1) A template without `contract.settings` → add_nodes is rejected
/// pre-destructively, `error_code == "contract_incomplete"`; the details name
/// the missing key (builder feedback).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_with_template_missing_settings_is_rejected_contract_incomplete() {
    let td = TempDir::new().unwrap();
    write_template(
        &td,
        "no_settings",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
            "contract":{"version":"0.1.0","consumes":{}}}"#,
    );
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"n1","template":"no_settings@1.0.0"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "contract_incomplete", "details: {details}");
            assert!(
                details.contains("settings"),
                "details must name the missing key for builder feedback, got: {details}"
            );
        }
        other => panic!("expected Rejected(contract_incomplete), got {other:?}"),
    }
    h.shutdown().await;
}

/// (2) Good case: a template with all three mandatory keys → committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_with_full_contract_presence_is_committed() {
    let td = TempDir::new().unwrap();
    write_template(
        &td,
        "full_contract",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"n2","template":"full_contract@1.0.0"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "full presence keys must commit, got {outcome:?}"
    );
    h.shutdown().await;
}
