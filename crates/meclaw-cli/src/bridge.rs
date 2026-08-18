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

/// Wire version of the JSON stdio format (`--stdio-format json`).
///
/// Asserted strictly in both directions from the very first line: the JSON
/// format is a versioned protocol surface, not a convenience. Note that this is
/// deliberately NOT the release version — a parent and a child may run different
/// MeClaw builds, which is the point of a sealed sub-colony.
pub const STDIO_PROTOCOL_VERSION: u64 = 1;

/// The boot-handshake frame, written once before stdin is read.
///
/// Two versions, on purpose. `v` is the wire protocol and is asserted strictly
/// by whoever drives this process — a mismatch means the two sides cannot talk
/// and there is nothing to negotiate yet. `version` is the release this binary
/// was built from and is REPORTED only: a parent and a sealed child colony are
/// expected to run different builds, and asserting on it would turn that
/// capability into a defect.
///
/// It is written only after the filesystem bootstrap succeeded, so its arrival
/// is the signal that this colony is actually able to answer.
pub fn ready_frame() -> Value {
    json!({
        "v": STDIO_PROTOCOL_VERSION,
        "type": "ready",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// The answer to a stdin line that could not be read as a request frame.
///
/// Written instead of staying quiet: whoever sent the line is a program waiting
/// on a correlation key, and silence would make it wait for its full timeout for
/// a reason it could have been told immediately.
pub fn ingress_error_frame(detail: &str) -> Value {
    json!({
        "v": STDIO_PROTOCOL_VERSION,
        "type": "error",
        "error_code": "invalid_frame",
        "detail": detail,
    })
}

/// One request frame read from a JSON stdin line.
///
/// Everything except `body` is optional: a caller that only has content behaves
/// exactly like the text format, and the envelope fields are the extra control
/// the JSON format exists for (parity with the HTTP ingress).
///
/// The two header compartments are named separately and never inferred from one
/// another: `context` is the persistent one (correlation), `hop` is the single
/// hop the caller asserts.
#[derive(Debug, Clone)]
pub struct IngressFrame {
    /// Carried, never regenerated — one conversation stays one trace across the
    /// process boundary.
    pub trace_id: Option<Uuid>,
    /// Already decremented by whoever crossed the boundary; `None` falls back to
    /// the substrate default.
    pub ttl: Option<u32>,
    /// The `headers.context` compartment for the source message.
    pub context: Map<String, Value>,
    /// The `headers.hop` compartment for the source message: the lane the caller
    /// ASSERTS, never one inferred from `context` (GH #180). Empty unless the
    /// frame said otherwise — a hive conditions its own `{"from": "."}` doors on
    /// `hop.route`, so without a way to say "hop" a frame addressed at a hive
    /// path matches no door at all.
    pub hop: Map<String, Value>,
    /// UBF body, carried verbatim — the JSON format synthesises nothing.
    pub body: Value,
}

/// Read one optional header compartment off an inbound frame: absent/`null` →
/// an empty map, an object → that object, anything else → `Err`.
///
/// Mirrors `validate_compartment` on the HTTP ingress (GH #175) so the two
/// ingresses answer a mistyped compartment the same way. A silent `{}` is the
/// failure mode both refuse: the sender gets an accepted frame for a compartment
/// it never set, and the consequence surfaces far from here — as a
/// `hive_no_route` dead letter for a dropped `hop`, or as an answer the sender
/// cannot attribute for a dropped `context` (GH #182), whose `turn_id` is the
/// correlation key. This is the only place that still knows what was written.
fn frame_compartment(field: &str, value: Option<&Value>) -> Result<Map<String, Value>, String> {
    match value {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(other) => Err(format!(
            "{field}: must be a JSON object, got {}",
            match other {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                _ => "value",
            }
        )),
    }
}

/// Read the optional `ttl` envelope field off an inbound frame: absent/`null` →
/// `None` (the substrate default applies), a positive integer in `1..=u32::MAX`
/// → that budget, anything else → `Err`.
///
/// Mirrors `validate_request_ttl` on the HTTP ingress, which has answered `422
/// invalid_ttl` for these inputs since 0.9.0. `ttl` is a scalar, so
/// [`frame_compartment`] does not fit, but the failure it prevents is the same
/// one: a string, a negative number or a float used to read as "no ttl" and the
/// frame ran on a budget the sender never chose. That surfaces far from here as
/// a message stopping mid-lane, which is among the harder things to trace back
/// to a typo in a frame.
///
/// The range check is not decoration. The previous `as u32` cast WRAPPED, so a
/// budget above `u32::MAX` became an unrelated small number — neither the value
/// sent nor the default, and the only inbound field that could silently shorten
/// a lane instead of failing.
fn frame_ttl(value: Option<&Value>) -> Result<Option<u32>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(x) if (1..=u32::MAX as u64).contains(&x) => Ok(Some(x as u32)),
            _ => Err(format!(
                "ttl: must be a positive integer in 1..={}, got {n}",
                u32::MAX
            )),
        },
        Some(other) => Err(format!("ttl: must be a positive integer, got {other}")),
    }
}

/// Read the optional `trace_id` envelope field off an inbound frame: absent/
/// `null` → `None` (a fresh trace is minted downstream), a UUID string → that
/// trace, anything else → `Err`.
///
/// The previous `and_then(Value::as_str)` gave one mistake two answers three
/// lines apart: `"trace_id": "nope"` was refused, `"trace_id": 12345` read as
/// "no trace_id at all" and the frame was accepted. What is lost then is not
/// visible here — `MessageBuilder` mints a fresh trace id, the message runs and
/// answers normally, and only the sender notices that the answer belongs to a
/// trace it never wrote, which is exactly the correlation the JSON wire exists
/// to preserve across the process boundary. The ingress is the last place that
/// still knows what the caller wrote.
///
/// Unlike `context`, `hop` and `ttl` this has no counterpart on the HTTP
/// ingress (`MessageRequest` carries no `trace_id`), so the rule it mirrors is
/// the local one: absent stays absent, present must be well-formed.
fn frame_trace_id(value: Option<&Value>) -> Result<Option<Uuid>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(
            Uuid::parse_str(s).map_err(|e| format!("trace_id: {e}"))?,
        )),
        Some(other) => Err(format!("trace_id: must be a UUID string, got {other}")),
    }
}

/// Parse one JSON stdin line into an [`IngressFrame`].
///
/// Strict on input, unlike the child-facing frame reader of the stdio-child
/// core: this is an ingress boundary, and a frame we do not fully understand
/// must not be half-executed. Every rejection names what it saw.
pub fn parse_ingress_frame(line: &str) -> Result<IngressFrame, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    let obj = v.as_object().ok_or("frame: must be a JSON object")?;
    match obj.get("v").and_then(Value::as_u64) {
        Some(STDIO_PROTOCOL_VERSION) => {}
        Some(other) => {
            return Err(format!(
                "v: unsupported protocol version {other} (this build speaks {STDIO_PROTOCOL_VERSION})"
            ));
        }
        None => {
            return Err(format!(
                "v: required (protocol version, {STDIO_PROTOCOL_VERSION})"
            ));
        }
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("message") => {}
        Some(other) => {
            return Err(format!(
                "type: unknown value {other:?} (known: \"message\")"
            ));
        }
        None => return Err("type: required (\"message\")".to_string()),
    }
    let body = obj
        .get("body")
        .filter(|b| b.is_object())
        .ok_or("body: required (UBF object)")?
        .clone();
    Ok(IngressFrame {
        trace_id: frame_trace_id(obj.get("trace_id"))?,
        ttl: frame_ttl(obj.get("ttl"))?,
        context: frame_compartment("context", obj.get("context"))?,
        hop: frame_compartment("hop", obj.get("hop"))?,
        body,
    })
}

/// Translate an [`IngressFrame`] into a source [`Message`] targeted at the root hive `/`.
///
/// The envelope fields the frame carries win over the substrate defaults — that
/// is the whole reason the JSON format exists. What the frame stays silent about
/// falls back to the text format's behaviour, so the two formats produce the
/// same shape of message and the topology below cannot tell them apart.
///
/// The context triad is filled in rather than overwritten: `turn_id` in
/// particular is the correlation key of whoever sent the frame, and replacing it
/// would break their ability to recognise the answer.
pub fn frame_to_message(frame: IngressFrame, user_id: Uuid, chat_id: Uuid) -> Message {
    let IngressFrame {
        trace_id,
        ttl,
        mut context,
        hop,
        body,
    } = frame;
    context
        .entry("user_id")
        .or_insert_with(|| json!(user_id.to_string()));
    context
        .entry("chat_id")
        .or_insert_with(|| json!(chat_id.to_string()));
    context
        .entry("turn_id")
        .or_insert_with(|| json!(Uuid::now_v7().to_string()));
    let mut builder = MessageBuilder::new(Path::new("/"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(body));
    if let Some(trace_id) = trace_id {
        builder = builder.trace_id(trace_id);
    }
    if let Some(ttl) = ttl {
        builder = builder.ttl(ttl);
    }
    builder.build()
}

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

/// Translate an egressing [`Message`] into one JSON stdout frame.
///
/// The whole body crosses, not just the last assistant turn: the text format has
/// to pick one line because a line is all it has, while a caller on the JSON
/// format is a program that wants what the topology actually produced.
///
/// `headers.context` crosses (it carries the correlation key back), `headers.hop`
/// does not — hop is a single-hop compartment and has no meaning on the far side.
///
/// A whole-body blob has no representation on this wire yet and becomes a typed
/// error frame; dropping it would leave the caller waiting for an answer that
/// silently never comes.
pub fn message_to_egress_frame(msg: &Message) -> Value {
    let turn_id = msg
        .headers
        .context
        .get("turn_id")
        .cloned()
        .unwrap_or(Value::Null);
    let Body::Inline(body) = &msg.body else {
        return json!({
            "v": STDIO_PROTOCOL_VERSION,
            "type": "error",
            "error_code": "blob_body_unsupported",
            "detail": "a whole-body blob cannot cross the stdio boundary (see docs/roadmap.md)",
            "trace_id": msg.trace_id.to_string(),
            "turn_id": turn_id,
        });
    };
    json!({
        "v": STDIO_PROTOCOL_VERSION,
        "type": "message",
        "trace_id": msg.trace_id.to_string(),
        "context": Value::Object(msg.headers.context.clone()),
        "body": body.clone(),
    })
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

    // --- P9 step A1: ingress frame parsing (JSON stdio format) ---

    #[test]
    fn a_well_formed_ingress_frame_parses() {
        let line = r#"{"v":1,"type":"message","body":{"messages":[]}}"#;
        let f = parse_ingress_frame(line).expect("must parse");
        assert!(f.trace_id.is_none(), "trace_id is optional");
        assert!(f.ttl.is_none(), "ttl is optional");
        assert!(f.context.is_empty(), "context defaults to empty");
        assert!(f.hop.is_empty(), "hop defaults to empty");
        assert!(f.body.get("messages").is_some(), "body is carried verbatim");
    }

    #[test]
    fn a_foreign_protocol_version_is_rejected_by_name() {
        let line = r#"{"v":2,"type":"message","body":{}}"#;
        let err = parse_ingress_frame(line).expect_err("must reject");
        assert!(err.contains('2'), "must name what it got: {err}");
        assert!(err.contains('1'), "must name what it expects: {err}");
    }

    #[test]
    fn a_missing_protocol_version_is_rejected() {
        let line = r#"{"type":"message","body":{}}"#;
        assert!(parse_ingress_frame(line).is_err(), "v is mandatory");
    }

    #[test]
    fn an_unknown_frame_type_is_rejected_by_name() {
        let line = r#"{"v":1,"type":"shutdown","body":{}}"#;
        let err = parse_ingress_frame(line).expect_err("must reject");
        assert!(err.contains("shutdown"), "must name the type: {err}");
    }

    #[test]
    fn a_frame_without_a_body_is_rejected() {
        let line = r#"{"v":1,"type":"message"}"#;
        assert!(parse_ingress_frame(line).is_err(), "body is mandatory");
    }

    #[test]
    fn a_non_json_line_is_rejected_rather_than_panicking() {
        assert!(parse_ingress_frame("hello world").is_err());
        assert!(parse_ingress_frame("[1,2,3]").is_err(), "must be an object");
    }

    #[test]
    fn envelope_fields_are_read_when_present() {
        let line = r#"{"v":1,"type":"message","trace_id":"018f0000-0000-7000-8000-000000000001",
            "ttl":7,"context":{"turn_id":"x"},"body":{"messages":[]}}"#;
        let f = parse_ingress_frame(line).expect("must parse");
        assert_eq!(f.ttl, Some(7));
        assert_eq!(
            f.trace_id.map(|t| t.to_string()).as_deref(),
            Some("018f0000-0000-7000-8000-000000000001")
        );
        assert_eq!(f.context["turn_id"], serde_json::json!("x"));
    }

    #[test]
    fn a_malformed_trace_id_is_rejected_rather_than_silently_dropped() {
        let line = r#"{"v":1,"type":"message","trace_id":"not-a-uuid","body":{}}"#;
        assert!(parse_ingress_frame(line).is_err());
    }

    // --- GH #180: the inbound `hop` seed ---

    #[test]
    fn an_inbound_hop_is_read_into_its_own_compartment() {
        let line = r#"{"v":1,"type":"message","hop":{"route":"search"},"body":{}}"#;
        let f = parse_ingress_frame(line).expect("must parse");
        assert_eq!(f.hop["route"], serde_json::json!("search"));
    }

    #[test]
    fn without_an_inbound_hop_the_compartment_is_empty() {
        let absent = parse_ingress_frame(r#"{"v":1,"type":"message","body":{}}"#).expect("parses");
        assert!(absent.hop.is_empty(), "absent hop is the historical shape");
        let null = parse_ingress_frame(r#"{"v":1,"type":"message","hop":null,"body":{}}"#)
            .expect("an explicit null is the same statement as saying nothing");
        assert!(null.hop.is_empty());
    }

    #[test]
    fn a_non_object_hop_is_rejected_rather_than_silently_dropped() {
        // A silent `{}` would reappear far from here as a `hive_no_route` dead
        // letter, and the sender has no way to trace that back to its frame.
        for bad in [
            r#"{"v":1,"type":"message","hop":"search","body":{}}"#,
            r#"{"v":1,"type":"message","hop":7,"body":{}}"#,
            r#"{"v":1,"type":"message","hop":["search"],"body":{}}"#,
            r#"{"v":1,"type":"message","hop":true,"body":{}}"#,
        ] {
            match parse_ingress_frame(bad) {
                Ok(f) => panic!("must reject {bad}, got {f:?}"),
                Err(err) => assert!(err.contains("hop"), "must name the field: {err}"),
            }
        }
    }

    // --- GH #182: the inbound `context` compartment is validated too ---

    #[test]
    fn a_non_object_context_is_rejected_rather_than_silently_dropped() {
        // `context` carries `turn_id`, the key the sender correlates the reply
        // on. Coerced to `{}` the frame still runs and still answers, and the
        // sender cannot tell whose answer arrived — the accepted frame is the
        // worse outcome, so the ingress refuses it where it still knows what
        // was written.
        for bad in [
            r#"{"v":1,"type":"message","context":"foo","body":{}}"#,
            r#"{"v":1,"type":"message","context":7,"body":{}}"#,
            r#"{"v":1,"type":"message","context":["turn_id"],"body":{}}"#,
            r#"{"v":1,"type":"message","context":true,"body":{}}"#,
        ] {
            match parse_ingress_frame(bad) {
                Ok(f) => panic!("must reject {bad}, got {f:?}"),
                Err(err) => assert!(err.contains("context"), "must name the field: {err}"),
            }
        }
    }

    #[test]
    fn without_an_inbound_context_the_compartment_is_empty() {
        // The tightening reaches mistyped frames only: the two ways of saying
        // nothing stay the historical shape a v1 sender relies on.
        let absent = parse_ingress_frame(r#"{"v":1,"type":"message","body":{}}"#).expect("parses");
        assert!(
            absent.context.is_empty(),
            "absent context is the historical shape"
        );
        let null = parse_ingress_frame(r#"{"v":1,"type":"message","context":null,"body":{}}"#)
            .expect("an explicit null is the same statement as saying nothing");
        assert!(null.context.is_empty());
    }

    // --- GH #187: the inbound `ttl` envelope field is validated too ---

    #[test]
    fn a_malformed_ttl_is_rejected_rather_than_silently_defaulted() {
        // `ttl` is the hop budget. Coerced to `None` the frame runs on the
        // colony default, and the sender only sees the consequence as a message
        // that stops somewhere mid-lane on a budget nobody asked for — nothing
        // there points back at the frame that mistyped it. `POST /messages`
        // answers 422 `invalid_ttl` for exactly these inputs and has since
        // 0.9.0, so the stdio ingress must not be the lenient one.
        for bad in [
            r#"{"v":1,"type":"message","ttl":"12","body":{}}"#,
            r#"{"v":1,"type":"message","ttl":-1,"body":{}}"#,
            r#"{"v":1,"type":"message","ttl":3.5,"body":{}}"#,
            r#"{"v":1,"type":"message","ttl":true,"body":{}}"#,
            r#"{"v":1,"type":"message","ttl":[7],"body":{}}"#,
            r#"{"v":1,"type":"message","ttl":{"n":7},"body":{}}"#,
            // Zero is a budget no hop can be spent from; the HTTP ingress
            // refuses it as well rather than letting it read as "no ttl".
            r#"{"v":1,"type":"message","ttl":0,"body":{}}"#,
        ] {
            match parse_ingress_frame(bad) {
                Ok(f) => panic!("must reject {bad}, got {f:?}"),
                Err(err) => assert!(err.contains("ttl"), "must name the field: {err}"),
            }
        }
    }

    #[test]
    fn a_ttl_above_the_u32_range_is_rejected_rather_than_truncated() {
        // The quiet half of the same defect: `as u32` wraps, so a budget above
        // `u32::MAX` used to arrive as an unrelated small number — neither the
        // value sent nor the default, and the only ingress input that could
        // silently shorten a lane instead of lengthening it.
        for bad in [
            r#"{"v":1,"type":"message","ttl":4294967296,"body":{}}"#,
            r#"{"v":1,"type":"message","ttl":18446744073709551615,"body":{}}"#,
        ] {
            match parse_ingress_frame(bad) {
                Ok(f) => panic!("must reject {bad}, got {f:?}"),
                Err(err) => assert!(err.contains("ttl"), "must name the field: {err}"),
            }
        }
        let max = parse_ingress_frame(r#"{"v":1,"type":"message","ttl":4294967295,"body":{}}"#)
            .expect("the top of the range is still a legal budget");
        assert_eq!(max.ttl, Some(u32::MAX));
    }

    #[test]
    fn without_an_inbound_ttl_the_envelope_stays_absent() {
        // The tightening reaches malformed frames only: both ways of saying
        // nothing keep meaning "fall back to the substrate default".
        let absent = parse_ingress_frame(r#"{"v":1,"type":"message","body":{}}"#).expect("parses");
        assert!(absent.ttl.is_none(), "absent ttl is the historical shape");
        let null = parse_ingress_frame(r#"{"v":1,"type":"message","ttl":null,"body":{}}"#)
            .expect("an explicit null is the same statement as saying nothing");
        assert!(null.ttl.is_none());
    }

    // --- P9 step A2: ingress frame -> source message ---

    fn frame(json: serde_json::Value) -> IngressFrame {
        parse_ingress_frame(&json.to_string()).expect("fixture frame must parse")
    }

    #[test]
    fn the_parent_trace_id_is_carried_not_regenerated() {
        let trace = Uuid::now_v7();
        let f = frame(serde_json::json!({
            "v": 1, "type": "message", "trace_id": trace.to_string(), "body": {"messages": []}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.trace_id, trace, "the trace must survive the boundary");
        assert_ne!(msg.id, trace, "the message still gets its own id");
    }

    #[test]
    fn without_a_trace_id_the_message_starts_its_own_trace() {
        let f = frame(serde_json::json!({"v": 1, "type": "message", "body": {}}));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.trace_id, msg.id, "substrate default: trace == own id");
    }

    #[test]
    fn the_frame_ttl_lands_in_the_envelope() {
        let f = frame(serde_json::json!({"v": 1, "type": "message", "ttl": 3, "body": {}}));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.ttl, 3, "a decremented boundary TTL must be honoured");
    }

    #[test]
    fn without_a_ttl_the_substrate_default_applies() {
        let f = frame(serde_json::json!({"v": 1, "type": "message", "body": {}}));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.ttl, meclaw_core::MESSAGE_DEFAULT_TTL);
    }

    #[test]
    fn the_body_crosses_verbatim_without_a_synthesised_turn() {
        let f = frame(serde_json::json!({
            "v": 1, "type": "message",
            "body": {"system": "be brief", "messages": [{"origin": "user", "type": "text", "text": "hi"}]}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.target, Path::new("/"));
        let Body::Inline(v) = &msg.body else {
            panic!("inline body expected")
        };
        assert_eq!(v["system"], "be brief", "the whole body is carried");
        assert_eq!(v["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(v["messages"][0]["text"], "hi");
    }

    #[test]
    fn the_context_triad_is_filled_in_where_the_frame_is_silent() {
        let chat = Uuid::now_v7();
        let f = frame(serde_json::json!({"v": 1, "type": "message", "body": {}}));
        let msg = frame_to_message(f, STDIO_USER_ID, chat);
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
    fn the_frame_context_wins_over_the_defaults() {
        let f = frame(serde_json::json!({
            "v": 1, "type": "message",
            "context": {"user_id": "caller-supplied", "turn_id": "correlation-key"},
            "body": {}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.headers.context["user_id"], "caller-supplied");
        assert_eq!(
            msg.headers.context["turn_id"], "correlation-key",
            "the turn_id is the correlation key — it must never be overwritten"
        );
    }

    #[test]
    fn the_frame_context_reaches_the_message_untouched() {
        let f = frame(serde_json::json!({
            "v": 1, "type": "message", "context": {"locale": "de-DE"}, "body": {}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.headers.context["locale"], "de-DE");
        assert!(
            msg.headers.hop.is_empty(),
            "a frame that asserts no lane still starts with an empty hop"
        );
    }

    #[test]
    fn the_frame_hop_reaches_the_message_verbatim() {
        // GH #180: a hive distributes internally over `{"from": "."}` edges that
        // condition on `hop.route` — without this, a stdio-driven colony cannot
        // address a hive at all and every such frame dead-letters.
        let f = frame(serde_json::json!({
            "v": 1, "type": "message", "hop": {"route": "search", "depth": 2}, "body": {}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert_eq!(msg.headers.hop["route"], "search");
        assert_eq!(msg.headers.hop["depth"], 2, "the seed is carried verbatim");
    }

    #[test]
    fn the_two_inbound_compartments_do_not_cross() {
        let f = frame(serde_json::json!({
            "v": 1, "type": "message",
            "context": {"locale": "de-DE"}, "hop": {"route": "search"},
            "body": {}
        }));
        let msg = frame_to_message(f, STDIO_USER_ID, Uuid::now_v7());
        assert!(
            !msg.headers.hop.contains_key("locale"),
            "a context key must never be inferred into the hop"
        );
        assert!(
            !msg.headers.context.contains_key("route"),
            "a hop key must never leak into the persistent compartment"
        );
    }

    // --- P9 step A3: message -> egress frame ---

    #[test]
    fn an_egress_frame_carries_trace_context_and_body() {
        let trace = Uuid::now_v7();
        let mut ctx: Map<String, Value> = Map::new();
        ctx.insert("turn_id".into(), json!("correlation-key"));
        let msg = MessageBuilder::new(Path::new("/"))
            .trace_id(trace)
            .context(ctx)
            .body(Body::Inline(
                json!({"messages": [{"origin": "assistant", "type": "text", "text": "pong"}]}),
            ))
            .build();
        let f = message_to_egress_frame(&msg);
        assert_eq!(f["v"], json!(STDIO_PROTOCOL_VERSION));
        assert_eq!(f["type"], "message");
        assert_eq!(f["trace_id"], json!(trace.to_string()));
        assert_eq!(
            f["context"]["turn_id"], "correlation-key",
            "the correlation key must come back or nobody can match the answer"
        );
        assert_eq!(f["body"]["messages"][0]["text"], "pong");
    }

    #[test]
    fn the_egress_frame_carries_the_whole_body_not_just_the_last_turn() {
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "question"},
                {"origin": "assistant", "type": "text", "text": "answer"}
            ]})))
            .build();
        let f = message_to_egress_frame(&msg);
        assert_eq!(
            f["body"]["messages"].as_array().map(Vec::len),
            Some(2),
            "unlike the text format, JSON egress is not lossy"
        );
    }

    #[test]
    fn a_blob_body_becomes_an_error_frame_rather_than_a_silent_drop() {
        let msg = MessageBuilder::new(Path::new("/"))
            .body(Body::Blob(Uuid::now_v7()))
            .build();
        let f = message_to_egress_frame(&msg);
        assert_eq!(f["type"], "error");
        assert_eq!(f["error_code"], "blob_body_unsupported");
        assert!(
            f["detail"].as_str().is_some_and(|d| !d.is_empty()),
            "an error frame must say what happened"
        );
    }

    #[test]
    fn the_hop_compartment_does_not_cross_the_boundary() {
        let mut hop: Map<String, Value> = Map::new();
        hop.insert("finish_reason".into(), json!("assistant"));
        let msg = MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build();
        let f = message_to_egress_frame(&msg);
        assert!(
            f.get("hop").is_none(),
            "hop is a single-hop compartment by definition"
        );
        assert!(
            f["context"].as_object().is_some_and(Map::is_empty),
            "an empty context stays empty"
        );
    }

    // --- P9 step A4: the boot handshake frame ---

    #[test]
    fn the_ready_frame_announces_the_protocol_and_the_build() {
        let f = ready_frame();
        assert_eq!(f["type"], "ready");
        assert_eq!(
            f["v"],
            json!(STDIO_PROTOCOL_VERSION),
            "the protocol integer is what a parent asserts strictly"
        );
        assert_eq!(
            f["version"],
            json!(env!("CARGO_PKG_VERSION")),
            "the release version is reported, never asserted — skew is a feature"
        );
    }

    #[test]
    fn the_release_version_is_not_the_protocol_version() {
        // Discriminator: were the two ever conflated, a parent asserting the
        // protocol would reject every child of a different build — which is
        // exactly the sealed-sub-colony capability this must not destroy.
        let f = ready_frame();
        assert_ne!(
            f["version"], f["v"],
            "release and protocol version must stay separate fields"
        );
    }

    #[test]
    fn the_ready_frame_is_one_line_of_json() {
        let line = serde_json::to_string(&ready_frame()).expect("serialises");
        assert!(!line.contains('\n'), "a frame is exactly one line");
        assert!(
            parse_ingress_frame(&line).is_err(),
            "ready is not a request"
        );
    }

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
