//! Phase-10-C: Emit-Helpers für `ProxyCell`. (1) `build_user_turn_content`
//! konstruiert den UBF-Body für Outbound (T11). (2) `emit_inbound_error`
//! emittiert Error-Replies für `missing_chat_id`/`send_failed`/`missing_assistant_turn`
//! (T13, W5/W6/W12).

use meclaw_core::{CellOutput, JsonValue, Message, OutputSink, serde_json::json};

/// Baut den UBF-Content für eine User-Source-Emission. Header pro Spec
/// cell-types.md Z.370: `chat_id`, `user_id`, `platform: "telegram"`,
/// optional `message_id`. Body = `messages[]` mit einem User-Turn
/// (`origin: "user"`, `type: "text"`, `text: <user-getippt>`).
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

/// Phase-10-C T13: Error-Reply für Inbound-Fehlerpfade (W5/W6/W12, Spec
/// cell-types.md Z.374 Commit `1dc081a`). Target = `msg.reply_to`.
/// Phase-16 W2 (Ruling A1): bei fehlendem `reply_to` ist `/colony/dead_letters`
/// (der READ-Endpoint) NICHT mehr das Emissions-Target — die Error-Reply läuft
/// als normale Emission an das eigene `msg.target`; matcht keine Out-Edge,
/// endet sie laut als `no_route` in der DLQ. Nicht-Konversations-Origin:
/// `messages[]` ist leer — KEIN user/assistant-Turn. Die Pure-Sink-Disziplin
/// bleibt gewahrt, weil die Error-Reply kein Konversations-Turn ist.
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
