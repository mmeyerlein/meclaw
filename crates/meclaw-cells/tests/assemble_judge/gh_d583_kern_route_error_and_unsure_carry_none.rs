//! D58-3 kern (GH #1524, contract point 2, N1) -- an error stamp and an
//! unsure stamp carry no yes-values.
//!
//! WHY this file exists: the late stamp gains the yes-values of its answers
//! because its decider answered. An error stamp has no answers, and an unsure
//! stamp is one the route cell itself does not stand behind; neither carries
//! `needs_yes`, with `stamp_needs_yes` on as off. This pin keeps the change
//! for `late` from spreading to the other fallbacks.
//!
//! Runs the shipped `talky/route` script (`support/assemble_cell.rs`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";

fn four_needs() -> Value {
    json!({"tool": "a tool call",
           "reasoning": "careful step-by-step reasoning",
           "research": "a deep search over many sources",
           "memory": "something the person said earlier or before today"})
}

fn decision(ago_ms: i64, hop: Value, answers: Value) -> Value {
    let mut h = json!({"finish_reason": "stop", "asked_at": (now_ms() - ago_ms).to_string()});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    let mut doc = lane("in_decision", h, json!({"turn_id": "c-1"}), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
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

#[test]
fn gh_d583_kern_route_error_and_unsure_carry_none() {
    let on = [("needs", four_needs()), ("stamp_needs_yes", json!("1"))];
    // In time (asked ahead): never late.
    let unsure = stamp_of(&on, decision(-3_600_000, json!({}), answers(0.3)));
    assert_eq!(unsure["header"]["fallback"], "unsure", "{unsure}");
    let error = stamp_of(
        &on,
        decision(
            -3_600_000,
            json!({"finish_reason": "error", "error_code": "decide_failed"}),
            json!({}),
        ),
    );
    assert_eq!(error["header"]["fallback"], "error", "{error}");
    // An error with answers beside it is still an error: no judgement.
    let error_with = stamp_of(
        &on,
        decision(
            -3_600_000,
            json!({"finish_reason": "error", "error_code": "decide_failed"}),
            answers(0.91),
        ),
    );
    assert_eq!(error_with["header"]["fallback"], "error", "{error_with}");
    for m in [&unsure, &error, &error_with] {
        assert!(
            m["stamp"].get("needs_yes").is_none(),
            "a stamp on the fallback {} carries no yes-values: {m}",
            m["header"]["fallback"]
        );
        assert_eq!(m["stamp"]["needs"], json!([]), "{m}");
    }
}
