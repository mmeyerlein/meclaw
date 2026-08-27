//! The Phoenix channels wire format, on its own.
//!
//! GH #381. The protocol is a JSON 5-tuple `[join_ref, ref, topic, event,
//! payload]`, and a reply reuses both refs. That much is true for every
//! consumer; what differs is who answers. The codec was split out so it would
//! not belong to either answering side: the `web` cell replies out of its own
//! materialised pages and runs its own loop. The second loop it was once
//! shared with — the api-side connection — went with GH #396, and the split
//! outlived it, because a codec that knows nothing about who answers is the
//! right shape either way.
//!
//! This module deliberately understands **no event names**. `node:moved` means
//! nothing here: the name and value travel verbatim. The moment this layer
//! interpreted one, the binary would know what is being drawn.

use meclaw_core::serde_json::{Value, json};

/// One inbound frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The client's join reference. Echoed back unchanged.
    pub join_ref: Value,
    /// The client's message reference. Echoed back unchanged.
    pub msg_ref: Value,
    /// The channel topic, e.g. `lv:surface-web` or `phoenix`.
    pub topic: String,
    /// The event name, e.g. `phx_join`, `heartbeat`, `event`.
    pub event: String,
    /// Whatever the client sent with it.
    pub payload: Value,
}

/// Parse a text frame.
///
/// `None` means the text was not a vsn 2.0.0 tuple, and the caller should close
/// the connection rather than guess.
pub fn parse(text: &str) -> Option<Frame> {
    let parsed: Value = meclaw_core::serde_json::from_str(text).ok()?;
    let tuple = parsed.as_array()?;
    if tuple.len() != 5 {
        return None;
    }
    Some(Frame {
        join_ref: tuple[0].clone(),
        msg_ref: tuple[1].clone(),
        topic: tuple[2].as_str()?.to_string(),
        event: tuple[3].as_str()?.to_string(),
        payload: tuple[4].clone(),
    })
}

/// A successful reply, reusing both refs.
pub fn ok_reply(join_ref: &Value, msg_ref: &Value, topic: &str, response: Value) -> String {
    meclaw_core::serde_json::to_string(&json!([
        join_ref,
        msg_ref,
        topic,
        "phx_reply",
        { "status": "ok", "response": response }
    ]))
    .unwrap_or_default()
}

/// A refusal, carrying the reason the answering side gave.
pub fn error_reply(join_ref: &Value, msg_ref: &Value, topic: &str, reason: String) -> String {
    meclaw_core::serde_json::to_string(&json!([
        join_ref,
        msg_ref,
        topic,
        "phx_reply",
        { "status": "error", "response": { "reason": reason } }
    ]))
    .unwrap_or_default()
}

/// A server-initiated frame: no reply, no refs.
///
/// This is how a diff reaches a viewer that did not ask for it — the shape
/// `["<join_ref>", null, topic, "diff", payload]`.
pub fn push(join_ref: &Value, topic: &str, event: &str, payload: Value) -> String {
    meclaw_core::serde_json::to_string(&json!([join_ref, Value::Null, topic, event, payload]))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_tuple_parses() {
        let f = parse(r#"["1","2","phoenix","heartbeat",{}]"#).expect("parse");
        assert_eq!(f.topic, "phoenix");
        assert_eq!(f.event, "heartbeat");
    }

    #[test]
    fn anything_that_is_not_a_five_tuple_is_refused() {
        assert!(parse("{}").is_none());
        assert!(parse("[1,2,3]").is_none());
        assert!(parse("not json").is_none());
        // A topic that is not a string: the shape is right, the types are not.
        assert!(parse(r#"["1","2",7,"heartbeat",{}]"#).is_none());
    }

    #[test]
    fn a_reply_reuses_both_refs() {
        let s = ok_reply(&json!("7"), &json!("9"), "lv:x", json!({}));
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert_eq!(v[0], json!("7"));
        assert_eq!(v[1], json!("9"));
        assert_eq!(v[3], json!("phx_reply"));
        assert_eq!(v[4]["status"], json!("ok"));
    }

    #[test]
    fn a_push_has_no_message_ref() {
        // Nobody is waiting on it, so there is no ref to reuse.
        let s = push(&json!("7"), "lv:x", "diff", json!({"0": "<p>hi</p>"}));
        let v: Value = meclaw_core::serde_json::from_str(&s).unwrap();
        assert_eq!(v[1], Value::Null);
        assert_eq!(v[3], json!("diff"));
    }
}
