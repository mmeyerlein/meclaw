//! Phase-13.5 slice 4 T3 (F3): FS path truth — bootstrap and mutation must
//! agree on the `logical_path → fs_path` mapping.
//!
//! Spec § Authority "Instantiation and cell_id stability" (overview Z.170-180):
//! instantiation happens **iff** no cell-directory exists at the target path
//! (fresh cell_id). If the directory exists → Reconnect/Resume: NO new cell_id,
//! `cell.db` resumed (M1). `cell_id` is assigned exactly once for a path.
//!
//! The F3 divergence: bootstrap registers cells under their real walk path
//! (`{root}/main/<name>`, because the single root cell dir `main` maps to `/`).
//! Mutation's `add_nodes` Resume-detect computes the target fs path as
//! `root.join(scope_clean).join(name)` = `{root}/<name>` — the `main/` segment
//! is missing. So a Resume `add_nodes` at a bootstrap-instantiated path does NOT
//! see the existing directory, the node is NOT recognised as a Resume, and the
//! existing registry name collides (or a fresh cell_id is minted) instead of the
//! `cell.db` continuity Resume the spec mandates.
//!
//! The pre-existing 3a Resume demos sidestep this by instantiating the cell via
//! mutation first (landing at `{root}/<name>`, prefix-less) so both sides agree.
//! This test deliberately uses a **bootstrap-instantiated** cell under the real
//! `{root}/main/<name>` layout to exercise the divergence.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

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

async fn read_entry(
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
    let reply = ack_rx.await.unwrap();
    reply.entries.into_iter().find(|e| e.path == path)
}

/// Read the persisted `counter` slot from a stateful cell's `cell.db`.
fn db_counter(cell_dir: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM system WHERE slot_path='counter'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<i64>().ok())
}

/// Bootstrap layout: a root hive at `main/` and a stateful `persist_mock` cell
/// at `main/q` (meclaw path `/q`, real fs path `{root}/main/q`). A
/// `persist_mock` template named identically so a `add_nodes` Resume can target
/// the same logical path.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    // Bootstrap-instantiated stateful cell at main/q (logical /q).
    std::fs::create_dir_all(td.join("main/q")).unwrap();
    std::fs::write(
        td.join("main/q/config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // Template `persist_mock` so the Resume `add_nodes` can reference it.
    let tpl_dir = td.join("templates").join("persist_mock");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// F3 repro: a bootstrap-instantiated stateful cell at `{root}/main/q` (logical
/// `/q`) with a populated `cell.db`. An `add_nodes(scope="/", name="q")` must be
/// recognised as a Reconnect/Resume — `cell_id` unchanged, `cell.db` continuity
/// (the earlier-written counter survives), NOT a fresh re-instantiation nor a
/// naming-collision rejection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_at_bootstrap_instantiated_path_resumes() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory)],
    );

    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "persist_mock".into(),
        Arc::new(PersistCellFactory {
            spawn_count: spawn_count.clone(),
        }) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // /q is bootstrap-registered, NotYetSpawned, with a stable cell_id.
    let before = read_entry(&h, "/q").await.expect("/q must be registered");
    assert_eq!(
        before.lifecycle_status, "NotYetSpawned",
        "bootstrap stateful cell must be NotYetSpawned"
    );
    let cell_id_before = before.cell_id.clone();

    // Simulate a cell that has already run: write a populated cell.db counter.
    // The real fs path of the bootstrap cell is `{root}/main/q`.
    let cell_dir = td.path().join("main/q");
    {
        let conn =
            meclaw_colony::persist::cell_db::open_or_create_cell_db(&cell_dir.join("cell.db"))
                .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO system (slot_path, value, updated_at) VALUES ('counter','7',0)",
            [],
        )
        .unwrap();
    }
    assert_eq!(
        db_counter(&cell_dir),
        Some(7),
        "precondition: cell.db counter seeded to 7"
    );

    // add_nodes at the EXISTING logical /q → must be a Reconnect/Resume.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Resume at a bootstrap-instantiated path must be Committed (NOT a fresh \
         re-instantiation / naming_collision); got {outcome:?}"
    );

    // cell_id unchanged (no re-mint — cell_id is assigned exactly once per path).
    let after = read_entry(&h, "/q")
        .await
        .expect("/q must still be registered after resume");
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be unchanged on Resume (assigned exactly once)"
    );

    // cell.db continuity: the earlier-written counter row is still present.
    assert_eq!(
        db_counter(&cell_dir),
        Some(7),
        "cell.db counter must survive Resume (continuity, no reseed)"
    );

    h.shutdown().await;
}
