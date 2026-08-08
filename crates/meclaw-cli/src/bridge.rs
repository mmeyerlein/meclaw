//! stdio-Bridge: translation between stdin/stdout text lines and MeClaw [`Message`]s.
//!
//! Ingress: a text line from stdin → a `user`-turn [`Message`] targeted at the root hive `/`.
//! Egress: a [`Message`] arriving at `/` with no out-edge → the last `assistant` text turn → stdout.

use meclaw_core::{Body, Message, MessageBuilder, Path};
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// Well-known fixed UUID for the stdio user — identical across all runs.
/// Override via `--user-id` is post-v0.1.0 (docs/roadmap.md).
pub const STDIO_USER_ID: Uuid = Uuid::from_bytes([
    0x73, 0x74, 0x64, 0x69, 0x6f, 0x5f, 0x75, 0x73, 0x65, 0x72, 0x5f, 0x69, 0x64, 0x00, 0x00, 0x01,
]);

/// Translate a stdin text line into a source [`Message`] (one `user` turn) targeted at the root
/// hive `/`. `chat_id` is constant for the process run; `turn_id` is fresh per line.
pub fn line_to_message(line: &str, user_id: Uuid, chat_id: Uuid) -> Message {
    let body = json!({ "messages": [
        { "origin": "user", "type": "text", "text": line }
    ]});
    let mut ctx: Map<String, Value> = Map::new();
    ctx.insert("user_id".into(), json!(user_id.to_string()));
    ctx.insert("chat_id".into(), json!(chat_id.to_string()));
    ctx.insert("turn_id".into(), json!(Uuid::now_v7().to_string()));
    MessageBuilder::new(Path::new("/"))
        .context(ctx)
        .body(Body::Inline(body))
        .build()
}

/// Extract the last `assistant` text turn from a [`Message`] as a stdout line.
///
/// Returns `None` when there is no `assistant`-text turn (caller should log + discard).
/// Mirrors the proxy inbound extraction (`crates/meclaw-cells/src/proxy/cell.rs`).
pub fn message_to_stdout_line(msg: &Message) -> Option<String> {
    let Body::Inline(v) = &msg.body else {
        return None;
    };
    v.get("messages")
        .and_then(|m| m.as_array())
        .and_then(|arr| {
            arr.iter().rev().find(|t| {
                t.get("origin").and_then(|x| x.as_str()) == Some("assistant")
                    && t.get("type").and_then(|x| x.as_str()) == Some("text")
            })
        })
        .and_then(|t| t.get("text"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{Body, Path};

    // --- Task 3 tests ---

    #[test]
    fn line_becomes_user_turn_targeted_at_root() {
        let chat = Uuid::now_v7();
        let msg = line_to_message("hello world", STDIO_USER_ID, chat);
        assert_eq!(msg.target, Path::new("/"));
        let Body::Inline(v) = &msg.body else {
            panic!("inline body expected")
        };
        let turn = &v["messages"][0];
        assert_eq!(turn["origin"], "user");
        assert_eq!(turn["type"], "text");
        assert_eq!(turn["text"], "hello world");
        // context triad set:
        assert_eq!(
            msg.headers.context["user_id"],
            serde_json::json!(STDIO_USER_ID.to_string())
        );
        assert_eq!(
            msg.headers.context["chat_id"],
            serde_json::json!(chat.to_string())
        );
        assert!(msg.headers.context.contains_key("turn_id"));
    }

    #[test]
    fn turn_id_is_fresh_per_line() {
        let chat = Uuid::now_v7();
        let m1 = line_to_message("eins", STDIO_USER_ID, chat);
        let m2 = line_to_message("zwei", STDIO_USER_ID, chat);
        let id1 = &m1.headers.context["turn_id"];
        let id2 = &m2.headers.context["turn_id"];
        assert_ne!(id1, id2, "each line must get a distinct turn_id");
    }

    // --- Task 4 tests ---

    #[test]
    fn extracts_last_assistant_text() {
        let body = serde_json::json!({ "messages": [
            { "origin": "user", "type": "text", "text": "frage" },
            { "origin": "assistant", "type": "text", "text": "antwort" }
        ]});
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Inline(body))
            .build();
        assert_eq!(message_to_stdout_line(&msg).as_deref(), Some("antwort"));
    }

    #[test]
    fn none_when_no_assistant_turn() {
        let body = serde_json::json!({ "messages": [
            { "origin": "user", "type": "text", "text": "frage" }
        ]});
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Inline(body))
            .build();
        assert!(message_to_stdout_line(&msg).is_none());
    }

    #[test]
    fn extracts_last_of_multiple_assistant_turns() {
        let body = serde_json::json!({ "messages": [
            { "origin": "assistant", "type": "text", "text": "first answer" },
            { "origin": "assistant", "type": "text", "text": "last answer" }
        ]});
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Inline(body))
            .build();
        assert_eq!(message_to_stdout_line(&msg).as_deref(), Some("last answer"));
    }

    #[test]
    fn none_when_body_is_not_inline() {
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Blob(Uuid::now_v7()))
            .build();
        assert!(message_to_stdout_line(&msg).is_none());
    }
}
