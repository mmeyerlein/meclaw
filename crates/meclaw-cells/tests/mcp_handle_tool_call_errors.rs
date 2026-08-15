//! T18: handle(tool_call) error paths — provider_timeout + mcp_error.
//!
//! Plan-Adaption T18: Analogous to T17 — tool_call body via `MessageBuilder +
//! Body::Inline`; `text` = JSON-string with `{"name":"...","arguments":{...}}`;
//! `id` = UBF-required for tool_call. Uses `em.content[...]` (not `em.output.body()`).
//! MockMcp (canned-response sequence API, not handler-closure API).

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use meclaw_testing::mock_http::MockResponse;
use mock_mcp::{MockMcp, canned_rpc_error};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Build a UBF-valid tool_call message targeting `target_path`, with `reply_to = /sink`.
/// `text` = JSON-string with `{"name":"...","arguments":{}}`, `id` = "call_1".
fn build_tool_call_msg(target: &str) -> meclaw_core::Message {
    let inner = json!({"name": "echo", "arguments": {}}).to_string();
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": inner, "id": "call_1"}
            ]
        })))
        .build()
}

/// Spin up a cell against `server_endpoint` with the given `timeout_ms`,
/// feed one tool_call message, return the single emission.
async fn run_handle_single(server_endpoint: &str, timeout_ms: u64) -> CellEmission {
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    let client = McpClient::new(server_endpoint, None).unwrap();
    let mut cell = McpCell::new(client, timeout_ms, 5_000, "main_mcp".into());

    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let msg = build_tool_call_msg("/mcp");
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
    rx.recv().await.expect("emission expected")
}

// ─── Test 1: RPC-Error ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_error_maps_to_mcp_error_code() {
    // Mock delivers a JSON-RPC error response with code -32000 / message "boom".
    let server = MockMcp::start(vec![canned_rpc_error(-32000, "boom")]).await;
    let em = run_handle_single(&server.endpoint(), 5_000).await;

    let header = &em.content["header"];
    assert_eq!(header["error_code"], "mcp_error", "header: {header}");
    let last = em.content["messages"].as_array().unwrap().last().unwrap();
    assert!(
        last["text"].as_str().unwrap().contains("boom"),
        "expected 'boom' in text, got: {last}"
    );
}

// ─── Test 2: A-Timeout → provider_timeout ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_maps_to_provider_timeout() {
    // Blackhole TCP listener: accepts connections but never writes a byte.
    // The cell's A-Timeout (200 ms) fires before the server responds.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                use tokio::io::AsyncReadExt;
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
            });
        }
    });

    let em = run_handle_single(&format!("http://{addr}/"), 200).await;
    let header = &em.content["header"];
    assert_eq!(
        header["error_code"], "provider_timeout",
        "expected provider_timeout, got header: {header}"
    );
}

// ─── Test 3: HTTP 5xx → mcp_error ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_5xx_maps_to_mcp_error_code() {
    // Mock returns HTTP 503, no body.
    let server = MockMcp::start(vec![MockResponse {
        status: 503,
        body: b"err".to_vec(),
        content_type: "text/plain".into(),
        delay: None,
    }])
    .await;

    let em = run_handle_single(&server.endpoint(), 5_000).await;
    let header = &em.content["header"];
    assert_eq!(header["error_code"], "mcp_error", "header: {header}");
}
