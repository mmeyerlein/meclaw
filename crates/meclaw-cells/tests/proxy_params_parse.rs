//! Phase-10-C T3: ProxyParams::parse. Pflichtfelder bot_token + emit_to;
//! Defaults from W7; the W7 tripwire `long_poll_timeout_ms > long_poll_request_secs * 1000`.

use meclaw_cells::proxy::params::ProxyParams;
use serde_json::json;

#[test]
fn parse_minimal_required_fields_with_defaults() {
    let p = ProxyParams::parse(&json!({
        "bot_token": "1234:abc",
        "emit_to": "/main/agent",
    }))
    .unwrap();
    assert_eq!(p.bot_token, "1234:abc");
    assert_eq!(p.emit_to.as_str(), "/main/agent");
    assert_eq!(p.long_poll_timeout_ms, 35000);
    assert_eq!(p.long_poll_request_secs, 30);
    assert_eq!(p.send_timeout_ms, 10000);
    assert_eq!(p.query_timeout_ms, 5000);
    assert_eq!(p.base_url, "https://api.telegram.org");
}

#[test]
fn parse_rejects_missing_bot_token() {
    let err = ProxyParams::parse(&json!({"emit_to": "/x"})).unwrap_err();
    assert!(err.contains("bot_token"), "got: {err}");
}

#[test]
fn parse_rejects_missing_emit_to() {
    let err = ProxyParams::parse(&json!({"bot_token": "x"})).unwrap_err();
    assert!(err.contains("emit_to"), "got: {err}");
}

#[test]
fn parse_w7_tripwire_rejects_client_timeout_le_request_secs() {
    // long_poll_timeout_ms = 30000, long_poll_request_secs = 30 → 30s == 30s
    // → the client cuts off its own poll. Reject.
    let err = ProxyParams::parse(&json!({
        "bot_token": "x", "emit_to": "/x",
        "long_poll_timeout_ms": 30000,
        "long_poll_request_secs": 30,
    }))
    .unwrap_err();
    assert!(
        err.contains("long_poll_timeout_ms") && err.contains("long_poll_request_secs"),
        "got: {err}"
    );
}

#[test]
fn parse_w7_tripwire_accepts_client_timeout_strictly_greater() {
    // 31000 > 30000 → OK (>5s headroom recommended, but strictly > suffices).
    let p = ProxyParams::parse(&json!({
        "bot_token": "x", "emit_to": "/x",
        "long_poll_timeout_ms": 31000,
        "long_poll_request_secs": 30,
    }))
    .unwrap();
    assert_eq!(p.long_poll_timeout_ms, 31000);
}
