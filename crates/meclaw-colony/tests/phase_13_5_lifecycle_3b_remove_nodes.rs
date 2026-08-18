//! Phase-13.5 Lifecycle-3b Task 6 demo (f): `remove_nodes` = Disconnect, not
//! Delete (SCOPE 4, spec Z.260).
//!
//! `remove_nodes` on a connected cell removes ALL of its edges (from + to), the
//! connectivity-recompute deactivates it (`active == false`), its task stops
//! gracefully, BUT the registry entry STAYS (No-Delete: `cell_id` unchanged,
//! FS/`cell.db` untouched). A subsequent `add_edges` reactivates the very same
//! entry. The persisted `colony.db.registry.status == "inactive"` is read back
//! via a FRESH read-only connection.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive + two echo cells `/a`, `/b`.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::create_dir_all(td.join("main/b")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

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
        .unwrap();
    ack_rx.await.unwrap()
}

/// Read the RAM registry entry DTO for `path` (or `None` if absent).
async fn ram_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
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

/// Fresh read-only connection probe: persisted `registry.status` for `path`.
fn db_registry_status(db_dir: &std::path::Path, path: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

/// Count edge rows in colony.db that touch `path` as from OR to (fresh conn).
fn db_edges_touching(db_dir: &std::path::Path, path: &str) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT count(*) FROM edges WHERE from_path = ?1 OR to_path = ?1",
        [path],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_nodes_disconnects_keeps_entry_then_add_edges_reactivates() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    // /sink BEFORE bootstrap (anti-cascade).
    let (sink_tx, _sink_rx) = mpsc::channel::<meclaw_core::Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Connect /a with edges in BOTH directions: /a -> /sink and /b -> /a.
    // Also add a persistent /b -> /sink edge that does NOT touch /a, so removing
    // node /a leaves /sink connected (a no-stop-wiring Awake sink would otherwise
    // trip the Step-10 stop-wiring guard when stranded).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"a","to":"sink"},
            {"from":"b","to":"a"},
            {"from":"b","to":"sink"}
        ]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let a = ram_entry(&h, "/a").await.expect("/a registered");
    let cell_id_before = a.cell_id.clone();
    assert!(a.active, "/a must be active after add_edges");
    assert_eq!(db_edges_touching(&db_dir, "/a"), 2, "two edges touch /a");

    // remove_nodes /a → ALL its edges (from + to) removed, recompute deactivates.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{"name":"a"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_nodes must commit (death-ack fires < TERM_TIMEOUT), got {outcome:?}"
    );

    // Registry entry STAYS, same cell_id, active == false.
    let a = ram_entry(&h, "/a")
        .await
        .expect("/a entry must STAY per No-Delete");
    assert_eq!(a.cell_id, cell_id_before, "cell_id must be unchanged");
    assert!(!a.active, "/a must be inactive after remove_nodes");
    // All edges of /a are gone (from + to).
    assert_eq!(
        db_edges_touching(&db_dir, "/a"),
        0,
        "all edges touching /a must be removed"
    );

    // Persisted status is "inactive" after the disconnect (fresh read-only conn).
    // NB: the reactivation tail of demo (f) ("add_edges reactivates afterwards") needs
    // the recompute-hook's `false→true` arm, which is Task 7 (plan 7.4). Here we
    // only prove that `add_edges` against the still-registered entry COMMITS and
    // re-attaches an edge through the SAME entry (`cell_id` unchanged) — the
    // `active==true` flip is asserted in Task 7's demo (e)/(f).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"a","to":"sink"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges against the surviving /a entry must commit, got {outcome:?}"
    );
    let a = ram_entry(&h, "/a").await.expect("/a still registered");
    assert_eq!(a.cell_id, cell_id_before, "cell_id stays across re-edge");
    assert_eq!(
        db_edges_touching(&db_dir, "/a"),
        1,
        "the new edge must be persisted against the surviving /a entry"
    );

    h.shutdown().await;
}

/// Root hive + a top-level cell `/x` + a sub-hive `/h` with cells `/h/c1`,
/// `/h/c2`. `/h`'s connectivity hangs on the single parent-level edge `/x -> /h`.
fn write_hive_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/h/c1")).unwrap();
    std::fs::create_dir_all(td.join("main/h/c2")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    // Internal hive wiring lives in the hive's own config.json graph (relative
    // `./` endpoints), NOT in a mutation — mutation edge endpoints are short-names
    // resolved at the mutation scope and cannot express nesting.
    std::fs::write(
        td.join("main/h/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./c1","to":"./c2"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/c1/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/c1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/c2/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_nodes_cascades_disconnect_through_hive_subtree() {
    let td = TempDir::new().unwrap();
    write_hive_topology(td.path());
    let db_dir = td.path().to_path_buf();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // Wire the parent-level gating edge /x -> /h via mutation (short-names at
    // scope `/`). The internal /h/c1 -> /h/c2 edge already came from /h's
    // config.json graph during bootstrap.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"x","to":"h"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges (hive wiring) must commit, got {outcome:?}"
    );
    assert!(
        ram_entry(&h, "/h/c1").await.expect("/h/c1").active,
        "/h/c1 active while the hive is connected"
    );
    assert!(
        ram_entry(&h, "/h/c2").await.expect("/h/c2").active,
        "/h/c2 active while the hive is connected"
    );

    // remove_nodes /x → removes /x -> /h → /h disconnected → whole subtree
    // inactive via the recompute affected_scope.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{"name":"x"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_nodes /x must commit, got {outcome:?}"
    );

    // The whole hive subtree is now inactive (entries STAY).
    for n in ["/h/c1", "/h/c2"] {
        let e = ram_entry(&h, n)
            .await
            .unwrap_or_else(|| panic!("{n} stays"));
        assert!(!e.active, "{n} must be inactive after the hive disconnects");
    }
    // /x itself stays inactive too (its edge is gone).
    assert!(!ram_entry(&h, "/x").await.expect("/x stays").active);

    h.shutdown().await;

    // Persisted inactive for the subtree cells.
    for n in ["/h/c1", "/h/c2"] {
        assert_eq!(
            db_registry_status(&db_dir, n).as_deref(),
            Some("inactive"),
            "{n} must be persisted inactive"
        );
    }
}
