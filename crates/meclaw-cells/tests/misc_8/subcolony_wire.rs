//! P9 steps C4–C6 — what crosses the composition boundary, and what does not.
//!
//! These are the tests the whole package stands on. The boundary has to carry a
//! trace (so one conversation stays traceable across two message logs), has to
//! decrement a TTL (so a sub-colony cycle dies like any routing cycle), and has
//! to carry NOTHING else that was not declared.

use meclaw_cells::subcolony::{SubcolonyParams, wire};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use serde_json::{Map, Value, json};

fn params(extra: Value) -> SubcolonyParams {
    let td = tempfile::TempDir::new().expect("tempdir");
    // Leak the tempdir so the canonicalised root stays alive for the test.
    let root = td.keep();
    let mut v = json!({"root": root.to_string_lossy()});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in e {
            o.insert(k.clone(), val.clone());
        }
    }
    SubcolonyParams::parse(&v).expect("params must parse")
}

fn inbound(ttl: u32, context: Value) -> Message {
    let mut ctx: Map<String, Value> = Map::new();
    if let Some(o) = context.as_object() {
        ctx = o.clone();
    }
    MessageBuilder::new(Path::new("/child"))
        .ttl(ttl)
        .context(ctx)
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
        ))
        .build()
}

// --- C4: the request frame ---

#[test]
fn the_trace_id_is_carried_not_regenerated() {
    let msg = inbound(10, json!({}));
    let f = wire::request_frame(&msg, &params(json!({})), "k1").expect("must build");
    assert_eq!(
        f["trace_id"],
        json!(msg.trace_id.to_string()),
        "one conversation, one trace, across colonies"
    );
}

#[test]
fn the_ttl_is_decremented_across_the_boundary() {
    let msg = inbound(10, json!({}));
    let f = wire::request_frame(&msg, &params(json!({})), "k1").expect("must build");
    assert_eq!(
        f["ttl"], 9,
        "crossing into another colony is a hop and must cost one"
    );
}

#[test]
fn an_exhausted_ttl_refuses_the_crossing_instead_of_making_a_new_hop() {
    // The loop guard. A sub-colony cycle has to die exactly like any routing
    // cycle, and it can only do so if the boundary refuses at zero.
    let msg = inbound(0, json!({}));
    let err = wire::request_frame(&msg, &params(json!({})), "k1").expect_err("must refuse");
    assert_eq!(err.error_code, "ttl_exhausted");
}

#[test]
fn nothing_of_the_parent_context_crosses_by_default() {
    let msg = inbound(
        10,
        json!({"user_id": "u-1", "chat_id": "c-1", "secret_hint": "s-1"}),
    );
    let f = wire::request_frame(&msg, &params(json!({})), "k1").expect("must build");
    let ctx = f["context"].as_object().expect("context object");
    assert_eq!(
        ctx.len(),
        1,
        "only the correlation key crosses by default; got {ctx:?}"
    );
    assert_eq!(ctx["turn_id"], "k1");
}

#[test]
fn only_declared_context_keys_cross_and_may_be_renamed() {
    let msg = inbound(
        10,
        json!({"user_id": "u-1", "chat_id": "c-1", "secret_hint": "s-1"}),
    );
    let p = params(json!({"context_in": {"user_id": "user_id", "chat_id": "conversation"}}));
    let f = wire::request_frame(&msg, &p, "k1").expect("must build");
    let ctx = f["context"].as_object().expect("context object");
    assert_eq!(ctx["user_id"], "u-1");
    assert_eq!(ctx["conversation"], "c-1", "renamed on the way in");
    assert!(
        !ctx.contains_key("secret_hint"),
        "an undeclared key must not cross: {ctx:?}"
    );
}

#[test]
fn a_declared_key_the_message_does_not_have_is_simply_absent() {
    let msg = inbound(10, json!({}));
    let p = params(json!({"context_in": {"user_id": "user_id"}}));
    let f = wire::request_frame(&msg, &p, "k1").expect("must build");
    assert!(f["context"].get("user_id").is_none());
}

#[test]
fn the_hop_compartment_never_crosses() {
    let mut hop: Map<String, Value> = Map::new();
    hop.insert("finish_reason".into(), json!("assistant"));
    let msg = MessageBuilder::new(Path::new("/child"))
        .ttl(10)
        .hop(hop)
        .body(Body::Inline(json!({"messages": []})))
        .build();
    let f = wire::request_frame(&msg, &params(json!({})), "k1").expect("must build");
    assert!(f.get("hop").is_none(), "hop is single-hop by definition");
    let ctx = f["context"].as_object().expect("context object");
    assert!(
        !ctx.contains_key("finish_reason"),
        "and it is not smuggled into context"
    );
}

#[test]
fn the_body_crosses_verbatim() {
    let msg = inbound(10, json!({}));
    let f = wire::request_frame(&msg, &params(json!({})), "k1").expect("must build");
    assert_eq!(f["body"]["messages"][0]["text"], "ping");
    assert_eq!(f["v"], 1, "the frame declares its protocol");
    assert_eq!(f["type"], "message");
}

#[test]
fn a_body_without_turns_is_refused_before_the_child_is_bothered() {
    let msg = MessageBuilder::new(Path::new("/child"))
        .ttl(10)
        .body(Body::Inline(json!({"note": "no messages here"})))
        .build();
    let err = wire::request_frame(&msg, &params(json!({})), "k1").expect_err("must refuse");
    assert_eq!(err.error_code, "invalid_input");
}

#[test]
fn a_blob_body_is_refused_at_the_boundary() {
    // Any v7 uuid will do; `uuid` is not a dependency of this crate, so one is
    // borrowed from a throwaway message rather than added to the manifest.
    let blob_id = MessageBuilder::new(Path::new("/probe")).build().id;
    let msg = MessageBuilder::new(Path::new("/child"))
        .ttl(10)
        .body(Body::Blob(blob_id))
        .build();
    let err = wire::request_frame(&msg, &params(json!({})), "k1").expect_err("must refuse");
    assert_eq!(err.error_code, "invalid_input");
}

// --- C6: reading the reply ---

#[test]
fn a_reply_frame_yields_the_child_body() {
    let frame = json!({
        "v": 1, "type": "message",
        "body": {"messages": [{"origin": "assistant", "type": "text", "text": "pong"}]}
    });
    let body = wire::reply_body(&frame).expect("must read");
    assert_eq!(body["messages"][0]["text"], "pong");
}

#[test]
fn the_child_context_does_not_come_back_into_the_parent_tree() {
    // Opacity in the other direction: the answer travels in the parent's own
    // trace, and the child's internal context is the child's business.
    let frame = json!({
        "v": 1, "type": "message",
        "context": {"turn_id": "k1", "child_internal": "x"},
        "body": {"messages": []}
    });
    let body = wire::reply_body(&frame).expect("must read");
    assert!(
        body.get("child_internal").is_none() && body.get("context").is_none(),
        "no child context leaks into the emitted body: {body}"
    );
}

#[test]
fn an_error_frame_from_the_child_becomes_a_typed_failure() {
    let frame = json!({
        "v": 1, "type": "error", "error_code": "invalid_frame", "detail": "body: required"
    });
    let err = wire::reply_body(&frame).expect_err("must fail");
    assert_eq!(err.error_code, "child_error");
    assert!(
        err.detail.contains("body: required"),
        "the child's reason must survive: {}",
        err.detail
    );
}

#[test]
fn a_reply_with_a_foreign_protocol_version_is_refused() {
    let frame = json!({"v": 2, "type": "message", "body": {"messages": []}});
    let err = wire::reply_body(&frame).expect_err("must fail");
    assert_eq!(err.error_code, "protocol_mismatch");
}

#[test]
fn the_correlation_key_is_read_from_a_frame() {
    let frame = json!({"v": 1, "type": "message", "context": {"turn_id": "k7"}, "body": {}});
    assert_eq!(wire::correlation_key(&frame).as_deref(), Some("k7"));
    let spontaneous = json!({"v": 1, "type": "message", "context": {}, "body": {}});
    assert!(
        wire::correlation_key(&spontaneous).is_none(),
        "an unsolicited frame has no key"
    );
}
