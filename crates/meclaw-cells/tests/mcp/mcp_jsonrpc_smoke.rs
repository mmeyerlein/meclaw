//! T3: JSON-RPC-Envelope-Serde + RequestId-Generation.

use meclaw_cells::mcp::jsonrpc::{JsonRpcRequest, JsonRpcResponse, RequestId};
use serde_json::{Value, json};

#[test]
fn build_request_serializes_with_id_method_params() {
    let id = RequestId::new();
    let req = JsonRpcRequest::new(id.clone(), "tools/list", json!({}));
    let v: Value = serde_json::to_value(&req).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "tools/list");
    assert_eq!(v["id"], id.as_str());
    assert_eq!(v["params"], json!({}));
}

#[test]
fn parse_success_response() {
    let raw = json!({
        "jsonrpc": "2.0",
        "id": "abc-123",
        "result": { "tools": [] }
    });
    let resp: JsonRpcResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(resp.id.as_str(), "abc-123");
    let result = resp.result.unwrap();
    assert_eq!(result["tools"], json!([]));
    assert!(resp.error.is_none());
}

#[test]
fn parse_error_response() {
    let raw = json!({
        "jsonrpc": "2.0",
        "id": "abc-123",
        "error": { "code": -32000, "message": "boom" }
    });
    let resp: JsonRpcResponse = serde_json::from_value(raw).unwrap();
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32000);
    assert_eq!(err.message, "boom");
    assert!(resp.result.is_none());
}

#[test]
fn request_id_unique_across_calls() {
    let a = RequestId::new();
    let b = RequestId::new();
    assert_ne!(a.as_str(), b.as_str());
}
