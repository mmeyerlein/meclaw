//! Phase-10-D: Pure parser for the `tool_call`-tail-turn. Extracts `name` +
//! `arguments` from `text` (JSON-string), echoes `id` as `call_id` for
//! `tool_result`. Mirrors the Phase-9 `store` convention
//! (`crates/meclaw-cells/src/store/cell.rs::parse_tool_call`): `text` is a
//! JSON-string `{"name":"X","arguments":{...}}`; `id` is the UBF-required
//! tool_call id. Top-level `name`/`arguments` fields on the turn are
//! UBF-invalid (`TurnObject.additionalProperties: false`).

use meclaw_core::{Body, Message};
use serde_json::Value as JsonValue;

/// A parsed `tool_call`-turn extracted from the tail of `messages[]`.
#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    /// Tool name (e.g. `"echo"`, or the reserved `"__list_tools__"`).
    pub name: String,
    /// JSON arguments object — defaults to `{}` if absent in `text`.
    pub arguments: JsonValue,
    /// UBF-required `id` from the turn, echoed as `tool_result.id`.
    pub call_id: String,
}

/// Parse the last `messages[]`-entry of `msg` as a `tool_call`-turn.
///
/// Phase-9-store convention (authoritative): `text` is a JSON-string
/// `{"name":"X","arguments":{...}}`; `id` is UBF-required. Returns
/// `Err(<reason>)` on any structural violation.
pub fn parse_tool_call(msg: &Message) -> Result<ParsedToolCall, String> {
    let Body::Inline(v) = &msg.body else {
        return Err("body must be inline (Blob deferred)".into());
    };
    let arr = v
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or("body.messages: missing or not array")?;
    let last = arr.last().ok_or("body.messages: empty")?;
    let ty = last
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or("last turn: missing type")?;
    if ty != "tool_call" {
        return Err(format!("last turn: expected tool_call, got {ty}"));
    }
    let call_id = last
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or("tool_call: missing id (UBF-required)")?
        .to_string();
    let text = last
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or("tool_call: missing text")?;
    let parsed: JsonValue =
        serde_json::from_str(text).map_err(|e| format!("tool_call.text not valid JSON: {e}"))?;
    let name = parsed
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or("tool_call.text: missing name")?
        .to_string();
    let arguments = parsed
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(ParsedToolCall {
        name,
        arguments,
        call_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{Body, MessageBuilder, Path};
    use serde_json::json;

    fn msg_with_tool_call(text: &str, id: &str) -> Message {
        let body = Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": text, "id": id}
            ]
        }));
        MessageBuilder::new(Path::new("/mcp")).body(body).build()
    }

    #[test]
    fn extracts_name_arguments_and_id() {
        let txt = json!({"name": "echo", "arguments": {"text": "yo"}}).to_string();
        let m = msg_with_tool_call(&txt, "call_1");
        let p = parse_tool_call(&m).unwrap();
        assert_eq!(p.name, "echo");
        assert_eq!(p.arguments["text"], "yo");
        assert_eq!(p.call_id, "call_1");
    }

    #[test]
    fn missing_arguments_defaults_to_empty_object() {
        let txt = json!({"name": "x"}).to_string();
        let m = msg_with_tool_call(&txt, "call_2");
        let p = parse_tool_call(&m).unwrap();
        assert_eq!(p.arguments, json!({}));
    }

    #[test]
    fn wrong_last_turn_type_rejected() {
        let body = Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "x"}]
        }));
        let m = MessageBuilder::new(Path::new("/mcp")).body(body).build();
        assert!(parse_tool_call(&m).is_err());
    }

    #[test]
    fn missing_id_rejected() {
        let body = Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": "{\"name\":\"x\"}"}
            ]
        }));
        let m = MessageBuilder::new(Path::new("/mcp")).body(body).build();
        let err = parse_tool_call(&m).unwrap_err();
        assert!(err.contains("id"), "got: {err}");
    }
}
