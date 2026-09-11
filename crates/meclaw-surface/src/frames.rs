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

/// The kind byte of a client push in the v2 serializer.
const KIND_PUSH: u8 = 0;
/// The kind byte of a server broadcast in the v2 serializer.
const KIND_BROADCAST: u8 = 2;
/// The longest a length-prefixed header field may be: one byte holds it.
const FIELD_MAX: usize = 255;

/// One inbound binary push: kind 0 of the v2 serializer.
///
/// The layout is the one `phoenix.min.js` `binaryEncode` writes:
/// `[0][join_ref_len][ref_len][topic_len][event_len][join_ref][ref][topic][event][payload]`.
/// Every length is a single byte, which is why the topic of a call — `voice:`
/// and a session id of at most 128 characters — is the longest one that can be
/// carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryFrame {
    /// The client's join reference, as text. A binary push carries it so a
    /// reply could address the same channel.
    pub join_ref: String,
    /// The client's message reference. Empty when the client wants no reply.
    pub msg_ref: String,
    /// The channel topic this frame belongs to.
    pub topic: String,
    /// The event name, travelling verbatim like every other name here.
    pub event: String,
    /// The bytes behind the header.
    pub payload: Vec<u8>,
}

/// Parse a binary push. `None` for any other kind or a header the buffer cannot hold.
///
/// Every read is bounds-checked, because the bytes come off a socket: a header
/// that promises more than arrived is a refusal, never a slice that panics.
pub fn parse_binary(bytes: &[u8]) -> Option<BinaryFrame> {
    if *bytes.first()? != KIND_PUSH {
        return None;
    }
    let join_len = usize::from(*bytes.get(1)?);
    let ref_len = usize::from(*bytes.get(2)?);
    let topic_len = usize::from(*bytes.get(3)?);
    let event_len = usize::from(*bytes.get(4)?);
    let mut at = 5usize;
    let mut take = |len: usize, buf: &[u8]| -> Option<String> {
        let end = at.checked_add(len)?;
        let field = buf.get(at..end)?;
        at = end;
        Some(std::str::from_utf8(field).ok()?.to_string())
    };
    let join_ref = take(join_len, bytes)?;
    let msg_ref = take(ref_len, bytes)?;
    let topic = take(topic_len, bytes)?;
    let event = take(event_len, bytes)?;
    Some(BinaryFrame {
        join_ref,
        msg_ref,
        topic,
        event,
        payload: bytes.get(at..)?.to_vec(),
    })
}

/// A server-initiated binary broadcast (kind 2): no refs, topic, event, payload.
///
/// Broadcast rather than a server push, because it is the one binary form that
/// needs no reference: nobody is waiting on it. A topic or event over 255 bytes
/// cannot be encoded at all, and the answer is an empty `Vec` rather than a
/// truncated frame — the caller treats that as "not sent".
pub fn binary_broadcast(topic: &str, event: &str, payload: &[u8]) -> Vec<u8> {
    if topic.len() > FIELD_MAX || event.len() > FIELD_MAX {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(3 + topic.len() + event.len() + payload.len());
    out.push(KIND_BROADCAST);
    out.push(topic.len() as u8);
    out.push(event.len() as u8);
    out.extend_from_slice(topic.as_bytes());
    out.extend_from_slice(event.as_bytes());
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_push_decodes_to_the_tuple_the_client_wrote() {
        // What phoenix.min.js `binaryEncode` produces for
        // {join_ref:"7", ref:"", topic:"voice:c1", event:"audio", payload:[1,2,3]}.
        let mut bytes = vec![0u8, 1, 0, 8, 5];
        bytes.extend_from_slice(b"7");
        bytes.extend_from_slice(b"voice:c1");
        bytes.extend_from_slice(b"audio");
        bytes.extend_from_slice(&[1, 2, 3]);
        let f = parse_binary(&bytes).expect("a push");
        assert_eq!(
            f,
            BinaryFrame {
                join_ref: "7".into(),
                msg_ref: "".into(),
                topic: "voice:c1".into(),
                event: "audio".into(),
                payload: vec![1, 2, 3]
            }
        );
    }

    #[test]
    fn a_broadcast_is_what_decode_broadcast_reads() {
        let b = binary_broadcast("voice:c1", "audio", &[9, 9]);
        assert_eq!(&b[..3], &[2u8, 8, 5]);
        assert_eq!(&b[3..11], b"voice:c1");
        assert_eq!(&b[11..16], b"audio");
        assert_eq!(&b[16..], &[9, 9]);
        // The boundary itself, in both directions: 255 is the largest field a
        // single length byte can name, and it is exactly where the client's own
        // `assertFieldSize` throws (`> 255`). A guard written one off would drop
        // legal frames silently, so the pair is what pins it.
        assert!(!binary_broadcast(&"t".repeat(255), "audio", &[]).is_empty());
        assert!(binary_broadcast(&"t".repeat(256), "audio", &[]).is_empty());
    }

    #[test]
    fn a_short_or_foreign_binary_frame_is_refused() {
        assert!(parse_binary(&[]).is_none());
        assert!(
            parse_binary(&[2, 1, 1, b'a', b'b']).is_none(),
            "a broadcast is not a push"
        );
        assert!(
            parse_binary(&[0, 5, 0, 1, 1, b'x']).is_none(),
            "the header promises more than the buffer holds"
        );
    }

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
