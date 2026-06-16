//! T10: McpClient against the mock MCP server — initialize / list_tools
//! / call_tool happy path + JSON-RPC-error path + HTTP-non-2xx path.
//!
//! Plan-Adaption: uses the canned-sequence API (`MockMcp::start(Vec<MockResponse>)`)
//! from T9, not the handler-closure form from the plan-text.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::wire::{McpClient, McpError};
use meclaw_testing::mock_http::MockResponse;
use mock_mcp::{
    MockMcp, canned_initialize, canned_rpc_error, canned_tools_call_text, canned_tools_list,
};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_succeeds_against_mock() {
    let server = MockMcp::start(vec![canned_initialize()]).await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    client.initialize(Duration::from_secs(2)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_tools_returns_echo() {
    let server = MockMcp::start(vec![canned_tools_list(vec![json!({
        "name": "echo",
        "description": "Echoes input.",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
    })])])
    .await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let tools = client.list_tools(Duration::from_secs(2)).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    let parsed: serde_json::Value = serde_json::from_str(&tools[0].schema_json).unwrap();
    assert_eq!(parsed["name"], "echo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_tool_returns_content() {
    let server = MockMcp::start(vec![canned_tools_call_text("hello back")]).await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let result = client
        .call_tool("echo", json!({"text": "hi"}), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "hello back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_error_surfaces_as_mcp_error() {
    let server = MockMcp::start(vec![canned_rpc_error(-32000, "boom")]).await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let err = client
        .call_tool("echo", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    match err {
        McpError::Rpc { code, message } => {
            assert_eq!(code, -32000);
            assert_eq!(message, "boom");
        }
        other => panic!("expected Rpc, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_non_2xx_surfaces_as_transport() {
    // MockResponse fields are all pub — build a 503 response directly.
    let r503 = MockResponse {
        status: 503,
        body: b"err".to_vec(),
        content_type: "text/plain".into(),
        delay: None,
    };
    let server = MockMcp::start(vec![r503]).await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let err = client.initialize(Duration::from_secs(2)).await.unwrap_err();
    match err {
        McpError::Transport(s) => assert!(s.contains("503"), "got: {s}"),
        other => panic!("expected Transport, got {other:?}"),
    }
}
