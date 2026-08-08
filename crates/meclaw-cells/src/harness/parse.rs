//! Reading the tool call out of an inbound message.
//!
//! The convention is the one `store` and `mcp` already use
//! (`docs/cell-types.md` § store: structured JSON args in the tail
//! `tool_call` turn): `text` holds a JSON string with `name` and `arguments`,
//! and `id` is the call id the answer has to echo.
//!
//! Written out here rather than shared with `mcp`: the two cell types happen
//! to agree on a convention today, and coupling them through a helper would
//! make a change to one a change to both.

use meclaw_core::serde_json::Value as JsonValue;
use meclaw_core::{Body, Message};

/// A parsed tool call.
#[derive(Debug, Clone)]
pub struct ParsedCall {
    /// Which verb was invoked.
    pub name: String,
    /// Its arguments, as an object (or null when absent).
    pub arguments: JsonValue,
    /// The call id, echoed into every reply so a tool loop can match it.
    pub call_id: String,
}

/// Extract the last `tool_call` turn. Every failure names what was missing —
/// these messages reach an operator, not a debugger.
pub fn parse_tool_call(msg: &Message) -> Result<ParsedCall, String> {
    let Body::Inline(body) = &msg.body else {
        return Err("body: expected an inline UBF body".to_string());
    };
    let turns = body
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or("body: messages[] missing")?;
    let turn = turns
        .iter()
        .rev()
        .find(|t| t.get("type").and_then(|x| x.as_str()) == Some("tool_call"))
        .ok_or("body: no tool_call turn found")?;

    let call_id = turn
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or("tool_call: id missing (required by the UBF schema)")?
        .to_string();
    let text = turn
        .get("text")
        .and_then(|x| x.as_str())
        .ok_or("tool_call: text missing")?;
    let payload: JsonValue = meclaw_core::serde_json::from_str(text)
        .map_err(|e| format!("tool_call: text is not JSON: {e}"))?;
    let name = payload
        .get("name")
        .and_then(|x| x.as_str())
        .ok_or("tool_call: name missing")?
        .to_string();

    Ok(ParsedCall {
        name,
        arguments: payload.get("arguments").cloned().unwrap_or(JsonValue::Null),
        call_id,
    })
}
