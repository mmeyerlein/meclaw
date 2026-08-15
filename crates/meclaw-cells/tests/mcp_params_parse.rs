//! T2: McpParams::parse — required, defaults, error paths.

use meclaw_cells::mcp::params::{McpParams, McpTransport};
use serde_json::json;

/// The http transport's two identity fields. P7 moved them behind
/// `McpParams::transport`; the assertions below are unchanged otherwise.
fn http_parts(p: &McpParams) -> (String, Option<String>) {
    match &p.transport {
        McpTransport::Http { endpoint, bearer } => (endpoint.clone(), bearer.clone()),
        other => panic!("expected the http transport, got {other:?}"),
    }
}

#[test]
fn parse_minimal_required_only_uses_defaults() {
    let v = json!({ "endpoint": "https://x.example/rpc" });
    let p = McpParams::parse(&v).unwrap();
    let (endpoint, bearer) = http_parts(&p);
    assert_eq!(endpoint, "https://x.example/rpc");
    assert_eq!(bearer, None);
    assert_eq!(p.external_timeout_ms, 30_000);
    assert_eq!(p.query_timeout_ms, 5_000);
}

#[test]
fn parse_with_full_fields() {
    let v = json!({
        "endpoint": "https://x.example/rpc",
        "auth": { "bearer": "tok-abc" },
        "external_timeout_ms": 12_345,
        "query_timeout_ms": 678
    });
    let p = McpParams::parse(&v).unwrap();
    let (_, bearer) = http_parts(&p);
    assert_eq!(bearer.as_deref(), Some("tok-abc"));
    assert_eq!(p.external_timeout_ms, 12_345);
    assert_eq!(p.query_timeout_ms, 678);
}

#[test]
fn parse_missing_endpoint_rejected() {
    let v = json!({ "auth": { "bearer": "x" } });
    let err = McpParams::parse(&v).unwrap_err();
    assert!(err.contains("endpoint"), "got: {err}");
}

#[test]
fn parse_non_object_rejected() {
    let v = json!(["not", "an", "object"]);
    let err = McpParams::parse(&v).unwrap_err();
    assert!(err.contains("object"), "got: {err}");
}
