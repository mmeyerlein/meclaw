//! The two frames that cross between colonies, and the eleven codes a crossing
//! can fail with.
//!
//! GH #1012: the delivery id (a UUIDv7 the sender sets once and keeps across
//! retries) and the sender's clock travel as the request headers
//! [`FRAME_ID_HEADER`] and [`SENT_MS_HEADER`], both optional; the `message`
//! frame itself is unchanged since 0.47.1. A receipt may carry
//! `duplicate: true`, and only to a request that carried an id.

use meclaw_core::{Message, Uuid};
use serde::Deserialize;
use serde_json::{Map, Value, Value as JsonValue, json};

use super::lanes::Refusal;
use super::params::Lane;

// The eleven codes. `protocol_mismatch`, `invalid_frame` and `ttl_exhausted` are
// deliberately the words of `subcolony/wire.rs` and `meclaw-cli/src/bridge.rs`:
// a boundary that renamed them would make an operator learn the same failure
// twice. The set is closed and disjoint from the five Telegram codes at the
// chat exit.

/// This side declares no lane of that name in that direction.
pub const LANE_UNDECLARED: &str = "lane_undeclared";
/// The body carries a field the lane does not name.
pub const LANE_FIELD_DENIED: &str = "lane_field_denied";
/// A whole-body blob, `attachments[]`, a body that is not inline, or a turn in
/// `messages[]` that is a blob reference (`text_id`, `messages_id`, GH #839):
/// the receiving colony would resolve it against its own store.
pub const LANE_BODY_UNSUPPORTED: &str = "lane_body_unsupported";
/// The carrier failed: no connection, no answer, or a non-200.
pub const PEER_UNREACHABLE: &str = "peer_unreachable";
/// No receipt within `external_timeout_ms`.
pub const PEER_TIMEOUT: &str = "peer_timeout";
/// The far side refused; its code and its boundary are in the detail.
pub const PEER_REFUSED: &str = "peer_refused";
/// A foreign protocol integer, or a frame without one.
pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
/// A frame this build cannot read, including one that names its sender.
pub const INVALID_FRAME: &str = "invalid_frame";
/// No hops left; the crossing would have been one more.
pub const TTL_EXHAUSTED: &str = "ttl_exhausted";
/// GH #828: the credential could not be obtained (the token endpoint did not
/// answer, or answered no token), so the frame never left this side.
pub const AUTH_UNAVAILABLE: &str = "auth_unavailable";
/// GH #840: `hop.peer_url` names an origin outside `params.egress`, carries
/// credentials, is not `http(s)`, or the cell declares no `egress` at all
/// (fail-closed). Decided before any token request and any connect: a policy
/// verdict, not a carrier failure, so never `peer_unreachable`.
pub const EGRESS_DENIED: &str = "egress_denied";

/// GH #1012: the outbox gave up after `peer_retry_deadline_s`. The code of
/// the `expired` receipt and of the dead letter the message becomes.
pub const PEER_EXPIRED: &str = "peer_expired";

/// GH #1012: a stored carrier code back to its constant. Only the two
/// retried codes are ever stored; anything else reads as `peer_unreachable`.
pub fn code_of(code: &str) -> &'static str {
    match code {
        PEER_TIMEOUT => PEER_TIMEOUT,
        _ => PEER_UNREACHABLE,
    }
}

/// GH #1012: the request header carrying the sender's delivery id (a
/// UUIDv7, stable across its retries). A header, never a frame key: the
/// frame body stays byte-identical to 0.47.1, whose parser refuses unknown
/// keys, and a receiver before #1012 ignores headers it does not read
/// (review C1, R-HV-3). Hyphens, because a reverse proxy may drop
/// underscore names.
pub const FRAME_ID_HEADER: &str = "X-Meclaw-Frame-Id";

/// GH #1012: the request header carrying the sender's clock (Unix ms) when it
/// booked the message; the same on every retry.
pub const SENT_MS_HEADER: &str = "X-Meclaw-Sent-Ms";

/// GH #1012: reads the two delivery headers. A missing header is `None` (a
/// sender before #1012: accepted, never deduplicated); a present one that
/// does not parse is `invalid_frame`.
pub fn read_delivery(
    frame_id: Option<&str>,
    sent_ms: Option<&str>,
) -> Result<(Option<Uuid>, Option<u64>), Refusal> {
    let id = match frame_id {
        None => None,
        Some(raw) => Some(Uuid::parse_str(raw).map_err(|e| {
            Refusal::new(
                INVALID_FRAME,
                format!("header {FRAME_ID_HEADER}: must be a UUID, got {raw:?} ({e})"),
            )
        })?),
    };
    let sent = match sent_ms {
        None => None,
        Some(raw) => Some(raw.parse::<u64>().map_err(|e| {
            Refusal::new(
                INVALID_FRAME,
                format!("header {SENT_MS_HEADER}: must be Unix ms, got {raw:?} ({e})"),
            )
        })?),
    };
    Ok((id, sent))
}

/// The protocol integer every frame carries as `v`. This build speaks exactly one.
pub const PROTOCOL_VERSION: u64 = 1;

/// A `message` frame as it arrived, read strictly and not yet judged against a lane.
#[derive(Debug, Clone)]
pub struct InboundFrame {
    /// The lane the frame claims to travel on.
    pub lane: String,
    /// The trace the message belongs to, carried unchanged across the boundary.
    pub trace_id: Uuid,
    /// The hops left after the crossing; 0 parses and is refused by the mount.
    pub ttl: u32,
    /// The projected `context` the sending side let cross.
    pub context: Map<String, Value>,
    /// The projected body, an inline JSON object.
    pub body: Value,
}

/// The wire form of a `message` frame. `deny_unknown_fields` is the whole of the
/// sender rule R-26-18 on the parser side: a frame with `sender`, `peer`,
/// `from` or any other key is `invalid_frame`, so no later reader can start to
/// honour an address a peer wrote into a frame.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageFrame {
    // Checked by `check_version` on the raw value before this struct is built;
    // declared so the key is known rather than refused.
    #[allow(dead_code)]
    v: u64,
    #[serde(rename = "type")]
    frame_type: String,
    lane: String,
    trace_id: String,
    ttl: u32,
    context: Map<String, Value>,
    body: Value,
}

/// The wire form of a `receipt` frame, read as strictly as a message.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptFrame {
    // Checked by `check_version` on the raw value before this struct is built.
    #[allow(dead_code)]
    v: u64,
    #[serde(rename = "type")]
    frame_type: String,
    result: String,
    // Known keys of a crossed receipt; the writer's verdict needs only
    // `result`, but an unknown key must stay a refusal.
    #[allow(dead_code)]
    #[serde(default)]
    lane: String,
    #[serde(default)]
    boundary: String,
    #[allow(dead_code)]
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    error_code: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    because: Option<String>,
    // GH #1012: a repeated frame id is answered `crossed` with this flag; the
    // sender books it as the success it is.
    #[allow(dead_code)]
    #[serde(default)]
    duplicate: bool,
}

/// The strict `v` test shared by both parsers: the protocol integer first and
/// alone, before any other key is looked at.
fn check_version(v: &JsonValue, what: &str) -> Result<(), Refusal> {
    let Some(obj) = v.as_object() else {
        return Err(Refusal::new(
            INVALID_FRAME,
            format!("frame: must be a JSON object, got {v}"),
        ));
    };
    let Some(raw) = obj.get("v") else {
        return Err(Refusal::new(
            PROTOCOL_MISMATCH,
            format!("{what} without a protocol version"),
        ));
    };
    match raw.as_u64() {
        Some(PROTOCOL_VERSION) => Ok(()),
        Some(other) => Err(Refusal::new(
            PROTOCOL_MISMATCH,
            format!("{what} in protocol {other}, this build speaks {PROTOCOL_VERSION}"),
        )),
        // Same code as a missing `v` (OR-Peer.L1a.4), but the detail names the
        // value an operator has to fix instead of calling it absent.
        None => Err(Refusal::new(
            PROTOCOL_MISMATCH,
            format!(
                "{what} whose `v` is not a protocol integer, got {raw}; \
                 this build speaks {PROTOCOL_VERSION}"
            ),
        )),
    }
}

/// Builds the `message` frame for a declared crossing. TTL first: a message
/// with no hops left may not buy one by changing colony. The frame carries no
/// `hop` and no address, only the lane.
pub fn message_frame(
    lane: &Lane,
    msg: &Message,
    projected_body: Value,
    projected_context: Map<String, Value>,
) -> Result<JsonValue, Refusal> {
    if msg.ttl == 0 {
        return Err(Refusal::new(
            TTL_EXHAUSTED,
            "the message has no hops left; crossing the boundary would have been one more",
        ));
    }
    Ok(json!({
        "v": PROTOCOL_VERSION,
        "type": "message",
        "lane": lane.route,
        "trace_id": msg.trace_id.to_string(),
        "ttl": msg.ttl - 1,
        "context": projected_context,
        "body": projected_body,
    }))
}

/// Reads an arriving `message` frame strictly: `v` first (`protocol_mismatch`),
/// everything else malformed is `invalid_frame`. A `ttl` of 0 parses: the mount
/// refuses it with `ttl_exhausted`, the code an operator has to read.
pub fn parse_message_frame(v: &JsonValue) -> Result<InboundFrame, Refusal> {
    check_version(v, "the peer sent a frame")?;
    let f = MessageFrame::deserialize(v)
        .map_err(|e| Refusal::new(INVALID_FRAME, format!("frame: {e}")))?;
    if f.frame_type != "message" {
        return Err(Refusal::new(
            INVALID_FRAME,
            format!(
                "type: unknown value {:?} (known: \"message\")",
                f.frame_type
            ),
        ));
    }
    if f.lane.is_empty() {
        return Err(Refusal::new(
            INVALID_FRAME,
            "lane: required (the name of the lane this frame travels on)",
        ));
    }
    let trace_id = Uuid::parse_str(&f.trace_id).map_err(|e| {
        Refusal::new(
            INVALID_FRAME,
            format!("trace_id: must be a UUID, got {:?} ({e})", f.trace_id),
        )
    })?;
    if !f.body.is_object() {
        return Err(Refusal::new(
            INVALID_FRAME,
            format!("body: must be a UBF object, got {}", f.body),
        ));
    }
    Ok(InboundFrame {
        lane: f.lane,
        trace_id,
        ttl: f.ttl,
        context: f.context,
        body: f.body,
    })
}

/// The receipt for a crossing: which lane, at which boundary, carrying which fields.
pub fn crossed_receipt(lane: &str, boundary: &str, fields: &[String]) -> JsonValue {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "receipt",
        "result": "crossed",
        "lane": lane,
        "boundary": boundary,
        "fields": fields,
    })
}

/// The receipt for a refusal: the code, the detail and, when the lane is known,
/// what the lane is for.
pub fn refused_receipt(
    lane: &str,
    boundary: &str,
    r: &Refusal,
    because: Option<&str>,
) -> JsonValue {
    let mut out = json!({
        "v": PROTOCOL_VERSION,
        "type": "receipt",
        "result": "refused",
        "lane": lane,
        "boundary": boundary,
        "error_code": r.error_code,
        "detail": r.detail,
    });
    if let (Some(because), Some(obj)) = (because, out.as_object_mut()) {
        obj.insert("because".to_string(), Value::String(because.to_string()));
    }
    out
}

/// Reads the far side's receipt: `crossed` is `Ok`, `refused` is ONE
/// `peer_refused` naming the far boundary and its code. The writer learns that
/// it failed and where, not the foreign colony's vocabulary as if it were its own.
pub fn read_receipt(v: &JsonValue) -> Result<(), Refusal> {
    check_version(v, "the peer answered")?;
    let f = ReceiptFrame::deserialize(v)
        .map_err(|e| Refusal::new(INVALID_FRAME, format!("receipt: {e}")))?;
    if f.frame_type != "receipt" {
        return Err(Refusal::new(
            INVALID_FRAME,
            format!(
                "type: unknown value {:?} (known: \"receipt\")",
                f.frame_type
            ),
        ));
    }
    match f.result.as_str() {
        "crossed" => Ok(()),
        "refused" => {
            // A thin receipt still parses (every field has a default); the
            // detail says what is missing instead of printing empty quotes.
            let boundary = if f.boundary.is_empty() {
                "an unnamed boundary".to_string()
            } else {
                format!("the boundary `{}`", f.boundary)
            };
            let code = quoted_or(&f.error_code, "no error_code");
            let reason = quoted_or(&f.detail, "no detail");
            let mut detail = format!("{boundary} refused with {code}: {reason}");
            if let Some(because) = f.because {
                detail.push_str(&format!(" (that lane is for: `{because}`)"));
            }
            Err(Refusal::new(PEER_REFUSED, detail))
        }
        other => Err(Refusal::new(
            INVALID_FRAME,
            format!("result: unknown value {other:?} (known: \"crossed\", \"refused\")"),
        )),
    }
}

/// `` `value` `` when `value` is set, else the plain `missing` phrase.
fn quoted_or(value: &str, missing: &str) -> String {
    if value.is_empty() {
        missing.to_string()
    } else {
        format!("`{value}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(extra: Value) -> Value {
        let mut f = json!({"v": 1, "type": "message", "lane": "topic",
            "trace_id": "0192f1b0-0000-7000-8000-000000000001", "ttl": 5, "context": {},
            "body": {"messages": []}});
        if let (Some(f), Some(extra)) = (f.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                f.insert(k.clone(), v.clone());
            }
        }
        f
    }

    /// Review C1 (R-HV-3): the frame is the 0.47.1 frame. `id` and
    /// `sent_ms` as frame keys are unknown fields, refused like any other,
    /// so the parser of this build and of a receiver before #1012 agree.
    #[test]
    fn delivery_keys_in_the_frame_are_an_invalid_frame() {
        let id = "0192f1b0-0000-7000-8000-0000000000aa";
        assert!(parse_message_frame(&raw(json!({}))).is_ok());
        for extra in [json!({"id": id}), json!({"sent_ms": 42})] {
            let r = parse_message_frame(&raw(extra.clone())).expect_err("refused");
            assert_eq!(r.error_code, INVALID_FRAME, "{extra}");
        }
    }

    /// What one header case expects: the id and clock, or the refusal code.
    type Want = Result<(Option<&'static str>, Option<u64>), &'static str>;
    /// One header case: the two header values as sent.
    type Sent = (Option<&'static str>, Option<&'static str>);

    /// GH #1012 part a, review C1: both delivery headers are optional; a
    /// present id must be a UUID, a present clock an unsigned integer.
    #[test]
    fn the_delivery_headers_read_back_or_refuse() {
        let id = "0192f1b0-0000-7000-8000-0000000000aa";
        let cases: [(Sent, Want); 6] = [
            ((None, None), Ok((None, None))),
            ((Some(id), None), Ok((Some(id), None))),
            (
                (Some(id), Some("1700000000123")),
                Ok((Some(id), Some(1_700_000_000_123))),
            ),
            ((Some("not-a-uuid"), None), Err(INVALID_FRAME)),
            ((None, Some("soon")), Err(INVALID_FRAME)),
            ((None, Some("-1")), Err(INVALID_FRAME)),
        ];
        for ((hid, hms), want) in cases {
            match (read_delivery(hid, hms), want) {
                (Ok((got_id, got_ms)), Ok((id, ms))) => {
                    assert_eq!(got_id.map(|u| u.to_string()).as_deref(), id, "{hid:?}");
                    assert_eq!(got_ms, ms, "{hms:?}");
                }
                (Err(r), Err(code)) => assert_eq!(r.error_code, code, "{hid:?} {hms:?}"),
                (got, want) => panic!("{hid:?} {hms:?}: got {got:?}, want {want:?}"),
            }
        }
    }

    /// GH #1012 part b: a duplicate answer still reads as a crossing, so a
    /// sender that repeats after a lost answer books one success.
    #[test]
    fn a_duplicate_receipt_reads_as_crossed() {
        let mut r = crossed_receipt("topic", "south", &[]);
        r["duplicate"] = json!(true);
        assert!(read_receipt(&r).is_ok(), "{r}");
    }
}
