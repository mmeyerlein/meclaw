//! Phase-13.5 a5-subtree T8b-2: subtree VALIDATION before staging.
//!
//! Proves that an invalid SUBTREE `add_nodes` is rejected in the validation phase
//! — BEFORE any staging / FS work — so a reject leaves NO `.staging` leak and
//! nothing in the registry / db:
//! - Paket-5 T12 (P9 F4-closure): a subtree at an already-existing root path is
//!   no longer rejected — it is a per-node resume that COMMITS as a no-op;
//! - a subtree whose internal `params.graph` edges form a CYCLE COMMITS —
//!   meclaw-Core does not reject cycles generally (Substrat-Fix Befund 2);
//! - a subtree whose internal edge ESCAPES the subtree root (`../sibling`) is
//!   rejected (schema/containment), nothing staged;
//! - (regression) a valid single-cell `add_nodes` still validates + commits.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

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

async fn all_registry(h: &ColonyHandle) -> Vec<meclaw_colony::api_dto::RegistryEntryDto> {
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
    ack_rx.await.unwrap().entries
}

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    all_registry(h).await.into_iter().find(|e| e.path == path)
}

/// A valid SUBTREE template: root hive (internal edge `./inner_a -> ./inner_b`)
/// + two echo cells.
fn write_valid_subtree_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("sub");
    write(&tpl, "template.json", r#"{"name":"sub"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner_a","to":"./inner_b"}]}}}"#,
    );
    write(
        &tpl,
        "inner_a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "inner_b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A SUBTREE template whose internal edges form a cycle: `./inner_a -> ./inner_b`
/// AND `./inner_b -> ./inner_a`.
fn write_cyclic_subtree_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("sub_cycle");
    write(&tpl, "template.json", r#"{"name":"sub_cycle"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner_a","to":"./inner_b"},{"from":"./inner_b","to":"./inner_a"}]}}}"#,
    );
    write(
        &tpl,
        "inner_a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "inner_b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A SUBTREE template whose internal edge escapes the subtree root via `../`.
fn write_escaping_subtree_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("sub_escape");
    write(&tpl, "template.json", r#"{"name":"sub_escape"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner_a","to":"../sibling"}]}}}"#,
    );
    write(
        &tpl,
        "inner_a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A trivial single-cell template (regression baseline).
fn write_single_cell_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("solo");
    write(&tpl, "template.json", r#"{"name":"solo"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

fn echo_factory() -> std::sync::Arc<dyn meclaw_colony::CellFactory> {
    std::sync::Arc::new(meclaw_testing::factories::EchoCellFactory)
}

/// Assert there is NO leftover `.staging/<mid>/<name>` for the given subtree name.
fn assert_no_staging_leak(root: &std::path::Path, name: &str) {
    let staging = root.join(".staging");
    if !staging.exists() {
        return;
    }
    let leaked: Vec<_> = std::fs::read_dir(&staging)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(name).exists())
        .collect();
    assert!(
        leaked.is_empty(),
        "no `.staging/<mid>/{name}` leak allowed, found: {leaked:?}"
    );
}

/// Paket-5 T12 (P9 F4-closure): instantiating a subtree succeeds once; a SECOND
/// `add_nodes` at the SAME name is no longer the F4 `subtree_resume_unsupported`
/// reject — it is now a per-node resume. Every node already exists (and is not
/// Awake), so the second apply COMMITS as a pure no-op resume (idempotent b1):
/// the existing cells keep their `cell_id` (no re-mint, no new registry entry).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_subtree_at_existing_root_path_resumes_per_node() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_valid_subtree_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    // First instantiation commits.
    let first = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(
        matches!(first, MutationOutcome::Committed { .. }),
        "first add_nodes(subtree) must commit; got {first:?}"
    );
    // Capture the original cell_ids so we can prove identity stability on resume.
    let id_a_before = registry_entry(&h, "/m1/inner_a")
        .await
        .expect("inner_a registered after first apply")
        .cell_id;
    let id_b_before = registry_entry(&h, "/m1/inner_b")
        .await
        .expect("inner_b registered after first apply")
        .cell_id;

    // Second instantiation at the SAME name → per-node resume, COMMITS as a no-op.
    let second = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(
        matches!(second, MutationOutcome::Committed { .. }),
        "second add_nodes(subtree) at existing root must resume (commit), not reject; got {second:?}"
    );

    // Identity preserved: same cell_ids, no duplicate / re-minted entries (F1).
    let id_a_after = registry_entry(&h, "/m1/inner_a")
        .await
        .expect("inner_a still registered after resume")
        .cell_id;
    let id_b_after = registry_entry(&h, "/m1/inner_b")
        .await
        .expect("inner_b still registered after resume")
        .cell_id;
    assert_eq!(id_a_before, id_a_after, "inner_a cell_id must be unchanged");
    assert_eq!(id_b_before, id_b_after, "inner_b cell_id must be unchanged");

    h.shutdown().await;
}

/// Substrat-Fix Befund 2 — a subtree whose internal edges form a cycle
/// (`./inner_a ⇄ ./inner_b`) COMMITS: spec overview § Validierung says
/// meclaw-Core does not reject cycles generally, and the identical shape boots
/// fine from the filesystem. Both nested cells register; the runtime TTL
/// loop-guard bounds the cycle. (Endpoint-existence + containment checks still
/// apply — see the escaping-edge test below.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_subtree_with_internal_cycle_commits() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_cyclic_subtree_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub_cycle"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "cyclic subtree must commit (cycles tolerated, Befund 2); got {outcome:?}"
    );

    // Both nested cells registered; subtree dir realised.
    assert!(
        registry_entry(&h, "/m1/inner_a").await.is_some(),
        "inner_a must be registered after commit"
    );
    assert!(
        registry_entry(&h, "/m1/inner_b").await.is_some(),
        "inner_b must be registered after commit"
    );
    // Anchored under the single root cell dir (`main/`), per spec § Filesystem-
    // Layout (root-cell-dir name stripped from logical paths).
    assert!(
        td.path().join("main/m1").exists(),
        "final subtree dir realised under root cell"
    );
    assert_no_staging_leak(td.path(), "m1");

    h.shutdown().await;
}

/// A subtree whose internal edge escapes the subtree root (`../sibling`) is
/// rejected (schema/containment) before staging — nothing staged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_subtree_with_edge_escaping_root_rejected() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_escaping_subtree_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub_escape"}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "schema", "escaping edge must be rejected");
        }
        other => panic!("escaping-edge subtree must be rejected; got {other:?}"),
    }

    assert!(registry_entry(&h, "/m1/inner_a").await.is_none());
    assert!(!td.path().join("m1").exists(), "no final subtree dir");
    assert_no_staging_leak(td.path(), "m1");

    h.shutdown().await;
}

/// Regression: a valid single-cell `add_nodes` still validates + commits with the
/// subtree aggregates empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_single_cell_still_commits() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_single_cell_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"solo1","template":"solo"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "single-cell add_nodes must commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/solo1").await.is_some(),
        "/solo1 must be registered"
    );

    h.shutdown().await;
}
