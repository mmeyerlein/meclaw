//! Hardening Slice 4 (Task 4.2): Presence-Enforcement der Pflicht-Keys
//! `contract.version`/`settings`/`consumes` auf dem Mutations-Pfad
//! (config.md § contract, Enforcement-Stufen).
//!
//! (1) `add_nodes` mit einem Template, dessen `contract`-Block `settings`
//!     NICHT deklariert, wird pre-destruktiv rejected —
//!     `error_code == "contract_incomplete"` ist Builder-Feedback-Vertrag.
//! (2) Ein Template mit allen drei Pflicht-Keys wird Committed.
//!
//! Muster: phase_11_contract_via_mutation.rs (ColonyHandle + RescanTemplates
//! + add_nodes, kein Filesystem-Bootstrap nötig).

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

/// Template-Verzeichnis mit gegebenem `config.json`-Body anlegen.
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

/// Mutation senden + Outcome über den ack-oneshot lesen
/// (Muster: hardening_header_locality.rs).
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
        .expect("ack sender not dropped");
}

/// (1) Template ohne `contract.settings` → add_nodes wird pre-destruktiv
/// rejected, `error_code == "contract_incomplete"`; die Details nennen den
/// fehlenden Key (Builder-Feedback).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_with_template_missing_settings_is_rejected_contract_incomplete() {
    let td = TempDir::new().unwrap();
    write_template(
        &td,
        "no_settings",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/dev/null"},
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

/// (2) Gutfall: Template mit allen drei Pflicht-Keys → Committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_with_full_contract_presence_is_committed() {
    let td = TempDir::new().unwrap();
    write_template(
        &td,
        "full_contract",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/dev/null"},
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
