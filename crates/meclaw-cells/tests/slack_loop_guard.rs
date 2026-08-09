//! P12 S6/S7: event extraction and the stateless half of the bot-loop guard.
//!
//! Rules R1–R4 are pure functions of one event, so they are proven here without
//! a socket, a database or a topology. R5 (channel message without mention or
//! thread ownership) needs persisted ownership state and is proven at the
//! handler level in a later step.
//!
//! The sharpest test in this file is
//! `own_api_app_id_alone_never_drops_an_event`: `api_app_id` names the app that
//! RECEIVES the event, so it equals our own app id on literally every inbound
//! event. Wiring R3 to it would silently discard all traffic and present as "the
//! bot is deaf" with a perfectly healthy socket.

use meclaw_cells::proxy::slack::wire::{
    LoopDrop, SlackEventKind, loop_drop_reason, parse_user_event,
};
use serde_json::{Value as JsonValue, json};

/// Builds an `event_callback` payload around one inner event.
fn payload(event: JsonValue) -> JsonValue {
    json!({
        "type": "event_callback",
        "api_app_id": "A_SELF",
        "team_id": "T0001",
        "event_id": "Ev123",
        "event_time": 1515449522i64,
        "event": event
    })
}

fn mention_event() -> JsonValue {
    json!({
        "type": "app_mention",
        "user": "U061F7AUR",
        "text": "<@U0LAN0Z89> is it everything a river should be?",
        "ts": "1515449522.000016",
        "channel": "C123ABC456",
        "event_ts": "1515449522000016"
    })
}

#[test]
fn app_mention_parses_as_a_mention() {
    let ev = parse_user_event(&payload(mention_event())).expect("mention must parse");
    assert!(matches!(ev.kind, SlackEventKind::Mention));
    assert_eq!(ev.channel, "C123ABC456");
    assert_eq!(ev.user.as_deref(), Some("U061F7AUR"));
    assert!(ev.thread_ts.is_none(), "channel-root mention has no thread");
}

/// Slack timestamps are addresses, not numbers. `ts` keeps its dot, `event_ts`
/// often has none, and both must survive byte-for-byte: routing a reply to a
/// thread depends on the exact string. A float round-trip drops the trailing
/// microsecond digits and silently retargets the message.
#[test]
fn timestamps_survive_as_exact_strings() {
    let ev = parse_user_event(&payload(mention_event())).expect("parse");
    assert_eq!(ev.ts, "1515449522.000016");
    assert_eq!(ev.event_ts.as_deref(), Some("1515449522000016"));
}

#[test]
fn plain_message_carries_channel_type() {
    for (ch_type, channel) in [
        ("channel", "C1"),
        ("im", "D1"),
        ("group", "G1"),
        ("mpim", "G2"),
    ] {
        let ev = parse_user_event(&payload(json!({
            "type": "message", "channel": channel, "user": "U1",
            "text": "Hello world", "ts": "1355517523.000005", "channel_type": ch_type
        })))
        .expect("message must parse");
        assert!(matches!(ev.kind, SlackEventKind::Message));
        assert_eq!(ev.channel_type.as_deref(), Some(ch_type));
    }
}

/// A reply inside a thread carries `thread_ts` differing from `ts`; the parent
/// carries them equal. Thread detection rides exclusively on these two fields
/// because the `message_replied` event drops its own subtype (documented bug).
#[test]
fn thread_reply_and_parent_are_distinguishable() {
    let reply = parse_user_event(&payload(json!({
        "type": "message", "channel": "C1", "user": "U1", "text": "reply",
        "ts": "200.000200", "thread_ts": "100.000100", "channel_type": "channel"
    })))
    .expect("parse");
    assert_eq!(reply.thread_ts.as_deref(), Some("100.000100"));
    assert_ne!(reply.thread_ts.as_deref(), Some(reply.ts.as_str()));

    let parent = parse_user_event(&payload(json!({
        "type": "message", "channel": "C1", "user": "U1", "text": "parent",
        "ts": "100.000100", "thread_ts": "100.000100", "channel_type": "channel"
    })))
    .expect("parse");
    assert_eq!(parent.thread_ts.as_deref(), Some(parent.ts.as_str()));
}

#[test]
fn non_user_events_are_not_user_events() {
    // No text, no user: reactions, joins, rate-limit notices.
    assert!(parse_user_event(&payload(json!({"type": "reaction_added", "user": "U1"}))).is_none());
    assert!(parse_user_event(&payload(json!({"type": "app_rate_limited"}))).is_none());
    assert!(parse_user_event(&json!({"type": "event_callback"})).is_none());
}

// ---------------------------------------------------------------- loop guard

/// R1: the authoritative bot marker. Present on anything a bot posted.
#[test]
fn r1_bot_id_drops_the_event() {
    let p = payload(json!({
        "type": "message", "channel": "C1", "text": "from a bot",
        "ts": "1.1", "bot_id": "B999", "channel_type": "channel"
    }));
    assert_eq!(
        loop_drop_reason(&p, Some("A_SELF"), None),
        Some(LoopDrop::BotId)
    );
}

/// R2: classic-app marker. Kept as a belt-and-braces rule because it is NOT
/// reliably present on modern bot messages — R1 is the load-bearing one.
#[test]
fn r2_bot_message_subtype_drops_the_event() {
    let p = payload(json!({
        "type": "message", "channel": "C1", "text": "classic bot",
        "ts": "1.1", "subtype": "bot_message", "channel_type": "channel"
    }));
    assert_eq!(
        loop_drop_reason(&p, Some("A_SELF"), None),
        Some(LoopDrop::BotMessageSubtype)
    );
}

/// R3 reads `event.app_id` — the app that SENT the message. Undocumented and
/// usually absent, hence defensive only.
#[test]
fn r3_own_app_id_inside_the_event_drops_it() {
    let p = payload(json!({
        "type": "message", "channel": "C1", "text": "echo of ourselves",
        "ts": "1.1", "app_id": "A_SELF", "channel_type": "channel"
    }));
    assert_eq!(
        loop_drop_reason(&p, Some("A_SELF"), None),
        Some(LoopDrop::OwnAppId)
    );
}

/// THE DEAF-BOT PIN. `api_app_id` sits on the envelope payload and names the
/// RECEIVING app — it equals our own app id on every single inbound event.
/// If R3 ever reads it instead of `event.app_id`, every event is dropped and
/// the bot goes silent while the socket looks perfectly healthy.
#[test]
fn own_api_app_id_alone_never_drops_an_event() {
    let p = payload(mention_event());
    assert_eq!(
        p.get("api_app_id").and_then(|v| v.as_str()),
        Some("A_SELF"),
        "fixture must exercise the confusable field"
    );
    assert_eq!(
        loop_drop_reason(&p, Some("A_SELF"), None),
        None,
        "a normal user mention must survive the loop guard"
    );
}

/// R4: optional self-filter by user id.
#[test]
fn r4_own_bot_user_id_drops_the_event() {
    let p = payload(json!({
        "type": "message", "channel": "C1", "user": "U_SELF",
        "text": "mine", "ts": "1.1", "channel_type": "channel"
    }));
    assert_eq!(
        loop_drop_reason(&p, Some("A_SELF"), Some("U_SELF")),
        Some(LoopDrop::OwnUserId)
    );
    // Without the optional param configured, R1 still covers the real case.
    assert_eq!(loop_drop_reason(&p, Some("A_SELF"), None), None);
}

#[test]
fn ordinary_human_traffic_passes_every_rule() {
    let p = payload(json!({
        "type": "message", "channel": "C1", "user": "U_HUMAN",
        "text": "hello", "ts": "1.1", "channel_type": "channel"
    }));
    assert_eq!(loop_drop_reason(&p, Some("A_SELF"), Some("U_SELF")), None);
    assert_eq!(loop_drop_reason(&p, None, None), None);
}

/// A missing `hello` app id degrades R3 only; the other rules keep working.
#[test]
fn absent_own_app_id_degrades_r3_without_disabling_r1() {
    let bot = payload(json!({
        "type": "message", "channel": "C1", "text": "bot", "ts": "1.1", "bot_id": "B1"
    }));
    assert_eq!(loop_drop_reason(&bot, None, None), Some(LoopDrop::BotId));
}
