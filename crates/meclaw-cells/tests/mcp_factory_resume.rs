//! T21: McpCellFactory::spawn_cell — initial + Resume (OpenStatus::Resumed).
//! Seeds a tool into the cache before spawn; after the cell runs its I/O
//! sub-task (tools/list) and terminates, both the seeded tool and the
//! discovered "echo" tool must be present.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::db::{
    DiscoveredTool, load_discovery_cache, setup_mcp_schema, upsert_discovery_tools,
};
use meclaw_cells::mcp::factory::McpCellFactory;
use meclaw_colony::CellFactory;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_core::{CellEmission, Path};
use mock_mcp::{MockMcp, canned_initialize, canned_tools_list};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_then_drop_then_respawn_preserves_cache() {
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![json!({"name":"echo","inputSchema":{"type":"object"}})]),
    ])
    .await;

    let tmp = TempDir::new().unwrap();
    let cell_dir = tmp.path().to_path_buf();

    // Seed an extra tool into the cache so we can verify Resume keeps it.
    let (conn0, _) = open_or_create_cell_db_with_status(&cell_dir.join("cell.db")).unwrap();
    setup_mcp_schema(&conn0).unwrap();
    upsert_discovery_tools(
        &conn0,
        &[DiscoveredTool {
            name: "seeded".into(),
            schema_json: "{}".into(),
        }],
        "2026-05-25T00:00:00Z",
    )
    .unwrap();
    drop(conn0);

    // Spawn — must reuse the existing cell.db (Resumed).
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let factory: Arc<McpCellFactory> = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({"endpoint": server.endpoint()}),
            out_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("spawn_cell");

    // Give the I/O sub-task a moment to fetch tools/list and upsert.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let (sender, join) = match spawned {
        meclaw_colony::SpawnedCellKind::Active { sender, join, .. } => (sender, join),
        meclaw_colony::SpawnedCellKind::Dormant { .. } => unreachable!(),
    };
    // Drop the sender → cell terminates (mailbox closes).
    drop(sender);
    let _ = join.await;

    // Fresh-Connection-Probe: both seeded + echo (from tools/list) must
    // be in the cache.
    let conn1 = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    let cache = load_discovery_cache(&conn1).unwrap();
    let names: Vec<_> = cache.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"seeded"), "seeded tool gone: {names:?}");
    assert!(
        names.contains(&"echo"),
        "echo tool not discovered: {names:?}"
    );
}
