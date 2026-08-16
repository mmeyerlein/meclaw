//! What R10 protected, after GH #140 opened the surface it had closed.
//!
//! The original finding (F2-Ruling, 2026-06-11): every addressing form of
//! `override_params` on a subtree template COMMITTED and applied nothing —
//! `stage.rs` dispatched subtree templates to `stage_subtree_merge` without
//! ever reading the field. Commit-without-effect is a false-accept surface: a
//! builder believes the override took. The ruling was to reject the field
//! outright.
//!
//! #140 removes the cause instead of the feature: `override_params` on a
//! subtree template is now ADDRESSED by the cells' paths inside the template.
//! What stays exactly as R10 left it is the property this file guards — **a
//! key that addresses nothing is refused, pre-destructively**. The flat form
//! below is precisely such a key (`external_timeout_ms` is a params name, not
//! a cell path), so the mutation this file has always sent is still rejected,
//! for a reason that is now specific rather than categorical.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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

async fn registry_has(h: &ColonyHandle, path: &str) -> bool {
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
    ack_rx.await.unwrap().entries.iter().any(|e| e.path == path)
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "persist_mock".to_string(),
        Arc::new(PersistCellFactory {
            spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }) as Arc<dyn CellFactory>,
    )]
}

/// SUBTREE template `unit` (root hive + two nested cells) and SINGLE-CELL
/// template `solo` (negative probe — override_params stays legal there).
fn write_templates(root: &std::path::Path) {
    let unit = root.join("templates").join("unit");
    write(&unit, "template.json", r#"{"name":"unit"}"#);
    write(
        &unit,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./s","to":"./e"}]}}}"#,
    );
    write(
        &unit,
        "s/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/u1/e"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &unit,
        "e/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let solo = root.join("templates").join("solo");
    write(&solo, "template.json", r#"{"name":"solo"}"#);
    write(
        &solo,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A FLAT `override_params` on a SUBTREE template addresses no cell, so it
/// still rejects `schema` — pre-destructively (nothing staged, nothing
/// registered, no directory under {root}).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn override_params_on_subtree_template_rejects_schema() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_templates(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit","override_params":{"external_timeout_ms":12345}}
        ]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "schema",
                "override_params on a subtree template must reject schema (F2/R10)"
            );
            assert!(
                details.contains("names no cell of the subtree template"),
                "the reject must say WHICH key addressed nothing; got: {details}"
            );
            assert!(
                details.contains("Its cells are:"),
                "and it must list what the template does contain, so the next \
                 attempt is informed rather than guessed; got: {details}"
            );
        }
        other => panic!("expected Rejected, got {other:?} (pre-fix: silent no-op commit)"),
    }

    // Pre-destructive: nothing registered, nothing materialized on disk.
    assert!(
        !registry_has(&h, "/u1/s").await && !registry_has(&h, "/u1/e").await,
        "reject must register nothing"
    );
    assert!(
        !td.path().join("main/u1").exists(),
        "reject must materialize nothing under {{root}}"
    );

    h.shutdown().await;
}

/// Negative probe: `override_params` on a SINGLE-CELL template stays legal
/// (commits) — the reject is subtree-specific.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn override_params_on_single_cell_template_still_commits() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_templates(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"c1","template":"solo","override_params":{"external_timeout_ms":12345}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "single-cell override_params must stay legal; got {outcome:?}"
    );
    assert!(registry_has(&h, "/c1").await, "/c1 must be registered");

    h.shutdown().await;
}
