//! D58 moveout (GH #1476): the bridge, the waiting-time advise and the crisis
//! interrupt leave the curator for the lead -- the moves the twelve pins of
//! the strand drive, written once here instead of once per file.
//!
//! The curator keeps two general means (`plans/d58-2026-11-10/plan-parts/
//! u-moveout.md`): a conditional side call beside the main call (the verdict's
//! `side`: `{when, target, window, text, fill?, first?}`, `when` one of
//! `external` -- the verdict sends the main call out of the composition --
//! and `must_call` -- the cut gives the call a required tool round) and the
//! conditional feed (the verdict's `feed`: `{when, section, body, once?,
//! mark?}`). The decision, the wording and the bookkeeping are the lead's.
//! A side call says what it is in the neutral head field [`KIND`]
//! (OR-d58-2.moveout.1), never in the ending of its call id.
//!
//! The hive runs in one process (`support/curator_hive.rs`), the shipped
//! scripts under python3; no model.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{Map, Value, json};

/// The neutral head field that carries the kind of a call (contract point 3,
/// OR-d58-2.moveout.1). The composite lifts it into the context on every
/// edge to a model cell, the way it lifts `curator_call`.
pub const KIND: &str = "call_kind";

/// Its value on a side call (contract point 2: the trigger in `calls` too).
pub const SIDE: &str = "side";

/// The neutral head field "this answer does not end the round" (contract
/// point 4, orchestrator ruling a): "1" on the side call of a tool round.
/// talky's edges lift it onto `context.interim` "1", so the dispatcher, the
/// collector's answer mark and `proxy` read what they read before.
pub const OPEN: &str = "round_open";

/// talky's model cells a side call can reach; their profiles name the slot of
/// the side text (`instructions.side_slot`, orchestrator ruling b).
pub const TALKY_FAST_CELLS: [&str; 3] = ["quick", "quick_bridge", "quick_spec"];

/// The two conditions of a side call or a conditional feed (contract point 1).
pub const EXTERNAL: &str = "external";
pub const MUST_CALL: &str = "must_call";

/// The handle of the running session the pins' turns belong to.
pub const LIVE_H: &str = "call-9";

/// The session of the pins.
pub const SESSION_M: &str = "s1";

/// The knobs that move from `curator/policy` to `lead/lead` (contract
/// points 5 and 7), and the wording of a side call beside an errand
/// (`bridge_wording`, moved with the rebase on feed, re-check M-1).
pub const MOVED_KNOBS: [&str; 4] = [
    "horizon_advise",
    "horizon_defaults_ms",
    "horizon_quantile",
    "bridge_wording",
];

/// The tool round's bridge sentence as it stands today (`bridge_text("tool")`
/// in `curator/policy` before the strand): the lead ships it as the default
/// of a knob of its own (contract point 5).
pub const TOOL_WORDS: &str = "Your lookup is already running right now";

/// The curator with a lead wired as talky wires it, `over` on top, every
/// emission checked against the cell's `contract.emits`; talky's model cells
/// have told it their packages, with the profile talky ships them
/// ([`Hive::report_composite`]): the curator knows the slot of a side text
/// only from the target's profile (orchestrator ruling b) and the required
/// tool round only from its `must_call`; a target that reported nothing gets
/// no side call. `out` cleared after.
pub fn moveout_hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut h = Hive::with(over);
    h.check_emits = true;
    h.report_composite("talky");
    h.out.clear();
    h
}

/// The lead's waiting-time knobs on: `defaults` per family (ms).
pub fn horizon_knobs(defaults: Value) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("lead", "horizon_advise", json!("1")),
        ("lead", "horizon_defaults_ms", json!(defaults.to_string())),
    ]
}

/// One `in_curate` of `turn`, iteration `iter`, in session `s1`; `out`
/// cleared before.
pub fn seam(h: &mut Hive, turn: &str, iter: &str, hop_extra: Value, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": SESSION_M, "turn_id": turn, "iter": iter, "phase": "",
                         "path": "", "needs": "", "families": ""});
    for (k, v) in obj(hop_extra) {
        hop[k] = v;
    }
    let path = hop["path"].clone();
    let ctx = json!({"session_id": SESSION_M, "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, body);
}

/// The stamp of a running session's `simple` turn that needs a tool of
/// `families`.
pub fn tool_stamp(families: &str) -> Value {
    json!({"live": LIVE_H, "path": "simple", "needs": "tool", "families": families})
}

/// The stamp of a turn whose main call leaves the composition (`complex`:
/// the errand class `consult`, `research`: the class `research`).
pub fn errand_stamp(path: &str) -> Value {
    json!({"live": LIVE_H, "path": path, "families": "web"})
}

/// Every side call that left since `out` was cleared, in order.
pub fn side_calls(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| m.route() == "model_call" && m.hop.get(KIND) == Some(&json!(SIDE)))
        .cloned()
        .collect()
}

/// Every main call (a model call or an errand that is no side call) that
/// left since `out` was cleared, in order.
pub fn main_calls(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| {
            (m.route() == "model_call" || m.route() == "errand")
                && m.hop.get(KIND) != Some(&json!(SIDE))
                && m.hop.get("spec") != Some(&json!("1"))
        })
        .cloned()
        .collect()
}

/// The position of a message in `out`.
pub fn position(h: &Hive, m: &Msg) -> usize {
    h.out
        .iter()
        .position(|o| {
            o.hop.get("curator_call") == m.hop.get("curator_call") && o.route() == m.route()
        })
        .unwrap_or_else(|| panic!("{:?} is in out", m.hop))
}

/// The leaf of the side text in a call's system part, as talky's targets
/// name the slot (contract point 1: the leaf `instructions.bridge` today).
pub fn side_text(m: &Msg) -> String {
    m.body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every text of a call's messages, joined.
pub fn message_texts(m: &Msg) -> String {
    m.messages()
        .iter()
        .map(|el| el["text"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The trigger of the `calls` row of `call`.
pub fn trigger_of(h: &Hive, call: &Msg) -> String {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    h.rows(&format!("SELECT trigger FROM calls WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r.first())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The answer of a call as the composite's edges hand it back to the
/// curator: the call's context, the keys the edge to the model cell set
/// from the head (`curator_call`, `turn_id`, `session_id`, `iter`), the
/// call's kind lifted the same way, and `interim` as talky's edge lifts it:
/// "1" for a side call with [`OPEN`], else the call's own `interim`.
pub fn answer_call(h: &mut Hive, call: &Msg, finish: &str, extra: Value, messages: Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert(
        KIND.into(),
        call.hop.get(KIND).cloned().unwrap_or(json!("")),
    );
    let interim = if call.hop.get(OPEN).is_some_and(|v| *v == "1") {
        json!("1")
    } else {
        call.hop.get("interim").cloned().unwrap_or(json!(""))
    };
    ctx.insert("interim".into(), interim);
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
    for (k, v) in obj(extra) {
        hop.insert(k, v);
    }
    h.out.clear();
    h.lane(
        "in_llm",
        Value::Object(ctx),
        Value::Object(hop),
        json!({"messages": messages}),
    );
}

/// The horizon feeds that left since `out` was cleared.
pub fn horizons(h: &Hive) -> Vec<Msg> {
    h.routed("live_feed")
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!("horizon")))
        .collect()
}

/// The tool round of `turn` (stamped with `families`) to its end: the
/// required call asks for the tool, the result comes back on iteration 1,
/// the quick voice answers and closes the turn.
pub fn finish_round(h: &mut Hive, turn: &str, families: &str) {
    let calls: Vec<Msg> = main_calls(h)
        .into_iter()
        .filter(|m| m.route() == "model_call")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "one required call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    answer_call(
        h,
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "web_search")]),
    );
    let mut hop = tool_stamp(families);
    hop["needs"] = json!("");
    seam(
        h,
        turn,
        "1",
        hop,
        json!({"messages": [tool_call("c1", "web_search"), tool_result("c1", "12 degrees")]}),
    );
    let answer = main_calls(h);
    assert_eq!(answer.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        side_calls(h).is_empty(),
        "no side call on iteration 1: {:?}",
        h.out
    );
    let answer = answer[0].clone();
    answer_call(h, &answer, "stop", json!({}), json!([said("12 degrees.")]));
}

// --- the lead's pure decision ---

/// talky's wiring of the lead (`LEAD_WIRING`) as knobs, `extra` on top.
pub fn lead_knobs(extra: Value) -> Value {
    let mut k = Map::new();
    for (key, value) in LEAD_WIRING {
        k.insert(key.to_string(), json!(value));
    }
    for (key, value) in obj(extra) {
        k.insert(key, value);
    }
    Value::Object(k)
}

/// The head of a call's question to the lead: a running session's first
/// iteration, no speculation.
pub fn ask_head(live: &str) -> Value {
    json!({"channel": "text", "live": live, "session": SESSION_M, "turn": "t1",
           "call": "c-t1", "iter": 0, "consult_id": "", "consult_class": "", "spec": "",
           "escalate": "", "at": "2026-11-12T10:00:00.000000Z"})
}

/// The lead's state rows of a session that has decided nothing yet.
pub fn no_state() -> Value {
    json!({"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}})
}

/// `lead_verdict` of the shipped lead under `knobs` for a stamp of `path`,
/// `needs` and `families`.
pub fn verdict(knobs: Value, path: &str, needs: &[&str], families: &[&str], live: &str) -> Value {
    assert!(lead_shipped(), "the lead cell is shipped");
    let (v, err) = cell_scope(
        "lead",
        knobs,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([{"path": path, "needs": needs, "families": families}, ask_head(live), no_state()]),
    );
    assert!(v.is_object(), "a verdict: {v} {err}");
    v
}

/// The entries of the verdict's `side` (contract point 1) under `when`.
pub fn sides_of(v: &Value, when: &str) -> Vec<Value> {
    v["side"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["when"] == json!(when))
        .collect()
}

/// The entries of the verdict's `feed` (contract point 6) under `when`.
pub fn feeds_of_verdict(v: &Value, when: &str) -> Vec<Value> {
    v["feed"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["when"] == json!(when))
        .collect()
}

/// Whether a flag of an entry is set (`true` or "1").
pub fn flag(v: &Value) -> bool {
    v == &json!(true) || v == &json!("1")
}

/// The program of a cell of a template (`templates/<path>/config.json`).
pub fn script_at(path: &str) -> String {
    read_json(&repo(&format!("templates/{path}/config.json")))["params"]["script_inline"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A row of the lead's own store as JSON, `Value::Null` when there is none.
pub fn lead_json(h: &Hive, key: &str) -> Value {
    meclaw_core::serde_json::from_str(&h.lead_state(key)).unwrap_or(Value::Null)
}

/// The turns of a table `{turn: row}` of the lead's store
/// (`horizon_plan:<session>`, `horizon_turn:<session>`; review B M-2),
/// sorted; empty when the row is absent.
pub fn table_turns(h: &Hive, key: &str) -> Vec<String> {
    let mut turns: Vec<String> = lead_json(h, key)
        .as_object()
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    turns.sort();
    turns
}

/// The durations (ms) the lead booked for `family` in `horizon_rounds`.
pub fn booked(h: &Hive, family: &str) -> Vec<u64> {
    lead_json(h, "horizon_rounds")[family]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

/// The lead's store rows whose value contains `needle`.
pub fn lead_rows_with(h: &Hive, needle: &str) -> Vec<(String, String)> {
    h.lead_rows("SELECT key, value FROM state ORDER BY key")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                r[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .filter(|(_, v)| v.contains(needle))
        .collect()
}
