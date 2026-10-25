//! GH #1305 -- conversation events and inner turns, the curator's half.
//!
//! WHY this file exists. An app that follows a conversation (the gate of a
//! coordinator deciding whether somebody is reachable, a receipt of what was
//! said) had nothing to listen to: the keeper's `close` went into the curator
//! and came out only as the `write` batch, a whole transcript, and nothing at
//! all said that a conversation had begun. And a turn without a speaking person
//! -- a reminder an app hands the companion -- entered the window as a turn of
//! the person: the model read it as the person's words, and the memory filed it
//! as the person's episode.
//!
//! Pinned here:
//! - with the knob `conversation_events` on, `./writer` sends ONE `closed` per
//!   generation, after the `write` of the same close, carrying counts and
//!   times and never a word of the conversation; a close the ledger refused
//!   twice still says the generation closed (`degraded: store`, no counts);
//! - the knob off, nothing more leaves and `write` keeps its bytes;
//! - an inner turn (`context.inner_source`) stands in the window as a marked
//!   event, never as the person's words, is never a person's episode, and a
//!   later turn of the same round on another channel sees it and its answer.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "tg:7-2026-10-24T08:00:00.000000Z";
const EVENT: &str = "The dentist appointment moved to Thursday at ten.";

/// The close as talky's keeper edge hands it in (GH #1305): the keeper's
/// facts about the generation on the hop, the round in context.
fn close_with_meta(h: &mut Hive, session: &str) {
    h.lane(
        "in_close",
        json!({"session_id": session, "channel": "tg:7"}),
        json!({"close_reason": "night", "opened_at": "2026-10-24T08:00:00.000000Z",
               "closed_at": "2026-10-24T23:00:00.000000Z", "channel_node": "telegram"}),
        json!({"messages": []}),
    );
}

fn events_on() -> Hive {
    Hive::with(&[("writer", "conversation_events", json!("1"))])
}

/// One person turn and its final answer on the wall of session `S`.
fn one_turn(h: &mut Hive) {
    turn(
        h,
        S,
        "t1",
        "When is the dentist?",
        "On Thursday at ten.",
        json!({}),
    );
    h.out.clear();
}

fn position(h: &Hive, route: &str) -> Vec<usize> {
    h.out
        .iter()
        .enumerate()
        .filter(|(_, m)| m.route() == route)
        .map(|(i, _)| i)
        .collect()
}

// ------------------------------------------------------------ closed

#[test]
fn gh1305_one_closed_per_generation() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    one_turn(&mut h);
    close_with_meta(&mut h, S);
    let closed = h.routed("closed");
    assert_eq!(closed.len(), 1, "one closed per close: {:?}", h.stderr);
    let c = &closed[0];
    assert_eq!(c.hop["session_id"], S);
    assert_eq!(c.hop["channel"], "tg:7");
    assert_eq!(c.hop["channel_node"], "telegram");
    assert_eq!(c.hop["audience_set"], ROUND_E);
    assert_eq!(c.hop["opened_at"], "2026-10-24T08:00:00.000000Z");
    assert_eq!(c.hop["closed_at"], "2026-10-24T23:00:00.000000Z");
    assert_eq!(c.hop["reason"], "night");
    assert_eq!(c.hop["turns_person"], "1");
    assert_eq!(c.hop["turns_assistant"], "1");
    assert_eq!(c.hop["last_turn_id"], "t1");
    assert_eq!(c.hop["degraded"], "");
}

#[test]
fn gh1305_closed_after_write() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    one_turn(&mut h);
    close_with_meta(&mut h, S);
    let (w, c) = (position(&h, "write"), position(&h, "closed"));
    assert_eq!((w.len(), c.len()), (1, 1), "{:?}", h.out);
    assert!(w[0] < c[0], "write leaves first, then closed: {:?}", h.out);
}

#[test]
fn gh1305_closed_carries_no_text() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    one_turn(&mut h);
    close_with_meta(&mut h, S);
    let c = &h.routed("closed")[0];
    assert!(
        c.messages().is_empty(),
        "no turn rides on closed: {:?}",
        c.body
    );
    assert_eq!(
        c.body.keys().cloned().collect::<Vec<_>>(),
        vec!["messages".to_string()]
    );
    let all = format!("{:?} {:?}", c.hop, c.context);
    for said in ["dentist", "Thursday"] {
        assert!(!all.contains(said), "{said} leaked onto closed: {all}");
    }
}

#[test]
fn gh1305_closed_of_an_empty_generation_counts_nothing() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    close_with_meta(&mut h, "tg:7-empty");
    let c = h.routed("closed");
    assert_eq!(c.len(), 1, "{:?}", h.out);
    assert_eq!(c[0].hop["turns_person"], "0");
    assert_eq!(c[0].hop["turns_assistant"], "0");
    assert_eq!(c[0].hop["last_turn_id"], "");
}

#[test]
fn gh1305_a_refused_close_still_says_closed_degraded() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    one_turn(&mut h);
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("c-wall".into(), "query_timeout".into())];
    h.refuse_phase = Some("close-wall-again".into());
    close_with_meta(&mut h, S);
    assert!(h.routed("write").is_empty(), "no batch from a refused read");
    let c = h.routed("closed");
    assert_eq!(c.len(), 1, "the generation still closed: {:?}", h.out);
    assert_eq!(c[0].hop["degraded"], "store");
    assert_eq!(c[0].hop["turns_person"], "0");
    assert_eq!(c[0].hop["turns_assistant"], "0");
    assert_eq!(c[0].hop["reason"], "night");
}

#[test]
fn gh1305_events_off_sends_nothing() {
    if !shipped() {
        return;
    }
    assert_eq!(
        cell_config("writer")["params"]["conversation_events"],
        json!("0"),
        "the knob ships off"
    );
    let mut off = Hive::new();
    one_turn(&mut off);
    close_with_meta(&mut off, S);
    assert!(off.routed("closed").is_empty(), "{:?}", off.out);
    // The keeper's facts on the hop change nothing on `write`.
    let mut plain = Hive::new();
    one_turn(&mut plain);
    plain.lane(
        "in_close",
        json!({"session_id": S, "channel": "tg:7"}),
        json!({}),
        json!({"messages": []}),
    );
    let (a, b) = (&off.routed("write")[0], &plain.routed("write")[0]);
    assert_eq!(
        (&a.hop, &a.context, &a.body),
        (&b.hop, &b.context, &b.body),
        "write is byte for byte the write without the keeper's facts"
    );
    // The knob on, `write` is the same as well.
    let mut on = events_on();
    one_turn(&mut on);
    close_with_meta(&mut on, S);
    let c = &on.routed("write")[0];
    assert_eq!((&c.hop, &c.context, &c.body), (&a.hop, &a.context, &a.body));
}

// ------------------------------------------------------------ inner turns

/// One inner round on the inner channel of `source`, in the standard round.
fn curate_inner(h: &mut Hive, session: &str, turn_id: &str, source: &str, text: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn_id, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": session, "turn_id": turn_id, "iter": "0",
                     "channel": format!("inner:{source}"), "channel_node": "telegram",
                     "inner_source": source, "audience_set": ROUND_E});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

#[test]
fn gh1305_inner_turn_is_marked_not_the_persons_words() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = curate_inner(&mut h, "inner:orga-1", "i1", "orga", EVENT);
    let last = texts(&call).last().cloned().unwrap_or_default();
    assert!(
        last.starts_with("[event from orga -- not something the person said"),
        "the event is marked: {last}"
    );
    assert!(last.ends_with(EVENT), "{last}");
    let kinds = h.rows("SELECT kind FROM wall WHERE session_id = 'inner:orga-1'");
    assert_eq!(kinds, vec![vec![json!("event")]], "kind event, never user");
}

#[test]
fn gh1305_the_inner_header_is_a_param() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("intake", "inner_header", json!("[note by {source}]"))]);
    let call = curate_inner(&mut h, "inner:orga-1", "i1", "orga", EVENT);
    assert_eq!(
        texts(&call).last().cloned().unwrap_or_default(),
        format!("[note by orga]\n{EVENT}")
    );
}

#[test]
fn gh1305_inner_turn_is_never_a_person_episode() {
    if !shipped() {
        return;
    }
    let mut h = events_on();
    let call = curate_inner(&mut h, "inner:orga-1", "i1", "orga", EVENT);
    let episodes = h.routed("turn_write");
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("Shall I note Thursday?")]),
    );
    let mut all = episodes;
    all.extend(h.routed("turn_write"));
    let origins: Vec<Value> = all
        .iter()
        .flat_map(|m| m.messages())
        .map(|m| m["origin"].clone())
        .collect();
    assert!(!origins.contains(&json!("user")), "{origins:?}");
    assert!(origins.contains(&json!("event")), "{origins:?}");
    h.out.clear();
    close_with_meta(&mut h, "inner:orga-1");
    let w = &h.routed("write")[0];
    let batch: Vec<Value> = w.messages().iter().map(|m| m["origin"].clone()).collect();
    assert_eq!(
        batch,
        vec![json!("event"), json!("assistant")],
        "{:?}",
        w.body
    );
    assert_eq!(h.routed("closed")[0].hop["turns_person"], "0");
}

#[test]
fn gh1305_reply_after_an_inner_message_sees_it_in_the_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = curate_inner(&mut h, "inner:orga-1", "i1", "orga", EVENT);
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("Shall I note Thursday?")]),
    );
    // The person answers on the telegram channel: another session, the same round.
    let next = h.curate(
        "tg:7-s2",
        "t2",
        0,
        json!([user("Yes please.")]),
        Value::Null,
    );
    let seen = texts(&next).join("\n");
    assert!(seen.contains(EVENT), "the event is in the window: {seen}");
    assert!(
        seen.contains("Shall I note Thursday?"),
        "and its answer: {seen}"
    );
    assert!(seen.contains("[event from orga"), "still marked: {seen}");
}

#[test]
fn gh1305_no_kind_inner_means_todays_bytes() {
    if !shipped() {
        return;
    }
    // A person who types the marker is still the person: without
    // `context.inner_source` nothing is framed and the row is `user`.
    let typed = "[event from orga -- not something the person said] hi";
    let mut h = Hive::new();
    let call = h.curate("tg:7-s3", "t3", 0, json!([user(typed)]), Value::Null);
    assert_eq!(texts(&call).last().cloned().unwrap_or_default(), typed);
    let kinds = h.rows("SELECT kind FROM wall WHERE session_id = 'tg:7-s3'");
    assert_eq!(kinds, vec![vec![json!("user")]]);
    // An empty inner source is no inner turn.
    let mut e = Hive::new();
    e.out.clear();
    e.lane(
        "in_curate",
        json!({"session_id": "tg:7-s4", "turn_id": "t4", "iter": "0", "channel": "test",
               "inner_source": "", "audience_set": ROUND_E}),
        json!({"session_id": "tg:7-s4", "turn_id": "t4", "iter": "0", "phase": ""}),
        json!({"messages": [user("hello")]}),
    );
    assert_eq!(
        texts(&e.routed("brain")[0])
            .last()
            .cloned()
            .unwrap_or_default(),
        "hello"
    );
}

/// Review M3: the day summary never reads an inner event as the person's
/// words -- the policy's transcript sets it apart as a `note:` line.
#[test]
fn gh1305_the_day_summary_reads_an_event_as_a_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    let at = backdated(40);
    let framed = format!("[event from orga -- not something the person said]\n{EVENT}");
    h.row(at, S, "i-0", "event", &user(&framed), 0);
    h.row(
        at + chrono::Duration::seconds(1),
        S,
        "i-0",
        "assistant",
        &said("Your appointment is on Thursday now."),
        1,
    );
    turn(
        &mut h,
        S,
        "t-1",
        "Thanks, and the weather?",
        "Sunny.",
        json!({}),
    );
    turn(
        &mut h,
        S,
        "t-2",
        "Lovely. Call me tomorrow at nine.",
        "Will do, tomorrow at nine.",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let order = h.clock_order_for(ROUND_E).expect("a strike");
    h.fire(&order);
    let req = h.summ.front().cloned().expect("a summarizer request");
    let transcript = texts(&req).join("\n");
    assert!(
        transcript.contains(&format!("note: {framed}")),
        "the event is a note: {transcript}"
    );
    assert!(
        !transcript.contains(&format!("user: {framed}")),
        "an event is never a turn of the person: {transcript}"
    );
}
