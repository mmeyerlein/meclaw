//! Phase-10-D: `McpClient`. A reqwest-based HTTP+JSON-RPC wrapper for MCP.
//! POC scope: `initialize`, `list_tools`, `call_tool`. An A timeout per op via
//! `tokio::time::timeout` (CONTRIBUTING.md rule 12). TLS gate: reqwest with
//! `rustls-tls` + `default-features = false` (phase-7 gate).

use crate::mcp::db::DiscoveredTool;
use crate::mcp::jsonrpc::{JsonRpcRequest, JsonRpcResponse, RequestId};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;

/// Error classification for `McpClient` ops. In the POC every non-timeout error
/// maps onto `mcp_error` as the `error_code` header (see
/// `emit::emit_tool_result_error`).
#[derive(Debug)]
pub enum McpError {
    /// A timeout elapsed (client-side `tokio::time::timeout`).
    Timeout,
    /// reqwest build/send/JSON-parse errors, HTTP non-2xx, missing result and
    /// error.
    Transport(String),
    /// JSON-RPC error object from the server.
    Rpc {
        /// Negative numeric code per JSON-RPC 2.0.
        code: i64,
        /// Server-side human-readable message.
        message: String,
    },
}

/// HTTP+JSON-RPC MCP client.
///
/// Holds a `reqwest::Client` (Arc internally) + endpoint + an optional bearer
/// token. `Clone` is cheap (Arc internally). One client is built per cell
/// instance in the factory; `McpCell` and `McpIo` each hold a clone (symmetric to
/// the `proxy::TelegramClient` pattern).
#[derive(Clone)]
pub struct McpClient {
    inner: reqwest::Client,
    endpoint: String,
    bearer: Option<String>,
}

impl McpClient {
    /// Build the client. TLS/builder errors yield a string error for the factory
    /// spawn path (analogous to `TelegramClient::new`).
    pub fn new(endpoint: &str, bearer: Option<String>) -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest build: {e}"))?;
        Ok(Self {
            inner,
            endpoint: endpoint.to_string(),
            bearer,
        })
    }

    /// Send a single JSON-RPC request and decode the response. A-Timeout
    /// is mandatory (`tokio::time::timeout`). `Timeout` → `McpError::Timeout`;
    /// reqwest/HTTP/decode errors → `McpError::Transport`;
    /// a JSON-RPC error object → `McpError::Rpc`.
    pub async fn call_rpc(
        &self,
        method: &str,
        params: JsonValue,
        timeout: Duration,
    ) -> Result<JsonValue, McpError> {
        let req = JsonRpcRequest::new(RequestId::new(), method, params);
        let mut builder = self.inner.post(&self.endpoint).json(&req);
        if let Some(tok) = &self.bearer {
            builder = builder.bearer_auth(tok);
        }
        let fut = builder.send();
        let resp = tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| McpError::Timeout)?
            .map_err(|e| McpError::Transport(format!("send: {e}")))?;
        if !resp.status().is_success() {
            return Err(McpError::Transport(format!("status: {}", resp.status())));
        }
        let parsed: JsonRpcResponse = resp
            .json()
            .await
            .map_err(|e| McpError::Transport(format!("json: {e}")))?;
        if let Some(err) = parsed.error {
            return Err(McpError::Rpc {
                code: err.code,
                message: err.message,
            });
        }
        parsed
            .result
            .ok_or_else(|| McpError::Transport("missing result and error".into()))
    }

    /// Perform the MCP `initialize` handshake. POC: no capability match;
    /// the server response is discarded after confirming no RPC error.
    /// The A timeout is applied per CONTRIBUTING.md rule 12.
    pub async fn initialize(&self, timeout: Duration) -> Result<(), McpError> {
        let _ = self
            .call_rpc("initialize", build_initialize_params(), timeout)
            .await?;
        Ok(())
    }

    /// Invoke a single MCP tool via `tools/call`.
    ///
    /// Builds a `tools/call` JSON-RPC request with params
    /// `{"name": name, "arguments": arguments}` and returns the raw
    /// `result` object from the server response. The caller (cell handler)
    /// serialises this into the `tool_result` turn via
    /// `emit::emit_tool_result_success`. A-Timeout is applied per
    /// CONTRIBUTING.md Regel 12.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: JsonValue,
        timeout: Duration,
    ) -> Result<JsonValue, McpError> {
        let params = json!({
            "name": name,
            "arguments": arguments,
        });
        self.call_rpc("tools/call", params, timeout).await
    }

    /// Fetch the tool list from the MCP provider via `tools/list`.
    ///
    /// Expected server response shape:
    /// `{ "tools": [{"name": "...", "inputSchema": {...}, ...}] }`.
    ///
    /// Each tool object is stored as-is in `DiscoveredTool::schema_json`
    /// (full object, not just `inputSchema`) so downstream consumers also
    /// receive `description` and other metadata. A-Timeout is applied per
    /// CONTRIBUTING.md Regel 12.
    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<DiscoveredTool>, McpError> {
        let result = self.call_rpc("tools/list", json!({}), timeout).await?;
        tools_from_result(&result)
    }
}

/// Decode a `tools/list` result object into the discovery snapshot.
///
/// Shared by both transports so the discovery cache holds identical rows
/// whether the provider was reached over HTTP or over a child process.
pub fn tools_from_result(result: &JsonValue) -> Result<Vec<DiscoveredTool>, McpError> {
    let arr = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| McpError::Transport("tools/list: missing tools array".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        let name = t
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| McpError::Transport("tools/list: tool missing name".into()))?
            .to_string();
        out.push(DiscoveredTool {
            name,
            schema_json: t.to_string(),
        });
    }
    Ok(out)
}

/// MCP-protocol-version constant for the POC `initialize` handshake.
/// Server-side `protocolVersion` from the response is ignored (POC).
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Builds the params object for `initialize`. Pure helper, kept here so
/// the T6 test can assert its shape without spinning up an `McpClient`.
pub fn build_initialize_params() -> JsonValue {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "meclaw-mcp-cell", "version": "0.1.0" }
    })
}
