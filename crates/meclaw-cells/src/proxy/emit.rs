//! Phase-10-C: emit helpers for `ProxyCell`. (1) `build_user_turn_content`
//! constructs the UBF body for outbound (T11). (2) `emit_inbound_error` emits
//! error replies for `missing_chat_id`/`send_failed`/`missing_assistant_turn`
//! (T13, W5/W6/W12).

use meclaw_core::{CellOutput, JsonValue, Message, OutputSink, serde_json::json};

/// Builds the UBF content for a user-source emission. Headers per spec
/// cell-types.md l.370: `chat_id`, `user_id`, `platform: "telegram"`, optionally
/// `message_id`. Body = `messages[]` with one user turn (`origin: "user"`,
/// `type: "text"`, `text: <what the user typed>`).
pub fn build_user_turn_content(
    chat_id: i64,
    user_id: Option<i64>,
    message_id: Option<i64>,
    text: &str,
) -> JsonValue {
    let mut header = meclaw_core::serde_json::Map::new();
    header.insert("chat_id".into(), json!(chat_id));
    if let Some(uid) = user_id {
        header.insert("user_id".into(), json!(uid));
    }
    header.insert("platform".into(), json!("telegram"));
    if let Some(mid) = message_id {
        header.insert("message_id".into(), json!(mid));
    }
    json!({
        "header": header,
        "messages": [
            { "origin": "user", "type": "text", "text": text }
        ]
    })
}

/// Phase-10-C T13: error reply for the inbound failure paths (W5/W6/W12, spec
/// cell-types.md l.374, commit `1dc081a`). Target = `msg.reply_to`.
/// Phase-16 W2 (ruling A1): with `reply_to` absent, `/colony/dead_letters` (the
/// READ endpoint) is NO longer the emission target — the error reply travels as a
/// normal emission to the cell's own `msg.target`; matching no out-edge, it ends
/// up loudly as `no_route` in the DLQ. Non-conversational origin: `messages[]` is
/// empty — NO user/assistant turn. The pure-sink discipline is preserved because
/// the error reply is not a conversation turn.
pub async fn emit_inbound_error(sink: &OutputSink, msg: &Message, error_code: &str, detail: &str) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let content = json!({
        "header": {
            "error_code": error_code,
            "msg_type":   "proxy_inbound_error",
        },
        "messages": [],
        "meta": { "detail": detail },
    });
    let _ = sink.push(CellOutput { target, content }).await;
}
