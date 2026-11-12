//! D58 feed (GH #1481): an order to feed a running session now, as the pins
//! of the strand drive it -- written once here for the eleven pins instead of
//! once per file.
//!
//! A domain orders on the lane `in_feed` of `curator/policy` that something
//! goes into a running session at once, without waiting for a turn's end
//! (`plans/d58-2026-11-10/plan-parts/u-feed.md`). The head names the session
//! (`live`, `session_id`, `turn_id`, the round `audience_set`), the section,
//! the orderer and, optionally, `once` and `at_turn_end`; the body goes out
//! as the body of the `live_feed`, except a field `mark` the curator takes out
//! and lays, uninterpreted, into the mark `feed` it books for every sent feed.
//! With `once` the curator claims `pending:feed:<live>#<turn>#<once>` first
//! and sends only where the claim took. Behind the knob `feed_in` on
//! `curator/policy`, shipped "0"; the curator's own feeds (the fact from a
//! tool, the renewal) take the same road whatever the knob says.
//!
//! The hive runs in one process (`support/curator_hive.rs`), the shipped
//! scripts under python3; no model.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The knob of the contract (point 7) on `curator/policy`.
pub const KNOB: &str = "feed_in";

/// The lane an order arrives on (contract point 1).
pub const LANE: &str = "in_feed";

/// The kind of the mark every sent feed books (contract point 4).
pub const MARK: &str = "feed";

/// The prefix of the claim of an order with `once` (contract point 2).
pub const CLAIM: &str = "pending:feed:";

/// The head field that names the orderer (contract point 1: "a head field
/// for the sender, set by the orderer or its edge"). The plan names the
/// field, not its key; OR-d58-2.feed.1 reads it `sender`.
pub const SENDER_KEY: &str = "sender";

/// The orderer of the pins.
pub const SENDER: &str = "probe";

/// The handle of the running session the pins feed.
pub const FEED_LIVE: &str = "call-feed-5";

/// The session of the orders.
pub const SESSION: &str = "s1";

/// The curator with the lane from outside open (`feed_in` "1"), `over` on
/// top, every emission checked against the cell's `contract.emits`.
pub fn feed_hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = vec![("policy", KNOB, json!("1"))];
    all.extend(over.iter().cloned());
    let mut h = Hive::with(&all);
    h.check_emits = true;
    h
}

/// The head of an order: the handle (`None`: no `live` at all), the session,
/// `turn`, `section`, the orderer and the round of the standard test round.
pub fn order_head(live: Option<&str>, turn: &str, section: &str) -> Value {
    let mut hop = json!({"session_id": SESSION, "turn_id": turn, "section": section,
                         SENDER_KEY: SENDER, "audience_set": ROUND_E});
    if let Some(l) = live {
        hop["live"] = json!(l);
    }
    hop
}

/// The body of a fact order: the form the tool fact has today
/// (`{section, payload}`).
pub fn fact_body(text: &str) -> Value {
    json!({"section": "fact", "payload": text})
}

/// One order on `in_feed`, the round in the context as every lane message of
/// a colony carries it; `out` cleared before.
pub fn order(h: &mut Hive, hop: Value, body: Value) {
    h.out.clear();
    h.lane(LANE, json!({"audience_set": ROUND_E}), hop, body);
}

/// The same with the context of the caller's choice (`audience_set` `null`:
/// the round only in the head).
pub fn order_in(h: &mut Hive, ctx: Value, hop: Value, body: Value) {
    h.out.clear();
    h.lane(LANE, ctx, hop, body);
}

/// The `live_feed`s that left the hive since `out` was last cleared.
pub fn feeds(h: &Hive) -> Vec<Msg> {
    h.routed("live_feed")
}

/// The feeds of one section.
pub fn feeds_of(h: &Hive, section: &str) -> Vec<Msg> {
    feeds(h)
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!(section)))
        .collect()
}

/// One row of the mark `feed`.
#[derive(Debug, Clone)]
pub struct FeedMark {
    pub session: String,
    pub turn: String,
    pub value: Value,
    pub audience: String,
}

/// Every mark `feed` of the ledger, in order.
pub fn feed_marks(h: &Hive) -> Vec<FeedMark> {
    h.rows(&format!(
        "SELECT session_id, turn_id, value, audience_set FROM marks \
         WHERE kind = '{MARK}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| FeedMark {
        session: r[0].as_str().unwrap_or("").to_string(),
        turn: r[1].as_str().unwrap_or("").to_string(),
        value: sj::from_str(r[2].as_str().unwrap_or("{}")).expect("the mark is JSON"),
        audience: r[3].as_str().unwrap_or("").to_string(),
    })
    .collect()
}

/// The marks `feed` of one section.
pub fn feed_marks_of(h: &Hive, section: &str) -> Vec<FeedMark> {
    feed_marks(h)
        .into_iter()
        .filter(|m| m.value.get("section") == Some(&json!(section)))
        .collect()
}

/// The key of the claim of `once` in `turn` of the handle `live`.
pub fn claim_key(live: &str, turn: &str, once: &str) -> String {
    format!("{CLAIM}{live}#{turn}#{once}")
}

/// The keys of the ledger's `state` rows that start with `prefix`, sorted.
pub fn keys_with(h: &Hive, prefix: &str) -> Vec<String> {
    h.rows("SELECT key FROM state ORDER BY key")
        .iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .filter(|k| k.starts_with(prefix))
        .collect()
}

/// The claims of orders with `once` the ledger holds.
pub fn claims(h: &Hive) -> Vec<String> {
    keys_with(h, CLAIM)
}

/// How many messages `./policy` has sent to the ledger so far: a step at the
/// store is one message, however many operations it bundles.
pub fn policy_steps(h: &Hive) -> usize {
    h.ledger_steps.iter().filter(|(c, _)| c == "policy").count()
}

/// The canonical round of the standard test round, as a row stores it.
pub fn round_e() -> String {
    round_canon(ROUND_E).expect("the standard round is canonical")
}

/// The held state of the start window of `live` (`seed_facts:<live>`),
/// parsed; `{}` when there is none.
pub fn held(h: &Hive, live: &str) -> Value {
    sj::from_str(&h.state(&format!("seed_facts:{live}"))).unwrap_or_else(|_| json!({}))
}

// --- the curator's own feeds, driven as their standing pins drive them ---

/// The summary the weather tool hands back.
pub const SUMMARY: &str = "Tomorrow in Berlin: cloudy, 4 to 9 degrees.";

/// The handle the tool fact and the interrupt run under (their standing pins'
/// `call-9`).
pub const OWN_LIVE: &str = "call-9";

/// One `in_curate` of `turn`, iteration `iter`, in session `s1`
/// (`curator_1/d53_fx_the_fact_comes_from_the_tool.rs`).
pub fn tool_seam(h: &mut Hive, turn: &str, iter: &str, hop_extra: Value, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": SESSION, "turn_id": turn, "iter": iter, "phase": "",
                         "path": "", "needs": "", "families": ""});
    for (k, v) in obj(hop_extra) {
        hop[k] = v;
    }
    let path = hop["path"].clone();
    let ctx = json!({"session_id": SESSION, "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, body);
}

/// The tool stamp of a running session's turn: `simple`, needs a tool.
pub fn tool_stamp(family: &str) -> Value {
    json!({"live": OWN_LIVE, "path": "simple", "needs": "tool", "families": family})
}

/// Turn `turn` to the result of the weather tool: the stamp, the required
/// call asks for the tool, the result comes back on iteration 1 -- the
/// moment the curator's tool fact leaves.
pub fn to_the_tool_result(h: &mut Hive, turn: &str) {
    let ask = "What is the weather tomorrow?";
    tool_seam(
        h,
        turn,
        "0",
        tool_stamp("weather"),
        json!({"messages": [user(ask)]}),
    );
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(calls.len(), 1, "one tool round: {:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(
        &calls[0],
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
    let mut hop = tool_stamp("weather");
    hop["needs"] = json!("");
    let text = json!({"summary": SUMMARY, "days": [{"min": 4, "max": 9}]}).to_string();
    tool_seam(
        h,
        turn,
        "1",
        hop,
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &text)]}),
    );
}

/// The crisis question of the pins.
pub const QUESTION: &str = "Are you safe right now?";

/// The knobs that make a crisis heard on a partial interrupt at once
/// (`curator_1/d53_v7_a_crisis_interrupts_on_the_partial.rs`).
pub fn crisis_knobs() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("lead", "interrupt_on_delta", json!("1")),
        (
            "lead",
            "interrupt_questions",
            json!(json!({"acute": QUESTION}).to_string()),
        ),
    ]
}

/// A partial's stamp of `turn` with the crisis value `acute`, the handle
/// `call-9` in the context, as the route sends it.
pub fn crisis_partial(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"turn_id": turn, "path": "talk"});
    let ctx = json!({"session_id": SESSION, "turn_id": turn, "delta": "1", "live": OWN_LIVE,
                     "audience_set": ROUND_E});
    h.lane(
        "in_delta_stamp",
        ctx,
        hop,
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "talk", "p": 0.9,
                                         "intent": "", "needs": [], "families": [],
                                         "duration": 5, "text_norm": "i do not want to go on",
                                         "choices": {"risk": {"value": "acute", "p": 0.9,
                                                              "source": "model"}},
                                         "turn_notes": []}}),
    );
}

/// The end of `turn` after a crisis partial: its normal curate and the
/// answer on the tap.
pub fn crisis_turn_end(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": SESSION, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "", "live": OWN_LIVE});
    let ctx = json!({"session_id": SESSION, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("I am here.")]));
}

/// The values of the marks of `kind`, parsed.
pub fn marks_of(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// The body of a sent feed as a value, an empty `messages` dropped: the
/// universal body may carry `messages: []` beside the order's own fields
/// (today's tool fact does); any other field the order did not give is a
/// change of the body.
pub fn body_of(m: &Msg) -> Value {
    let mut b = m.body.clone();
    if b.get("messages")
        .is_some_and(|v| v.as_array().is_some_and(|a| a.is_empty()))
    {
        b.remove("messages");
    }
    Value::Object(b)
}
