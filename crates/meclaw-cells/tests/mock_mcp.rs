//! Phase-10-D test helper — MCP HTTP+JSON-RPC mock server wrapper around
//! `meclaw_testing::mock_http::start_mock_server_capturing`.
//!
//! T9: canned-response helpers (initialize, tools/list, tools/call, rpc-error)
//! + `McpRequestSnapshot` parser for captured request bodies.
//!
//! Used by T10 (mcp_wire_against_mock), T12 (mcp_io_loop), T17/T18/T19
//! (mcp_handle_*), T24/T25 (phase_10d_mcp_demo). Future integration tests
//! declare:
//! ```ignore
//!     #[path = "mock_mcp.rs"]
//!     mod mock_mcp;
//!     use mock_mcp::{MockMcp, canned_initialize, canned_tools_list};
//! ```
//!
//! Layering: MCP-protocol-specific knowledge (JSON-RPC shape, MCP method
//! signatures) lives here in `meclaw-cells/tests/`; the generic TCP listener +
//! request-capture mechanics live in `meclaw-testing/src/mock_http.rs`.
//!
//! # Plan-Adaption (T9)
//!
//! The T9 plan-text specified a handler-closure API
//! (`MockMcpServer::start(|method, params| ...)`). This file implements the
//! **canned-response-sequence API** (`Vec<MockResponse>`, one per expected
//! request, in arrival order) instead — mirroring `tests/mock_openai.rs` as
//! an explicit design decision. The closure approach is not established pattern here
//! and would require a custom TCP server; the sequence approach reuses the
//! battle-tested `start_mock_server_capturing` substrate.

#![allow(dead_code)]

use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Mock MCP server handle. Lives for the duration of the test; the background
/// listener task is held via the `JoinHandle` field and aborts on drop only
/// when the tokio runtime tears down.
///
/// Mirrors `MockOpenAI` from `tests/mock_openai.rs`.
pub struct MockMcp {
    /// The bound socket address (`127.0.0.1:<port>`).
    pub addr: std::net::SocketAddr,
    /// Convenience: `format!("http://{addr}")` — usable as MCP endpoint base.
    pub base_url: String,
    /// Captured requests in arrival order. Lock + iterate via `recorded_requests()`.
    pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

impl MockMcp {
    /// Start a mock MCP server with a sequence of canned JSON-RPC responses
    /// (one per expected incoming request, in arrival order). Panics if the
    /// test sends more requests than responses (loud failure, mirrors mock_openai).
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let (addr, join, captured) = start_mock_server_capturing(responses).await;
        let base_url = format!("http://{}", addr);
        Self {
            addr,
            base_url,
            captured,
            join,
        }
    }

    /// Snapshot all captured request bodies, parsed as JSON-RPC.
    /// Panics if a captured body is not valid JSON (test contract — the
    /// caller must only point real MCP-shaped traffic at this mock).
    pub async fn recorded_requests(&self) -> Vec<McpRequestSnapshot> {
        let caps = self.captured.lock().await;
        caps.iter()
            .map(|c| {
                let body: serde_json::Value =
                    serde_json::from_slice(&c.body).expect("captured body is JSON");
                McpRequestSnapshot {
                    method: c.method.clone(),
                    path: c.path.clone(),
                    headers: c.headers.clone(),
                    body,
                }
            })
            .collect()
    }

    /// Convenience for `McpClient::new(&self.endpoint(), None)`-style tests:
    /// returns `"{base_url}/"`.
    pub fn endpoint(&self) -> String {
        format!("{}/", self.base_url)
    }
}

/// Parsed view of a captured JSON-RPC request body.
pub struct McpRequestSnapshot {
    /// HTTP method (expected: `"POST"`).
    pub method: String,
    /// Request path (expected: `"/"`).
    pub path: String,
    /// Request headers, names lowercased.
    pub headers: HashMap<String, String>,
    /// Parsed JSON body — the JSON-RPC 2.0 request payload.
    pub body: serde_json::Value,
}

impl McpRequestSnapshot {
    /// `body.method` as `&str`, if present — the JSON-RPC method name
    /// (e.g. `"initialize"`, `"tools/list"`, `"tools/call"`).
    pub fn rpc_method(&self) -> Option<&str> {
        self.body.get("method").and_then(|v| v.as_str())
    }
}

// ───── Canned-response builders ─────

/// Canned JSON-RPC success response: `{"jsonrpc":"2.0","id":"mock-id","result":<result>}`.
///
/// # Id-echo trade-off
///
/// The `start_mock_server_capturing` substrate serves pre-built
/// `Vec<MockResponse>` bytes without inspecting the incoming request —
/// it cannot echo the request `id` back. The response is therefore built
/// with a fixed `"id": "mock-id"` string instead of `null`, because
/// `JsonRpcResponse::id` is a non-nullable `RequestId(String)` and serde
/// rejects `null` for transparent String wrappers. Id-correlation is not
/// asserted in integration tests; the fixed value is a test-contract constant.
pub fn canned_result(result_json: serde_json::Value) -> MockResponse {
    let v = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "mock-id",
        "result": result_json
    });
    MockResponse::ok_json(&serde_json::to_vec(&v).unwrap())
}

/// Canned JSON-RPC error response:
/// `{"jsonrpc":"2.0","id":"mock-id","error":{"code":...,"message":...}}`.
///
/// Same id-echo trade-off as [`canned_result`]: `id` is `"mock-id"` (fixed string).
pub fn canned_rpc_error(code: i64, message: &str) -> MockResponse {
    let v = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "mock-id",
        "error": { "code": code, "message": message }
    });
    MockResponse::ok_json(&serde_json::to_vec(&v).unwrap())
}

/// Canned MCP `tools/list` success — wraps `canned_result({"tools":[...]})`.
pub fn canned_tools_list(tools: Vec<serde_json::Value>) -> MockResponse {
    canned_result(serde_json::json!({ "tools": tools }))
}

/// Canned MCP `tools/call` success — wraps
/// `canned_result({"content":[{"type":"text","text":<text>}],"isError":false})`.
pub fn canned_tools_call_text(text: &str) -> MockResponse {
    canned_result(serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

/// Canned MCP `initialize` success — wraps
/// `canned_result({"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{}})`.
pub fn canned_initialize() -> MockResponse {
    canned_result(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "serverInfo": {}
    }))
}
