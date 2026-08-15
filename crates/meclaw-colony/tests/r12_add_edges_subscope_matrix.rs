//! R12 — `add_edges` depth-endpoint resolution into sub-scopes (ruling
//! option A, 2026-06-11; empirical matrix: templates/llm-unit/RECEIPT.md).
//!
//! Spec Z.227 declares edge `from`/`to` as paths RELATIVE TO THE HIVE SCOPE —
//! without a depth restriction. This matrix pins all four forms at the
//! `handle_mutation` level:
//!
//! - Form A: ONE mutation `add_nodes(subtree)` + `add_edges` with depth
//!   endpoints (`./m1/inner_a`) → committed, subtree ACTIVE (no island).
//! - Form B: TWO mutations — depth endpoints against registry-EXISTING nodes
//!   → committed, subtree active.
//! - Form C: inside-out edge (`../sink` from a sub-scope) → still
//!   `scope_out_of_bounds` (containment NOT relaxed: the parent wires DOWN
//!   into its own subtree, never out).
//! - Form D: hive-path edge to the subtree root (level 1) → still committed
//!   (unchanged activation mechanics).
//! - Negative: depth path to a NON-existent node → `edge_schema`.

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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
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
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Build a SUBTREE template under `<root>/templates/sub`: root hive (internal
/// edge `./inner_a -> ./inner_b`) + two echo cells — the minimal unit shape
/// (dispatch/collector analog) of the llm-unit port convention.
fn write_subtree_template(root: &std::path::Path) {
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

/// Open the colony.db read-only and return whether an edge `from -> to` exists.
fn edge_persisted(db_dir: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Boot a colony with anchor + sink live cells and the subtree template.
async fn setup() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_subtree_template(td.path());
    let factory: std::sync::Arc<dyn meclaw_colony::CellFactory> =
        std::sync::Arc::new(meclaw_testing::factories::EchoCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(meclaw_core::Path::new("/anchor"), || {
        EchoMockCell::new(meclaw_core::Path::new("/anchor"))
    })
    .await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;
    (td, h)
}

/// Form A: ONE mutation wires the unit's ports in the SAME diff that
/// instantiates the subtree — entry INTO `./m1/inner_a`, exit FROM
/// `./m1/inner_b`. Committed + whole subtree active (no island status).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn form_a_one_mutation_subtree_plus_depth_port_edges_commits_and_activates() {
    let (td, h) = setup().await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name":"m1","template":"sub"}],
                "add_edges": [
                    {"from":"./anchor","to":"./m1/inner_a"},
                    {"from":"./m1/inner_b","to":"./sink"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Form A (one mutation, depth port edges) must commit; got {outcome:?}"
    );

    let a = registry_entry(&h, "/m1/inner_a").await.unwrap();
    let b = registry_entry(&h, "/m1/inner_b").await.unwrap();
    assert!(a.active, "/m1/inner_a must be ACTIVE (entry port wired)");
    assert!(b.active, "/m1/inner_b must be ACTIVE (no island status)");

    let db = td.path();
    assert!(edge_persisted(db, "/anchor", "/m1/inner_a"));
    assert!(edge_persisted(db, "/m1/inner_b", "/sink"));
    assert!(edge_persisted(db, "/m1/inner_a", "/m1/inner_b"));

    h.shutdown().await;
}

/// Form B: depth endpoints against nodes that already EXIST in the registry
/// (subtree instantiated by an earlier mutation) — committed + active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn form_b_two_mutations_depth_endpoints_against_existing_registry() {
    let (td, h) = setup().await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert!(!registry_entry(&h, "/m1/inner_a").await.unwrap().active);

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_edges": [
                {"from":"./anchor","to":"./m1/inner_a"},
                {"from":"./m1/inner_b","to":"./sink"}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Form B (depth endpoints, registry-existing) must commit; got {outcome:?}"
    );

    let a = registry_entry(&h, "/m1/inner_a").await.unwrap();
    let b = registry_entry(&h, "/m1/inner_b").await.unwrap();
    assert!(a.active, "/m1/inner_a must be active after depth reconnect");
    assert!(b.active, "/m1/inner_b must be active after depth reconnect");

    let db = td.path();
    assert!(edge_persisted(db, "/anchor", "/m1/inner_a"));
    assert!(edge_persisted(db, "/m1/inner_b", "/sink"));

    h.shutdown().await;
}

/// Form C (unchanged): an inside-out edge from a sub-scope (`../sink`) stays
/// `scope_out_of_bounds` — authority wires DOWN into its own subtree, never out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn form_c_inside_out_edge_stays_scope_out_of_bounds() {
    let (_td, h) = setup().await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/m1",
            "diff": {"add_edges": [{"from":"./inner_a","to":"../sink"}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "scope_out_of_bounds");
        }
        other => panic!("Form C must stay rejected; got {other:?}"),
    }

    h.shutdown().await;
}

/// Form D (unchanged): a hive-path edge to the subtree ROOT (level 1) commits
/// and activates the subtree — the pre-R12 mechanics keep working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn form_d_hive_path_edge_stays_committed() {
    let (td, h) = setup().await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name":"m1","template":"sub"}],
                "add_edges": [{"from":"./anchor","to":"./m1"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Form D (hive-path edge) must stay committed; got {outcome:?}"
    );
    assert!(registry_entry(&h, "/m1/inner_a").await.unwrap().active);
    assert!(edge_persisted(td.path(), "/anchor", "/m1"));

    h.shutdown().await;
}

/// Negative: a depth path to a NON-existent node rejects as `edge_schema`
/// (post_state membership miss; the path is scope-contained).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn depth_endpoint_to_nonexistent_node_rejects_edge_schema() {
    let (_td, h) = setup().await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name":"m1","template":"sub"}],
                "add_edges": [{"from":"./m1/nonexistent","to":"./sink"}]
            }
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "edge_schema");
        }
        other => panic!("depth path to unknown node must reject; got {other:?}"),
    }

    h.shutdown().await;
}
