//! Phase-9 code wire format.
//!
//! - **stdin**: serialize incoming Message to JSON (header, body slots,
//!   plus envelope fields target/reply_to/trace_id/parent_message_id/
//!   correlation_id/ttl).
//! - **stdout**: parse JSON, discriminate Object (1 message) vs. Array
//!   (N messages — Multi-Send-Wire-Format per cell-types.md Z.196–217).

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message};

/// Serialize an incoming Message to the stdin-JSON the script reads.
///
/// Includes the body slots (e.g. `messages`), plus a top-level `header`
/// object derived from `msg.headers`, plus envelope fields `target`,
/// `reply_to`, `trace_id`, `parent_message_id`, `correlation_id`, `ttl`.
///
/// Returns `Err` if `msg.body` is `Body::Blob` (Phase-12 deferred —
/// `code` expects Inline).
pub fn build_stdin_json(msg: &Message) -> Result<String, String> {
    let body = match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => {
            return Err("body is Blob — Phase-12 deferred (code expects Inline)".into());
        }
    };
    let mut out = match body {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    // Path::as_str() — Path has no Display impl (verified against
    // meclaw-core/src/path.rs).
    // Slice 2 Zwei-Fächer-Modell: das `header`-Feld trägt jetzt beide Fächer
    // (`{"context":{...},"hop":{...}}`) — die Code-Cell sieht beide Compartments.
    let headers_value = meclaw_core::serde_json::to_value(&msg.headers)
        .map_err(|e| format!("header serialize: {e}"))?;
    out.insert("header".into(), headers_value);
    out.insert("target".into(), Value::String(msg.target.as_str().into()));
    if let Some(r) = &msg.reply_to {
        out.insert("reply_to".into(), Value::String(r.as_str().into()));
    }
    out.insert("trace_id".into(), Value::String(msg.trace_id.to_string()));
    if let Some(p) = &msg.parent_message_id {
        out.insert("parent_message_id".into(), Value::String(p.to_string()));
    }
    if let Some(c) = &msg.correlation_id {
        out.insert("correlation_id".into(), Value::String(c.to_string()));
    }
    out.insert("ttl".into(), json!(msg.ttl));
    meclaw_core::serde_json::to_string(&Value::Object(out)).map_err(|e| e.to_string())
}

/// Parse the script's stdout-JSON; return a Vec of content-JSONs.
///
/// Object → 1 entry, Array → N entries (Multi-Send-Wire-Format per
/// cell-types.md Z.196–217). Other top-level types → Err
/// (`invalid_json`-equivalent).
pub fn parse_stdout_json(s: &str) -> Result<Vec<Value>, String> {
    let v: Value =
        meclaw_core::serde_json::from_str(s).map_err(|e| format!("invalid_json: {e}"))?;
    match v {
        Value::Array(a) => Ok(a),
        Value::Object(_) => Ok(vec![v]),
        _ => Err("invalid_json: top-level must be Object or Array".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{Body, MessageBuilder, Path};

    #[test]
    fn stdin_includes_messages_and_headers() {
        let msg = MessageBuilder::new(Path::new("/x"))
            .body(Body::Inline(meclaw_core::serde_json::json!({
                "messages":[{"origin":"user","type":"text","text":"hi"}]
            })))
            .reply_to(Path::new("/reply"))
            .build();
        let s = build_stdin_json(&msg).unwrap();
        let v: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert!(v.get("messages").is_some());
        assert!(v.get("reply_to").is_some());
    }

    #[test]
    fn stdout_object_yields_single_message() {
        let s =
            r#"{"header":{"x":1},"messages":[{"origin":"assistant","type":"text","text":"y"}]}"#;
        let parsed = parse_stdout_json(s).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn stdout_array_yields_multiple_messages() {
        let s = r#"[{"messages":[]},{"messages":[]}]"#;
        let parsed = parse_stdout_json(s).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn stdout_invalid_json_returns_err() {
        let r = parse_stdout_json("not json");
        assert!(r.is_err());
    }
}
