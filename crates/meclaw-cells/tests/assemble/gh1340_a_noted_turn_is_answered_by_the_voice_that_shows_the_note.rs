//! GH #1340 -- a turn whose stamp carries a choice's note leaves on the class
//! whose voice shows the note, never behind a bridge.
//!
//! WHY this file exists: a product's choice note (GH #1304) rides on the stamp
//! as a turn note, and the curator shows turn notes only on the ways with a
//! notes share -- `brain` (talky 0.02). The quick role has none (`notes_share`
//! 0), and a consult or research turn is first spoken to by the quick bridge,
//! whose window is the person's turn plus `instructions.bridge`. Measured on a
//! canary colony (meclaw 0.76.2): decide answered `choices.risk` = acute
//! (p 1.0) with `path` complex (p 0.67); the bridge (quick, mimo) spoke first
//! without the note -- no emergency number, no helpline -- and the guidance
//! reached the person only a round later through consult -> cogny -> brain.
//! What this file pins, at the route cell:
//!
//! 1. a chosen value with a note steers the stamp's path to `notes_class`
//!    (`talk` as shipped), whatever path the decider named;
//! 2. a chosen value without a note leaves the path alone;
//! 3. `notes_class` "" (or a class the cell does not know) steers nothing.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{lane, now_ms, run_cell};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

/// The choice of the canary's set, cut to two of its options: `acute` with a
/// note, `none` without one; fallback outside, threshold 0 (as the set has it).
fn risk() -> Value {
    json!({
        "risk": {
            "question": "How is the person doing in this turn, as far as their safety goes?",
            "options": {"none": "no sign of distress",
                        "acute": "says they want to hurt themselves now"},
            "fallback": "unknown",
            "threshold": 0,
            "notes": {"acute": "The person may be in danger right now. Ask whether they are safe."}
        }
    })
}

/// The decider's answer in the trace form of the canary (decide rowid 24779).
fn answers(path: &str, risk: &str) -> Value {
    json!({
        "path": {"choice": path, "p": 0.67},
        "intent": {"choice": "chat", "p": 0.8},
        "families.history": {"yes": 0.9},
        "choices.risk": {"choice": risk, "p": 1.0}
    })
}

fn decision(answers: Value) -> Value {
    // Asked an hour ahead: the duration clamps to 0, never late (dev-rules § 11).
    let asked_at = now_ms() + 3_600_000;
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": asked_at.to_string()}),
        json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"}),
        json!([]),
    );
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn stamp_of(over: &[(&str, Value)], answers: Value) -> Value {
    let out = run_cell(ROUTE, over, decision(answers)).0;
    out.iter()
        .find(|m| m["header"]["route"] == "stamp")
        .unwrap_or_else(|| panic!("no stamp in {out:?}"))["stamp"]
        .clone()
}

#[test]
fn gh1340_a_noted_complex_turn_is_stamped_talk() {
    let s = stamp_of(&[("choices", risk())], answers("complex", "acute"));
    assert_eq!(
        s["turn_notes"][0]["id"], "route.risk",
        "the note rides on the stamp: {s}"
    );
    assert_eq!(
        s["path"], "talk",
        "a noted turn never goes behind the quick bridge: {s}"
    );
    assert_eq!(
        s["p"], 1.0,
        "the confidence behind the steering is the choice's: {s}"
    );
    let out = run_cell(
        ROUTE,
        &[("choices", risk())],
        decision(answers("complex", "acute")),
    )
    .0;
    assert_eq!(out[0]["header"]["steered"], "route.risk", "{:?}", out[0]);
}

#[test]
fn gh1340_a_noted_simple_and_research_turn_are_stamped_talk() {
    for path in ["simple", "research"] {
        let s = stamp_of(&[("choices", risk())], answers(path, "acute"));
        assert_eq!(
            s["path"], "talk",
            "{path}: the quick role shows no note: {s}"
        );
    }
}

#[test]
fn gh1340_a_value_without_a_note_keeps_the_path() {
    let s = stamp_of(&[("choices", risk())], answers("complex", "none"));
    assert_eq!(s["turn_notes"], json!([]), "{s}");
    assert_eq!(s["path"], "complex", "{s}");
    assert_eq!(s["p"], 0.67, "{s}");
}

#[test]
fn gh1340_notes_class_empty_or_unknown_steers_nothing() {
    for class in ["", "nowhere"] {
        let s = stamp_of(
            &[("choices", risk()), ("notes_class", json!(class))],
            answers("complex", "acute"),
        );
        assert_eq!(s["path"], "complex", "notes_class {class:?}: {s}");
        assert_eq!(s["turn_notes"][0]["id"], "route.risk", "{s}");
    }
}

#[test]
fn gh1340_the_shipped_route_steers_to_talk() {
    let cfg: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(ROUTE),
        )
        .expect("route config"),
    )
    .expect("json");
    assert_eq!(cfg["params"]["notes_class"], "talk", "{}", cfg["params"]);
    assert!(
        cfg["contract"]["settings"]["notes_class"].is_object(),
        "a declared setting: {}",
        cfg["contract"]["settings"]
    );
}
