//! D58-3 kern (GH #1524, contract point 2, N1) -- a late stamp carries the
//! yes-values of its answers.
//!
//! WHY this file exists: a stamp that leaves after `late_ms` is sent on the
//! fallback `late`, but its decider DID answer: the answers lie before the
//! route cell as they do for a judged stamp. The stamp carried no
//! `needs_yes` all the same, and the curator's rule `recall_skip_sure` had
//! nothing to read -- measured live, most of the skips that did not happen
//! were late turns. With `stamp_needs_yes` on, the late stamp carries the same
//! `needs_yes` a judged stamp of those answers carries; its `needs` stay empty
//! (the fallback way carries no tool duty). The collector parks the field with
//! the leg and lays it on the seam whatever the fallback, so the whole way
//! from the route cell to the curator holds for `late` too. Shipped (knob
//! off), the late stamp is the stamp of before.
//!
//! GH #1562: the route cell cuts no verdict as late any more (`late_ms` is
//! gone; the collector's `route_wait_ms` is the one bound of the wait). The
//! subject of this file -- a slow decider's yes-values reach the curator --
//! now holds as a JUDGED stamp: answered an hour after the ask, the stamp is
//! the stamp of a fast answer, yes-values and named needs alike, and an unsure
//! path is `unsure`, not `late`. Whether it still reaches the turn is the
//! collector's word (`gh1562_a_stamp_after_the_fire_acts_on_nothing`).
//!
//! Runs the shipped `talky/route` and `collector/assemble` scripts
//! (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const SEAM_TURN: &str = "route-channel#0001";
const SEAM_FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn four_needs() -> Value {
    json!({"tool": "a tool call",
           "reasoning": "careful step-by-step reasoning",
           "research": "a deep search over many sources",
           "memory": "something the person said earlier or before today"})
}

/// The decider's answer for turn `c-1`, asked `ago_ms` before now.
fn decision(ago_ms: i64, hop: Value, answers: Value) -> Value {
    let mut h = json!({"finish_reason": "stop", "asked_at": (now_ms() - ago_ms).to_string()});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    let mut doc = lane("in_decision", h, json!({"turn_id": "c-1"}), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

/// An hour after the ask: past any bound a route cell ever had.
fn late(answers: Value) -> Value {
    decision(3_600_000, json!({}), answers)
}

fn stamp_of(over: &[(&str, Value)], doc: Value) -> Value {
    let out = run_cell(ROUTE, over, doc).0;
    assert_eq!(out.len(), 1, "expected one emission, got {out:?}");
    let m = out.into_iter().next().expect("one emission");
    assert_eq!(m["header"]["route"], "stamp", "{m}");
    m
}

fn answers(path_p: f64) -> Value {
    json!({
        "path": {"choice": "simple", "p": path_p},
        "intent": {"choice": "chat", "p": 0.8},
        "needs.memory": {"yes": 0.05}, "needs.tool": {"yes": 0.9},
        "needs.reasoning": {"yes": 0.45}, "needs.research": {"yes": 0.0}
    })
}

fn seam_knob() -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("needs_no_recall", json!("1")),
    ]
}

fn seam_leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": SEAM_TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": SEAM_FRESH})
}

/// What the lane `in_route` parks for the stamp emission `m` (its stamp,
/// its hop's fallback).
fn parked(m: &Value) -> Value {
    let mut hop = json!({"turn_id": SEAM_TURN, "path": m["stamp"]["path"]});
    if let Some(f) = m["header"].get("fallback") {
        hop["fallback"] = f.clone();
    }
    let mut doc = lane("in_route", hop, json!({}), json!([]));
    let mut stamp = m["stamp"].clone();
    stamp["turn_id"] = json!(SEAM_TURN);
    doc["stamp"] = stamp;
    let out = assemble(&seam_knob(), doc);
    let row = calls_of(in_phase(&out, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .next()
        .unwrap_or_else(|| panic!("in_route parked no row: {out:?}"));
    assert_eq!(row["role"], "leg-route", "{row}");
    serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json")
}

/// The seam of a turn whose parked route leg is `leg`.
fn seam_of(leg: Value) -> Value {
    let window = seam_leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "good morning, how are you",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    );
    let memory = seam_leg("leg-memory", json!({"system": {}, "messages": []}));
    let out = assemble(
        &seam_knob(),
        bundle_reply(
            "collect",
            SEAM_TURN,
            &[(
                "c-collect-read",
                json!([window, memory, seam_leg("leg-route", leg)]),
            )],
        ),
    );
    out.iter()
        .find(|m| m["header"]["route"] == "curate")
        .cloned()
        .unwrap_or_else(|| panic!("no seam: {out:?}"))
}

#[test]
fn gh_d583_kern_route_a_late_stamp_carries_its_yes_values() {
    let on = [("needs", four_needs()), ("stamp_needs_yes", json!("1"))];
    let all = json!({"memory": 0.05, "reasoning": 0.45, "research": 0.0, "tool": 0.9});

    // On: an hour after the ask the stamp is judged -- no fallback, the
    // yes-values of its answers and the needs they name.
    let m = stamp_of(&on, late(answers(0.91)));
    assert!(
        m["header"].get("fallback").is_none(),
        "a slow verdict is no fallback any more (GH #1562): {m}"
    );
    assert_eq!(
        m["stamp"]["needs_yes"], all,
        "the slow stamp carries the yes-values its decider answered: {m}"
    );
    assert_eq!(m["stamp"]["needs"], json!(["tool"]), "{m}");
    // An unsure path, slow or not, is `unsure` -- the fallback of a fast one.
    let u = stamp_of(&on, late(answers(0.3)));
    assert_eq!(u["header"]["fallback"], "unsure", "{u}");
    let mut slow = u["stamp"].clone();
    let mut fast = stamp_of(&on, decision(0, json!({}), answers(0.3)))["stamp"].clone();
    slow["duration"] = json!(0);
    fast["duration"] = json!(0);
    assert_eq!(slow, fast, "slow and fast unsure stamps are one stamp: {u}");

    // The whole way: parked with the leg, on the seam, no fallback beside it.
    let leg = parked(&m);
    assert_eq!(leg["needs_yes"], all, "the leg keeps the yes-values: {leg}");
    assert!(leg.get("fallback").is_none(), "and has no fallback: {leg}");
    let seam = seam_of(leg);
    let text = hop_str(&seam, "needs_yes");
    let back: Value = serde_json::from_str(&text).expect("needs_yes is JSON text");
    assert_eq!(
        back, all,
        "the slow stamp's yes-values reach the seam: {text}"
    );
    assert!(
        seam["header"].get("fallback").is_none(),
        "{}",
        seam["header"]
    );

    // Off (shipped): no yes-values.
    let off = stamp_of(&[("needs", four_needs())], late(answers(0.91)));
    assert!(
        off["stamp"].get("needs_yes").is_none(),
        "shipped off, no yes-values: {off}"
    );
}
