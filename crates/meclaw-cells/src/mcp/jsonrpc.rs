//! Phase-10-D: JSON-RPC 2.0 envelopes. Pure serde, no I/O.

use meclaw_core::Uuid;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Opaque request id, generated via UUID v7 (workspace convention).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a fresh request id.
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    /// Borrow as string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    /// Default generates a fresh request id (same as [`RequestId::new`]).
    fn default() -> Self {
        Self::new()
    }
}

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Request id (for response correlation).
    pub id: RequestId,
    /// MCP method name (e.g. `"tools/list"`).
    pub method: String,
    /// Method-specific params object.
    pub params: JsonValue,
}

impl JsonRpcRequest {
    /// Build a request with `jsonrpc = "2.0"`.
    pub fn new(id: RequestId, method: &str, params: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response envelope. Exactly one of `result` / `error`
/// is set in a valid response.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    /// Echoed request id.
    pub id: RequestId,
    /// Success payload (method-specific).
    #[serde(default)]
    pub result: Option<JsonValue>,
    /// Error payload (set on failure).
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code (negative for protocol errors).
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}
