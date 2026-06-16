//! T4: mcp_discovery_cache schema + load + upsert.

use meclaw_cells::mcp::db::{
    DiscoveredTool, load_discovery_cache, setup_mcp_schema, upsert_discovery_tools,
};
use rusqlite::Connection;
use serde_json::json;

fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    setup_mcp_schema(&conn).unwrap();
    conn
}

#[test]
fn schema_setup_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    setup_mcp_schema(&conn).unwrap();
    setup_mcp_schema(&conn).unwrap(); // second call must not error
}

#[test]
fn empty_cache_load_returns_empty_vec() {
    let conn = fresh_conn();
    let tools = load_discovery_cache(&conn).unwrap();
    assert!(tools.is_empty());
}

#[test]
fn upsert_then_load_round_trip() {
    let conn = fresh_conn();
    let echo_schema = json!({"type":"object","properties":{"text":{"type":"string"}}});
    upsert_discovery_tools(
        &conn,
        &[DiscoveredTool {
            name: "echo".to_string(),
            schema_json: echo_schema.to_string(),
        }],
        "2026-05-25T10:00:00Z",
    )
    .unwrap();
    let loaded = load_discovery_cache(&conn).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "echo");
    let parsed: serde_json::Value = serde_json::from_str(&loaded[0].schema_json).unwrap();
    assert_eq!(parsed["properties"]["text"]["type"], "string");
}

#[test]
fn upsert_overwrites_existing_tool() {
    let conn = fresh_conn();
    upsert_discovery_tools(
        &conn,
        &[DiscoveredTool {
            name: "echo".to_string(),
            schema_json: "{\"old\":true}".to_string(),
        }],
        "2026-05-25T10:00:00Z",
    )
    .unwrap();
    upsert_discovery_tools(
        &conn,
        &[DiscoveredTool {
            name: "echo".to_string(),
            schema_json: "{\"new\":true}".to_string(),
        }],
        "2026-05-25T10:01:00Z",
    )
    .unwrap();
    let loaded = load_discovery_cache(&conn).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].schema_json, "{\"new\":true}");
}
