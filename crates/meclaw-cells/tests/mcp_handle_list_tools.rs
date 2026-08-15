//! T19: handle(tool_call with __list_tools__) → reads discovery cache, emits
//! system.tools.<provider>.<tool>=<schema> via OutputSink an `reply_to`.
//!
//! Plan adaptation T19: the tool_call body is UBF-conformant (`text` = a JSON
//! string with `{"name":"__list_tools__"}`, `id` mandatory). McpClient points at
//! `http://x.invalid` — the `__list_tools__` branch does NOT call `call_tool`.
//! The cache is planted directly into cell.db via `upsert_discovery_tools`.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::{DiscoveredTool, setup_mcp_schema, upsert_discovery_tools};
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_list_tools_emits_system_tools_from_cache() {
    // 1. Plant tools into a fresh cell.db via `upsert_discovery_tools`.
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("cell.db");

    {
        let (conn, _) = open_or_create_cell_db_with_status(&db_path).unwrap();
        setup_mcp_schema(&conn).unwrap();
        let tools = vec![
            DiscoveredTool {
                name: "echo".to_string(),
                schema_json: json!({"type": "object", "properties": {"x": {"type": "string"}}})
                    .to_string(),
            },
            DiscoveredTool {
                name: "add".to_string(),
                schema_json: json!({"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}})
                    .to_string(),
            },
        ];
        upsert_discovery_tools(&conn, &tools, "2026-05-24T10:00:00Z").unwrap();
        // conn dropped here — close connection before re-opening
    }

    // 2. Re-open the DB via `open_or_create_cell_db_with_status` (fresh connection).
    let (conn, _) = open_or_create_cell_db_with_status(&db_path).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    // 3. McpClient points at http://x.invalid — no call_tool is triggered.
    let client = McpClient::new("http://x.invalid", None).unwrap();
    let mut cell = McpCell::new(client, 5_000, 5_000, "main_mcp".into());

    // 4. Build a UBF-conformant tool_call message with `__list_tools__` + `reply_to=/sink`.
    let inner = json!({"name": "__list_tools__"}).to_string();
    let msg = MessageBuilder::new(Path::new("/main/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": inner, "id": "call_lt_1"}
            ]
        })))
        .build();

    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/main/mcp"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    );
    let (rc_tx, _) = mpsc::channel(1);

    // 5. handle() — must take the __list_tools__ branch, NOT call call_tool.
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    // 6. Verifiziere Emission.
    let em = rx
        .recv()
        .await
        .expect("emission expected from __list_tools__ branch");

    // target == /sink
    assert_eq!(em.target.as_str(), "/sink", "target must be /sink");

    // UBF validator green
    meclaw_core::validate_ubf_body(&em.content).expect("system.tools emission must be valid UBF");

    // system.tools.main_mcp.echo and ...add must come from the cache
    assert_eq!(
        em.content["system"]["tools"]["main_mcp"]["echo"]["properties"]["x"]["type"], "string",
        "echo schema not found in emission"
    );
    assert!(
        em.content["system"]["tools"]["main_mcp"]["add"]["properties"]["a"]["type"]
            .as_str()
            .is_some(),
        "add schema not found in emission"
    );

    // header.mcp_tool == "__list_tools__"
    assert_eq!(
        em.content["header"]["mcp_tool"], "__list_tools__",
        "header.mcp_tool must be __list_tools__"
    );

    // duration_ms vorhanden (u64)
    assert!(
        em.content["header"]["duration_ms"].as_u64().is_some(),
        "header.duration_ms must be present"
    );
}
