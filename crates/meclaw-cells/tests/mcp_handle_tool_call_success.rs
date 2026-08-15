//! T17: handle(tool_call) success path — a synchronous POST against a mock MCP server,
//! tool_result-Turn via OutputSink emittiert.
//!
//! Plan-Adaption T17: Plan-Text las `name`/`arguments` als Top-Level-Felder am
//! tool_call-Turn (UBF-invalid: TurnObject.additionalProperties=false). Adaptiert
//! on the phase-9 store convention (authoritative): `text` = a JSON string with
//! `{"name":"...","arguments":{...}}`; `id` = the UBF-mandatory field for tool_call.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use mock_mcp::{MockMcp, canned_tools_call_text};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_tool_call_success_emits_tool_result() {
    let server = MockMcp::start(vec![canned_tools_call_text("hi back")]).await;

    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let mut cell = McpCell::new(client, 5_000, 5_000, "main_mcp".into());

    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let inner = json!({"name": "echo", "arguments": {"text": "hi"}}).to_string();
    let msg = MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": inner, "id": "call_1"}
            ]
        })))
        .build();
    let sink = OutputSink::new(
        tx,
        Path::new("/mcp"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    );
    let (rc_tx, _) = mpsc::channel(1);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = rx.recv().await.expect("emission");
    assert_eq!(em.target.as_str(), "/sink");
    meclaw_core::validate_ubf_body(&em.content).expect("UBF-valid");
    let header = &em.content["header"];
    assert_eq!(header["mcp_tool"], "echo");
    assert!(header["duration_ms"].as_u64().is_some());
    assert!(header.get("error_code").is_none());
    let last = em.content["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["type"], "tool_result");
    assert_eq!(last["origin"], "tool");
    assert_eq!(last["id"], "call_1");
    assert!(last["text"].as_str().unwrap().contains("hi back"));
}
