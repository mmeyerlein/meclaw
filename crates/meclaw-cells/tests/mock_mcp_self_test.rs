//! T9: self-test for the mock MCP server helper.
//!
//! Proves that `MockMcp::start(vec![canned_initialize()])` answers an
//! `initialize` POST with the expected JSON-RPC shape.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use mock_mcp::{MockMcp, canned_initialize};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_returns_canned_initialize_response() {
    let mock = MockMcp::start(vec![canned_initialize()]).await;
    assert!(mock.base_url.starts_with("http://"));

    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(mock.endpoint())
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "x-1",
            "method": "initialize",
            "params": {}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].method, "POST");
    assert_eq!(snaps[0].rpc_method(), Some("initialize"));
}
