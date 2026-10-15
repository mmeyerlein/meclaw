//! T16: handle_event(DiscoveryReady) upserts into mcp_discovery_cache
//! via DbConn::call. Fresh-Connection-Probe verifies persistence.

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::{DiscoveredTool, load_discovery_cache, setup_mcp_schema};
use meclaw_cells::mcp::io::McpEvent;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellEmission, OriginSink, Path};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_discovery_ready_upserts_cache() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("cell.db");
    let (conn, _status) = open_or_create_cell_db_with_status(&db_path).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    let client = McpClient::new("http://x", None).unwrap();
    let mut cell = McpCell::new(client, 30_000, 5_000, "main_mcp".into());

    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(tx, Path::new("/mcp"), 64);

    let event = McpEvent::DiscoveryReady {
        tools: vec![
            DiscoveredTool {
                name: "echo".into(),
                schema_json: serde_json::json!({"type":"object"}).to_string(),
            },
            DiscoveredTool {
                name: "add".into(),
                schema_json: serde_json::json!({"type":"object","x":1}).to_string(),
            },
        ],
    };
    cell.handle_event(event, &origin, &mut db).await;

    // Fresh probe — open a NEW connection to the same DB file.
    let probe = rusqlite::Connection::open(&db_path).unwrap();
    let cached = load_discovery_cache(&probe).unwrap();
    assert_eq!(cached.len(), 2);
    let names: Vec<_> = cached.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"add"));
}
