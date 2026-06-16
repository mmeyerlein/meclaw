//! Phase-10-C T15 / W7-Tripwire (Marcus 2026-05-24):
//! „Das Client-timeout für den Long-Poll MUSS größer sein als der an
//!  Telegram gesendete getUpdates?timeout=<sek>-Wert, sonst kappt der
//!  Client seinen eigenen gültigen Poll."
//!
//! Diese Datei ist die EXPLIZITE Tripwire-Demonstration. Strikt >, nicht >=.

use meclaw_cells::proxy::params::ProxyParams;
use serde_json::json;

#[test]
fn w7_tripwire_strict_greater_than_holds() {
    // 30001 > 30000 → OK
    let ok = ProxyParams::parse(&json!({
        "bot_token": "t", "emit_to": "/x",
        "long_poll_timeout_ms": 30001, "long_poll_request_secs": 30,
    }));
    assert!(ok.is_ok(), "30001ms > 30s*1000 must be accepted");

    // 30000 == 30000 → REJECT (strikt >, nicht >=)
    let eq = ProxyParams::parse(&json!({
        "bot_token": "t", "emit_to": "/x",
        "long_poll_timeout_ms": 30000, "long_poll_request_secs": 30,
    }));
    assert!(
        eq.is_err(),
        "30000ms == 30s*1000 must be REJECTED (strict >)"
    );

    // 29999 < 30000 → REJECT
    let lt = ProxyParams::parse(&json!({
        "bot_token": "t", "emit_to": "/x",
        "long_poll_timeout_ms": 29999, "long_poll_request_secs": 30,
    }));
    assert!(lt.is_err(), "29999ms < 30s*1000 must be REJECTED");

    // Default-Tuple 35000 > 30 → OK (Sanity)
    let def = ProxyParams::parse(&json!({"bot_token": "t", "emit_to": "/x"}));
    assert!(
        def.is_ok(),
        "default long_poll_timeout_ms=35000 > 30s*1000 must be accepted"
    );
}
