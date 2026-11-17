//! GH #1562 -- a late stamp keeps the product choices its decider answered.
//!
//! WHY this file exists: a decide answer later than `late_ms` left the route
//! cell on the fallback `late`, and the stamp built its choices out of `{}`:
//! every choice stood on its declared fallback. Measured on a member colony
//! (decide 1585 ms against `late_ms` 1500): the decider had answered the risk
//! question, the stamp said `unknown`, so no crisis note reached the window, the
//! guard wrote only an `unknown` receipt and no `crisis` reached care. A choice
//! is its own question, like the yes-values (GH #1524): late or not, its value
//! and its note stand.
//!
//! Since the one bound (GH #1562 (b)): the route cell cuts no verdict as late
//! at all -- `late_ms` is gone, the collector's `route_wait_ms` is the one
//! bound. The decider's answer an hour after the ask is a judged stamp, its
//! risk and its crisis note whole, on the stamp and on the rim copy the guard
//! reads. Whether it still reaches the turn is the collector's word
//! (`gh1562_a_stamp_after_the_fire_acts_on_nothing`); the guard gets it either way.
//!
//! Runs the shipped `talky/route` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

fn risk_choice() -> Value {
    json!({"risk": {
        "question": "How is the person doing, as far as their safety goes?",
        "options": {"none": "no sign of distress", "acute": "in danger now"},
        "fallback": "unknown",
        "threshold": 0,
        "notes": {"acute": "CRISIS: give the emergency number now."}
    }})
}

/// The decider's answer for turn `c-1`, asked an hour ago: past any bound.
fn late_decision(risk: &str) -> Value {
    let hop = json!({"finish_reason": "stop",
                     "asked_at": (now_ms() - 3_600_000).to_string()});
    let ctx = json!({"turn_id": "c-1", "session_id": "s-1", "channel": "phone",
                     "audience_set": "[]"});
    let mut doc = lane("in_decision", hop, ctx, json!([]));
    doc["decision"] = json!({"answers": {
        "path": {"choice": "talk", "p": 0.9},
        "intent": {"choice": "chat", "p": 0.8},
        "choices.risk": {"choice": risk, "p": 0.97}
    }, "missing": [], "model": "d"});
    doc
}

#[test]
fn gh1562_a_late_stamp_keeps_its_risk() {
    let over = [("choices", risk_choice()), ("stamp_out", json!("1"))];
    let out = run_cell(ROUTE, &over, late_decision("acute")).0;
    let stamp = out
        .iter()
        .find(|m| m["header"]["route"] == "stamp")
        .unwrap_or_else(|| panic!("no stamp: {out:?}"));
    assert!(
        stamp["header"].get("fallback").is_none(),
        "a slow verdict is stamped judged, never cut as late: {stamp}"
    );
    assert_eq!(stamp["stamp"]["path"], "talk", "{stamp}");
    assert_eq!(
        stamp["stamp"]["choices"]["risk"]["value"], "acute",
        "the late stamp keeps the risk its decider answered: {stamp}"
    );
    assert_eq!(
        stamp["stamp"]["choices"]["risk"]["source"], "model",
        "{stamp}"
    );
    let notes = stamp["stamp"]["turn_notes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        notes.iter().any(|n| n["id"] == "route.risk"),
        "the crisis note rides the late stamp: {stamp}"
    );
    // The rim copy is what the guard reads (`crisis` to care).
    let rim = out
        .iter()
        .find(|m| m["header"]["route"] == "stamp_out")
        .unwrap_or_else(|| panic!("no rim copy: {out:?}"));
    assert_eq!(
        rim["stamp"]["choices"]["risk"]["value"], "acute",
        "the guard's copy carries the risk: {rim}"
    );
}
