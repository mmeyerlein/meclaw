//! #1540 -- the voice that started a delegation hears of it while it waits.
//!
//! WHY this file exists: in the live set (BK L4 c) a delegation to the
//! background model left the person 2.5 to 7 s in silence. The two knobs
//! made for that gap -- the lead's waiting-time advise (`horizon_feed`) and
//! the policy's fact straight from the tool (`fact_from_tool`) -- were shut
//! out of every delegation round: the lead gave the advise only to a round
//! that was not routed (a delegation always is, `event_routes`), the policy
//! sent no tool fact in a round that carries a `delegation_id`. Now:
//!
//! * (a) the first call of a delegation round, `horizon_feed` on, sends ONE
//!   `live_feed` section `horizon` to the voice of the delegation (the
//!   round's handle `live`), in the form of a turn's own round;
//! * (b) a delegation round with a listed tool's result, `fact_from_tool`
//!   on, sends ONE tool fact to that voice with the round's
//!   `context.delegation_id` -- the voice counts it as the delegation's one
//!   answer (`settle_delegation`) -- also when the round closes twice;
//! * (c) both knobs off: neither, the round as before;
//! * (d) an event round without a delegation, both knobs on: no tool fact,
//!   and no advise of a delegation -- the round is not routed, so the lead
//!   leads it as a turn's own round (its `must_call` advise, as before).
//!
//! Every knob is set as the override of an instance, never in the template
//! (the lead's `event_routes`, `horizon_feed`, the policy's
//! `fact_from_tool`): a set can switch them.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "What is the weather tomorrow?";
const SUMMARY: &str = "Tomorrow in Berlin: cloudy, 4 to 9 degrees.";
/// The collector keys a delegation round `<turn>~<deadline_ms>~<hex8>`.
const ROUND: &str = "t1~4102444800000~ab12cd34";
const DELEGATION: &str = "d1";

/// The knobs a set switches on, as instance overrides.
fn on() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("lead", "event_routes", json!("delegation:quick_bridge")),
        ("lead", "horizon_feed", json!("1")),
        ("lead", "horizon_defaults_ms", json!("{\"weather\": 4000}")),
        ("policy", "fact_from_tool", json!("weather")),
    ]
}

/// The same set with both knobs off (the route stays).
fn off() -> Vec<(&'static str, &'static str, Value)> {
    vec![("lead", "event_routes", json!("delegation:quick_bridge"))]
}

fn hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut h = Hive::with(over).reporting_composite("talky");
    h.check_emits = true;
    h
}

/// A curate of `turn` at `iter`; `delegation` puts the delegation id where
/// the member's `in_delegation` edge and the round's seam carry it.
fn seam(h: &mut Hive, turn: &str, iter: &str, delegation: Option<&str>, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                         "live": "call-9", "path": "simple", "needs": "tool",
                         "families": "weather"});
    if iter != "0" {
        hop["needs"] = json!("");
    }
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                         "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    if let Some(id) = delegation {
        hop["delegation_id"] = json!(id);
        ctx["delegation_id"] = json!(id);
    }
    h.lane("in_curate", ctx, hop, body);
}

fn feeds(h: &Hive, section: &str) -> Vec<Msg> {
    h.routed("live_feed")
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!(section)))
        .collect()
}

fn result_text() -> String {
    json!({"summary": SUMMARY, "days": [{"min": 4, "max": 9}]}).to_string()
}

/// The round's call (not a side call).
fn the_call(h: &Hive) -> Msg {
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .chain(h.routed("brain"))
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the round's call: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// The round `turn` from its first call to its tool result: what each step
/// sent, `(first, result)`.
fn to_the_result(h: &mut Hive, turn: &str, delegation: Option<&str>) -> (Vec<Msg>, Vec<Msg>) {
    seam(h, turn, "0", delegation, json!({"messages": [user(ASK)]}));
    let first = h.out.clone();
    let call = the_call(h);
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
    let tool_body =
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &result_text())]});
    seam(h, turn, "1", delegation, tool_body);
    (first, h.out.clone())
}

fn of(msgs: &[Msg], section: &str) -> usize {
    msgs.iter()
        .filter(|m| m.route() == "live_feed" && m.hop.get("section") == Some(&json!(section)))
        .count()
}

/// (a) A delegation round, the advise on: ONE horizon to the voice of the
/// delegation, at its first call, in the form of a turn's own round.
#[test]
fn gh1540_a_delegation_round_tells_its_voice_how_long() {
    if !shipped() {
        return;
    }
    let mut h = hive(&on());
    seam(
        &mut h,
        ROUND,
        "0",
        Some(DELEGATION),
        json!({"messages": [user(ASK)]}),
    );
    let call = the_call(&h);
    assert_eq!(
        call.hop["target"],
        json!("quick_bridge"),
        "the route: {:?}",
        call.hop
    );
    let f = feeds(&h, "horizon");
    assert_eq!(f.len(), 1, "one horizon: {:?} {:?}", h.out, h.stderr);
    let f = &f[0];
    assert_eq!(f.hop["live"], json!("call-9"), "the delegation's voice");
    assert_eq!(f.hop["turn_id"], json!(ROUND));
    assert!(f.hop.get("at_turn_end").is_none(), "{:?}", f.hop);
    assert_eq!(f.body["seconds"], json!(4));
    assert_eq!(f.body["family"], json!("weather"));
    assert_eq!(f.body["way"], json!("quick_bridge"));
    assert!(f.messages().is_empty(), "no words: {:?}", f.body);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // Round 2 of the delegation: no second horizon.
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
    seam(
        &mut h,
        ROUND,
        "1",
        Some(DELEGATION),
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", "{}")]}),
    );
    assert!(
        feeds(&h, "horizon").is_empty(),
        "once per delegation: {:?}",
        h.out
    );
}

/// (b) A delegation round with a listed tool's result: ONE tool fact to the
/// voice of the delegation, carrying the delegation, also when the round
/// closes twice.
#[test]
fn gh1540_a_delegation_round_hands_its_tool_fact_to_its_voice_once() {
    if !shipped() {
        return;
    }
    let mut h = hive(&on());
    let (_, result) = to_the_result(&mut h, ROUND, Some(DELEGATION));
    let f: Vec<&Msg> = result
        .iter()
        .filter(|m| m.route() == "live_feed" && m.hop.get("section") == Some(&json!("fact")))
        .collect();
    assert_eq!(f.len(), 1, "one fact: {:?} {:?}", h.out, h.stderr);
    let f = f[0];
    assert_eq!(f.hop["fact_source"], json!("tool"));
    assert_eq!(f.hop["live"], json!("call-9"), "the delegation's voice");
    assert_eq!(f.hop["turn_id"], json!(ROUND));
    assert_eq!(f.body["payload"], json!(SUMMARY));
    assert_eq!(
        f.context.get("delegation_id"),
        Some(&json!(DELEGATION)),
        "the voice settles the delegation by it: {:?}",
        f.context
    );
    // Round 2 still answers.
    assert_eq!(
        h.routed("quick").len() + h.routed("brain").len(),
        1,
        "{:?}",
        h.out
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // The round closes a second time: no second fact of the delegation.
    let tool_body =
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &result_text())]});
    seam(&mut h, ROUND, "1", Some(DELEGATION), tool_body);
    assert!(
        feeds(&h, "fact").is_empty(),
        "once per delegation: {:?}",
        h.out
    );
}

/// (c) Both knobs off: a delegation round carries neither, and goes as it
/// went before.
#[test]
fn gh1540_both_knobs_off_a_delegation_round_carries_neither() {
    if !shipped() {
        return;
    }
    let mut h = hive(&off());
    let (first, result) = to_the_result(&mut h, ROUND, Some(DELEGATION));
    for (step, msgs) in [("first call", &first), ("tool result", &result)] {
        assert_eq!(of(msgs, "horizon"), 0, "{step}: {msgs:?}");
        assert_eq!(of(msgs, "fact"), 0, "{step}: {msgs:?}");
    }
}

/// (d) An event round without a delegation (keyed as one, no
/// `delegation_id`), both knobs on: no tool fact and no advise of a
/// delegation. The policy names no event for it, so it is not routed and the
/// lead leads it as a turn's own round: its first call keeps the one advise
/// of the required tool round, beside its side call -- as on the base.
#[test]
fn gh1540_an_event_round_without_a_delegation_carries_neither() {
    if !shipped() {
        return;
    }
    let mut h = hive(&on());
    let (first, result) = to_the_result(&mut h, ROUND, None);
    let side = first
        .iter()
        .filter(|m| {
            m.hop.get("call_kind") == Some(&json!("side"))
                && m.hop["curator_call"]
                    .as_str()
                    .unwrap_or("")
                    .ends_with("-bridge")
        })
        .count();
    assert_eq!(side, 1, "the must_call round's side call: {first:?}");
    assert_eq!(of(&first, "horizon"), 1, "first call: {first:?}");
    assert_eq!(of(&result, "horizon"), 0, "tool result: {result:?}");
    for (step, msgs) in [("first call", &first), ("tool result", &result)] {
        assert_eq!(of(msgs, "fact"), 0, "{step}: {msgs:?}");
    }
}
