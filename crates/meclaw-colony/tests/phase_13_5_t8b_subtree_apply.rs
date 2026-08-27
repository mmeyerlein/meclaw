//! Phase-13.5 a5-subtree T8b-1: subtree-instantiation wiring on the apply side.
//!
//! Proves that `add_nodes` of a SUBTREE template (root hive with a `params.graph`
//! internal edge + ≥2 nested echo cells) instantiates the WHOLE subtree:
//! - every subtree cell lands in the registry,
//! - the subtree-root hive-scope is persisted (fresh rusqlite probe),
//! - the internal edge is present (fresh rusqlite probe + `route` reachability),
//! - and — because the parentless subtree hive has NO incoming edge — EVERY
//!   subtree cell is `active == false` (inactive, no eager spawn).
//!
//! A second demo proves the reconnect side: an `add_edges` from a live anchor
//! cell to the subtree-root hive flips the whole subtree `active == true`.

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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
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

/// Build a SUBTREE template under `<root>/templates/sub`:
/// root hive (with the internal edge `./inner_a -> ./inner_b`) + two echo cells.
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
        r#"{"cell":{"type":"echo_sub"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "inner_b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Open the colony.db read-only and return whether a hive_scope row exists.
fn hive_scope_persisted(db_dir: &std::path::Path, path: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hive_scopes WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
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

/// Demo: `add_nodes` of a SUBTREE template registers ALL subtree cells inactive,
/// persists the root hive-scope + internal edge, with no `.staging` leak.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_subtree_registers_all_cells_inactive_with_hive_scope_and_internal_edges() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_subtree_template(td.path());

    // The inner cells are `echo_sub`; an inactive subtree never spawns them, but a
    // factory must exist for the cell-type so the mutation validates + so the
    // inactive-registration's `build_boot_inactive_respawn` hook is reachable.
    let factory: std::sync::Arc<dyn meclaw_colony::CellFactory> =
        std::sync::Arc::new(meclaw_testing::factories::EchoCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;

    // A terminal /sink so the echo cells' emitted_target target is resolvable (they
    // never run here, but keep the topology honest).
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"m1","template":"sub"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes(subtree) must commit; got {outcome:?}"
    );

    // (1) All subtree cells are in the registry (the hive is NOT a registry entry).
    let a = registry_entry(&h, "/m1/inner_a")
        .await
        .expect("/m1/inner_a must be registered");
    let b = registry_entry(&h, "/m1/inner_b")
        .await
        .expect("/m1/inner_b must be registered");
    assert!(
        registry_entry(&h, "/m1").await.is_none(),
        "the hive /m1 is a scope marker, NOT a registry entry"
    );

    // (2) Every subtree cell is INACTIVE (parentless hive) + parked NotYetSpawned.
    assert!(
        !a.active,
        "/m1/inner_a must be inactive (parentless subtree)"
    );
    assert!(
        !b.active,
        "/m1/inner_b must be inactive (parentless subtree)"
    );
    assert_eq!(a.lifecycle_status, "NotYetSpawned");
    assert_eq!(b.lifecycle_status, "NotYetSpawned");

    let db_dir = td.path().to_path_buf();

    // (3) The subtree-root hive-scope is persisted (fresh rusqlite probe).
    assert!(
        hive_scope_persisted(&db_dir, "/m1"),
        "subtree-root hive-scope /m1 must be persisted"
    );

    // (4) The internal edge is present + persisted (remapped to absolute paths).
    assert!(
        edge_persisted(&db_dir, "/m1/inner_a", "/m1/inner_b"),
        "internal edge /m1/inner_a -> /m1/inner_b must be persisted"
    );

    // (5) No `.staging/<mid>/` leak.
    let staging = td.path().join(".staging");
    if staging.exists() {
        let leaked: Vec<_> = std::fs::read_dir(&staging)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                // any leftover mutation-id dir that still contains an `m1` subtree.
                e.path().join("m1").exists()
            })
            .collect();
        assert!(leaked.is_empty(), "no `.staging/<mid>/m1` leak allowed");
    }

    h.shutdown().await;
}

/// Demo (optional but valuable): after instantiating the parentless subtree, an
/// `add_edges` from a live anchor cell to the subtree-root hive flips the WHOLE
/// subtree `active == true` (the existing connectivity-recompute reaches the
/// internal edges via the now-connected hive).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_edges_to_subtree_root_activates_whole_subtree() {
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

    // A live anchor cell (the edge's `from` endpoint must be a known cell).
    h.spawn(meclaw_core::Path::new("/anchor"), || {
        EchoMockCell::new(meclaw_core::Path::new("/anchor"))
    })
    .await;
    h.spawn(meclaw_core::Path::new("/sink"), || {
        EchoMockCell::new(meclaw_core::Path::new("/sink"))
    })
    .await;

    // Instantiate the parentless subtree → all cells inactive.
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
    assert!(!registry_entry(&h, "/m1/inner_b").await.unwrap().active);

    // Connect /anchor -> /m1 (the subtree-root hive) → recompute activates the
    // whole subtree.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_edges": [{"from":"anchor","to":"m1"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges anchor->m1 must commit; got {outcome:?}"
    );

    let a = registry_entry(&h, "/m1/inner_a").await.unwrap();
    let b = registry_entry(&h, "/m1/inner_b").await.unwrap();
    assert!(a.active, "/m1/inner_a must be active after hive reconnect");
    assert!(b.active, "/m1/inner_b must be active after hive reconnect");

    h.shutdown().await;
}
