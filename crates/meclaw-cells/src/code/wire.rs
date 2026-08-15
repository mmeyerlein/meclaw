//! Phase-9 code wire format.
//!
//! - **stdin**: serialize incoming Message into a three-object document —
//!   `envelope` (header, target, reply_to, trace_id, parent_message_id,
//!   correlation_id, ttl), `body` (the message slots) and `params` (a
//!   secret-filtered copy of the cell's own configuration).
//! - **stdout**: parse JSON, discriminate Object (1 message) vs. Array
//!   (N messages — Multi-Send-Wire-Format per cell-types.md Z.196–217).

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message};

/// Param keys whose value must NEVER reach the script's stdin, matched
/// exactly (ASCII-case-insensitive).
///
/// `auth` is listed as an exact key rather than as a prefix on purpose: an
/// ordinary knob such as `author` is configuration, not a credential, and
/// must keep travelling.
const SECRET_PARAM_KEYS: &[&str] = &["api_key", "auth", "auth_ref", "token", "secret", "password"];

/// Param-key suffixes whose value must NEVER reach the script's stdin
/// (`bot_token`, `app_token`, `openrouter_api_key`, `client_secret`, …).
///
/// Every suffix starts at the `_` separator, so a plural knob like
/// `max_tokens` — a budget, not a credential — is not caught.
const SECRET_PARAM_SUFFIXES: &[&str] = &["_key", "_token", "_secret", "_password"];

/// Substrate-owned params that carry the script's OWN source. Copying them
/// back onto stdin would double the wire payload of every single message
/// without giving the script anything it does not already have.
const SCRIPT_SOURCE_KEYS: &[&str] = &["script_inline", "script_path"];

/// Whether a param key is withheld from the stdin copy.
fn is_withheld_param_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SECRET_PARAM_KEYS.contains(&k.as_str())
        || SCRIPT_SOURCE_KEYS.contains(&k.as_str())
        || SECRET_PARAM_SUFFIXES.iter().any(|s| k.ends_with(s))
}

/// Recursively strip withheld keys from a nested param value.
fn filter_param_value(v: &Value) -> Value {
    match v {
        Value::Object(_) => filter_params_for_stdin(v),
        Value::Array(a) => Value::Array(a.iter().map(filter_param_value).collect()),
        other => other.clone(),
    }
}

/// Build the read-only `params` copy the script sees on stdin.
///
/// Returns an object in every case — a non-object input (or one whose keys
/// are all withheld) yields `{}` rather than a missing field, so the script
/// may read `d["params"]` unconditionally.
///
/// Withheld, recursively at every nesting level: the credential keys of
/// [`SECRET_PARAM_KEYS`] / [`SECRET_PARAM_SUFFIXES`] and the script's own
/// source ([`SCRIPT_SOURCE_KEYS`]). The function is idempotent — filtering an
/// already-filtered copy is a no-op.
pub fn filter_params_for_stdin(raw: &Value) -> Value {
    let Value::Object(o) = raw else {
        return Value::Object(Map::new());
    };
    let mut out = Map::new();
    for (k, v) in o {
        if is_withheld_param_key(k) {
            continue;
        }
        out.insert(k.clone(), filter_param_value(v));
    }
    Value::Object(out)
}

/// Serialize an incoming Message to the stdin-JSON the script reads.
///
/// The document has EXACTLY three top-level keys, all of them always present
/// and always objects:
///
/// - `envelope` — everything the substrate puts around the payload: `header`
///   (both compartments, `{"context":{…},"hop":{…}}`), `target`, `trace_id`,
///   `ttl`, plus `reply_to`, `parent_message_id` and `correlation_id` when the
///   message carries them.
/// - `body` — the message slots (e.g. `messages`, `system`), verbatim.
/// - `params` — the read-only, secret-filtered copy of the cell's own params
///   ([`filter_params_for_stdin`]), `{}` when there is nothing to hand over.
///
/// The top level is closed by construction: a script reads its payload from
/// `body` instead of subtracting a hard-coded envelope key list, and future
/// wire data lands INSIDE one of the three objects rather than beside them.
/// A body slot can never collide with `envelope` or `params` either, because
/// the slots no longer share a namespace with them.
///
/// stdout — what the script emits — is untouched by this shape.
///
/// Returns `Err` if `msg.body` is `Body::Blob` (Phase-12 deferred —
/// `code` expects Inline).
pub fn build_stdin_json(msg: &Message, params: &Value) -> Result<String, String> {
    let body = match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => {
            return Err("body is Blob — Phase-12 deferred (code expects Inline)".into());
        }
    };
    let body_obj = match body {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    let mut envelope = Map::new();
    // Path::as_str() — Path has no Display impl (verified against
    // meclaw-core/src/path.rs).
    // Slice 2 two-compartment model: the `header` field carries both
    // compartments (`{"context":{...},"hop":{...}}`) — the code cell sees both.
    let headers_value = meclaw_core::serde_json::to_value(&msg.headers)
        .map_err(|e| format!("header serialize: {e}"))?;
    envelope.insert("header".into(), headers_value);
    envelope.insert("target".into(), Value::String(msg.target.as_str().into()));
    if let Some(r) = &msg.reply_to {
        envelope.insert("reply_to".into(), Value::String(r.as_str().into()));
    }
    envelope.insert("trace_id".into(), Value::String(msg.trace_id.to_string()));
    if let Some(p) = &msg.parent_message_id {
        envelope.insert("parent_message_id".into(), Value::String(p.to_string()));
    }
    if let Some(c) = &msg.correlation_id {
        envelope.insert("correlation_id".into(), Value::String(c.to_string()));
    }
    envelope.insert("ttl".into(), json!(msg.ttl));

    let mut out = Map::new();
    out.insert("envelope".into(), Value::Object(envelope));
    out.insert("body".into(), Value::Object(body_obj));
    // route A (W12): the script sees its own configuration.
    out.insert("params".into(), filter_params_for_stdin(params));
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
        let s = build_stdin_json(&msg, &Value::Null).unwrap();
        let v: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert!(v["body"].get("messages").is_some());
        assert!(v["envelope"].get("header").is_some());
        assert!(v["envelope"].get("reply_to").is_some());
    }

    /// The shape contract itself: three keys, no more, no fewer — and the
    /// envelope carries every substrate field the flat document used to
    /// scatter beside the body slots.
    #[test]
    fn stdin_document_has_exactly_envelope_body_and_params() {
        let msg = MessageBuilder::new(Path::new("/x"))
            .body(Body::Inline(json!({
                "messages":[{"origin":"user","type":"text","text":"hi"}],
                "system": {"role": "tester"}
            })))
            .reply_to(Path::new("/reply"))
            .parent_message_id(meclaw_core::Uuid::nil())
            .correlation_id(meclaw_core::Uuid::nil())
            .build();
        let s = build_stdin_json(&msg, &json!({"window_size": 7})).unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();

        let top = v.as_object().expect("top level is an object");
        let mut keys: Vec<&str> = top.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["body", "envelope", "params"],
            "the top level is closed by construction — exactly three keys"
        );

        // body: the message slots, verbatim and nothing else.
        assert_eq!(
            v["body"],
            json!({
                "messages":[{"origin":"user","type":"text","text":"hi"}],
                "system": {"role": "tester"}
            })
        );

        // envelope: every substrate field, and none of them beside the body.
        let env = v["envelope"].as_object().expect("envelope is an object");
        let mut env_keys: Vec<&str> = env.keys().map(String::as_str).collect();
        env_keys.sort_unstable();
        assert_eq!(
            env_keys,
            [
                "correlation_id",
                "header",
                "parent_message_id",
                "reply_to",
                "target",
                "trace_id",
                "ttl",
            ]
        );
        assert_eq!(env["target"], json!("/x"));
        assert_eq!(env["reply_to"], json!("/reply"));
        assert!(env["header"].is_object());
        assert!(env["ttl"].is_number());

        // params: the cell's own configuration, next to the other two.
        assert_eq!(v["params"], json!({"window_size": 7}));
    }

    /// Optional envelope fields stay optional — but the three top-level keys
    /// and the mandatory envelope fields are there without them.
    #[test]
    fn stdin_envelope_omits_the_fields_the_message_does_not_carry() {
        let s = build_stdin_json(&plain_msg(), &Value::Null).unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert_eq!(v.as_object().map(Map::len), Some(3));
        assert!(v["params"].is_object(), "params is always there");
        let env = v["envelope"].as_object().expect("envelope is an object");
        let mut env_keys: Vec<&str> = env.keys().map(String::as_str).collect();
        env_keys.sort_unstable();
        assert_eq!(env_keys, ["header", "target", "trace_id", "ttl"]);
    }

    /// Helper: a minimal message, so the param assertions below read cleanly.
    fn plain_msg() -> Message {
        MessageBuilder::new(Path::new("/x"))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"messages":[]}),
            ))
            .build()
    }

    fn stdin_params_of(params: &Value) -> Value {
        let s = build_stdin_json(&plain_msg(), params).unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        v.get("params")
            .cloned()
            .expect("params field is always set")
    }

    #[test]
    fn stdin_params_field_is_always_present_and_empty_without_params() {
        // Deterministic beats optional: a script may read d["params"] blind.
        assert_eq!(stdin_params_of(&Value::Null), json!({}));
        assert_eq!(stdin_params_of(&json!({})), json!({}));
        // A non-object params value degrades to `{}`, never to a missing field.
        assert_eq!(stdin_params_of(&json!(7)), json!({}));
    }

    #[test]
    fn stdin_params_carries_configuration_but_withholds_secrets() {
        let out = stdin_params_of(&json!({
            "window_size": 12,
            "api_key": "sk-LEAK",
            "auth": "Bearer LEAK",
            "auth_ref": "env:LEAK",
            "token": "LEAK",
            "secret": "LEAK",
            "password": "LEAK",
            "bot_token": "LEAK",
            "client_secret": "LEAK",
            "db_password": "LEAK",
            "openrouter_api_key": "LEAK",
            // Configuration that only LOOKS like a credential — must travel.
            "max_tokens": 512,
            "author": "ada",
            "tokens_window": 8
        }));
        assert_eq!(
            out,
            json!({"window_size": 12, "max_tokens": 512, "author": "ada", "tokens_window": 8}),
            "only the credential-shaped keys are withheld"
        );
        let serialized = meclaw_core::serde_json::to_string(&out).unwrap();
        assert!(!serialized.contains("LEAK"), "no secret on the wire");
    }

    #[test]
    fn stdin_params_filter_reaches_into_nested_objects_and_arrays() {
        let out = stdin_params_of(&json!({
            "upstream": {"base_url": "http://x", "api_key": "sk-LEAK"},
            "peers": [{"name": "a", "bot_token": "LEAK"}, {"name": "b"}]
        }));
        assert_eq!(
            out,
            json!({
                "upstream": {"base_url": "http://x"},
                "peers": [{"name": "a"}, {"name": "b"}]
            })
        );
    }

    #[test]
    fn stdin_params_withholds_the_scripts_own_source() {
        // The script already IS its source; echoing it back would double the
        // wire payload of every message.
        let raw = json!({
            "runner": "python3",
            "script_inline": "print(1)",
            "script_path": "x.py",
            "external_timeout_ms": 10000
        });
        assert_eq!(
            stdin_params_of(&raw),
            json!({"runner": "python3", "external_timeout_ms": 10000})
        );
    }

    #[test]
    fn filter_params_for_stdin_is_idempotent() {
        let raw = json!({"window_size": 3, "api_key": "sk-LEAK", "nested": {"token": "LEAK"}});
        let once = filter_params_for_stdin(&raw);
        assert_eq!(filter_params_for_stdin(&once), once);
    }

    #[test]
    fn stdin_params_cannot_be_shadowed_by_a_body_slot() {
        // Body slots live in their own object now, so a slot called `params`
        // (or `envelope`) is simply a slot — it shares no namespace with the
        // cell's configuration and cannot spoof it.
        let msg = MessageBuilder::new(Path::new("/x"))
            .body(Body::Inline(json!({
                "messages":[],
                "params": {"spoofed": true},
                "envelope": {"spoofed": true}
            })))
            .build();
        let s = build_stdin_json(&msg, &json!({"window_size": 5})).unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert_eq!(v["params"], json!({"window_size": 5}));
        assert_eq!(v["body"]["params"], json!({"spoofed": true}));
        assert_eq!(v["envelope"]["target"], json!("/x"));
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
