//! Substrat-Fix Slice 0 (security hotfix) — `add_nodes[].name` traversal.
//!
//! Repro (workshop/fixtures/negative/scope_out_of_bounds, escalated finding): a
//! root-scope mutation with `add_nodes[].name == "../escape"` passed
//! `validate_scope_containment` (the logical normalisation root-clamps
//! `/../escape` to `/escape`, "within /") and COMMITTED — staging + rename
//! realised a directory OUTSIDE `{root}` (`resolve_cell_dir` joins the raw name
//! without clamping; a flat workspace anchors at `{root}` itself, so
//! `{root}/../escape` lands beside the colony root).
//!
//! Spec § Mutation format (overview Z.251): "Mutations whose paths would lie
//! outside the scope are rejected during validation."
//!
//! This test drives the REAL mutation path (`ColonyMsg::Mutation` →
//! `handle_mutation`) against a flat workspace and proves both halves:
//!
//!   * the mutation is `Rejected` with `error_code = "scope_out_of_bounds"`,
//!   * FS proof: NO directory materialises outside `{root}` (and no `escape`
//!     residue inside it either — reject is pre-destructive).

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{JsonValue, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

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

/// Flat colony workspace `td/ws` (no root-cell dir → `resolve_cell_dir`
/// anchors at `{root}` itself, the layout of the throwaway fixture colony the
/// finding was observed in) with one echo template.
fn write_workspace(td: &std::path::Path) -> std::path::PathBuf {
    let ws = td.join("ws");
    let tpl = ws.join("templates/escape-tpl");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"escape-tpl"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    ws
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dotdot_add_node_name_rejects_and_leaves_no_directory_outside_root() {
    let td = tempfile::TempDir::new().unwrap();
    let ws = write_workspace(td.path());
    let h = ColonyHandle::new_with_echo_at(&ws);
    rescan_templates(&h, ws.join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "../escape", "template": "escape-tpl"}]}
        }),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "scope_out_of_bounds",
            "`..`-name must reject as scope_out_of_bounds, got {error_code}"
        ),
        other => panic!("expected Rejected{{scope_out_of_bounds}}, got {other:?}"),
    }

    // FS proof: nothing escaped the colony root, and the reject left no
    // realised `escape` directory inside it either.
    assert!(
        !td.path().join("escape").exists(),
        "directory was created OUTSIDE {{root}} — confinement breach"
    );
    assert!(
        !ws.join("escape").exists(),
        "reject must not leave a realised `escape` directory inside {{root}}"
    );

    h.shutdown().await;
}
