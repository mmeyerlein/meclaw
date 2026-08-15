//! P12: UBF content for Slack-sourced user turns.
//!
//! The `chat_id` here is a composite — `"<channel>"` for a DM, or
//! `"<channel>:<thread_ts>"` when the conversation lives in a thread. One
//! opaque string is what the standard header convention wants, and it survives
//! the promotion edge unchanged because the mechanism is type-agnostic.
//!
//! Everything in `header` becomes the `hop` of the emitted message. Only what
//! an out-edge promotes with `modifier.set_context` reaches the persistent
//! `context` — cells never write `context` themselves (overview
//! § Standard-Header-Konvention). `chat_id` MUST be promoted, or the reply leg
//! will not know where to answer; the Slack template ships that edge.

use super::wire::SlackUserEvent;
use meclaw_core::{JsonValue, serde_json::json};

/// Builds the composite address for a conversation.
///
/// Kept in one place because both the emit path and the reply path have to
/// agree on it exactly; two independently written string joins would drift.
pub fn composite_chat_id(channel: &str, thread_ts: Option<&str>) -> String {
    match thread_ts {
        Some(t) => format!("{channel}:{t}"),
        None => channel.to_string(),
    }
}

/// Splits a composite address back into channel and thread.
///
/// `splitn(2, ':')` is safe: Slack ids and timestamps contain no colon, so the
/// first colon is unambiguously the separator.
pub fn split_chat_id(chat_id: &str) -> (String, Option<String>) {
    match chat_id.split_once(':') {
        Some((channel, thread)) if !thread.is_empty() => {
            (channel.to_string(), Some(thread.to_string()))
        }
        _ => (chat_id.to_string(), None),
    }
}

/// UBF content for one user-authored Slack message.
///
/// `thread_ts` is the conversation's thread, which for a root mention is the
/// mention's own `ts` — that is the thread the bot is about to open.
pub fn build_user_turn_content(
    event: &SlackUserEvent,
    thread_ts: Option<&str>,
    text: &str,
) -> JsonValue {
    let mut header = meclaw_core::serde_json::Map::new();
    header.insert(
        "chat_id".into(),
        json!(composite_chat_id(&event.channel, thread_ts)),
    );
    if let Some(user) = &event.user {
        header.insert("user_id".into(), json!(user));
    }
    header.insert("platform".into(), json!("slack"));
    header.insert("slack_channel".into(), json!(event.channel));
    if let Some(t) = thread_ts {
        header.insert("slack_thread_ts".into(), json!(t));
    }
    if let Some(ev_ts) = &event.event_ts {
        header.insert("slack_event_ts".into(), json!(ev_ts));
    }
    json!({
        "header": header,
        "messages": [
            { "origin": "user", "type": "text", "text": text }
        ]
    })
}
