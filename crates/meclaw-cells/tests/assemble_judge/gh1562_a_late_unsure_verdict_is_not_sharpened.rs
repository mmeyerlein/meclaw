//! GH #1562 (OR-54g-261 (2), OR-54g-263) -- nothing is sharpened for a turn that
//! has already left.
//!
//! WHY this file exists: the collector waits for the stamp at most
//! `route_wait_ms`, then the turn fires without it. An unsure first verdict
//! that arrived after that still ordered a view from the curator (`view_ask`),
//! called the second judge and, with a wait bound, sent a route-hold notice:
//! a model call and a hold for a turn that was gone. The route cell now takes
//! the SAME bound (`route_wait_ms`, the set writes one value into both cells)
//! and sharpens only while `now - asked_at` is under it. Past it the verdict is
//! stamped at once -- path on the fallback as an unsure verdict without a
//! second judge, its choices (the risk) and their notes whole -- and the rim
//! copy carries the risk to the guard. 0 (shipped) is no bound: the cell
//! sharpens as before.
//!
//! The moments are injected on the hop (`asked_at` an hour ago or an hour
//! ahead); no assertion reads a clock. Runs the shipped `talky/route` script
//! (`support/assemble_cell.rs`); the decider is a fixture, no model is called.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const ASSEMBLE: &str = "templates/collector/assemble/config.json";
const TEXT: &str = "Please put the drive to my mother in the calendar for two o'clock.";
/// The bound of these pins, in ms.
const BOUND: i64 = 2000;

fn risk_choice() -> Value {
    json!({"risk": {
        "question": "How is the person doing, as far as their safety goes?",
        "options": {"none": "no sign of distress", "acute": "in danger now"},
        "fallback": "unknown",
        "threshold": 0,
        "notes": {"acute": "CRISIS: give the emergency number now."}
    }})
}

/// A composition that sharpens an unsure class with a second judge, asks the
/// risk and copies every stamp to the rim; `bound` is `route_wait_ms`
/// (None: the shipped value).
fn knobs(bound: Option<i64>) -> Vec<(&'static str, Value)> {
    let mut all = vec![
        ("sure", json!({"path": 0.9})),
        ("sharpen", json!(["path"])),
        ("second_judge", json!("judge_two")),
        ("second_wait_ms", json!(5000)),
        ("choices", risk_choice()),
        ("stamp_out", json!("1")),
    ];
    if let Some(b) = bound {
        all.push(("route_wait_ms", json!(b)));
    }
    all
}

/// An unsure first verdict (`simple` 0.7 under the bar 0.9) with the risk
/// `acute`, asked `ago_ms` before now (negative: ahead).
fn unsure_decision(ago_ms: i64) -> Value {
    let hop = json!({"finish_reason": "stop",
                     "asked_at": (now_ms() - ago_ms).to_string(),
                     "state": TEXT});
    let ctx = json!({"turn_id": "c-1", "session_id": "s-1", "channel": "phone",
                     "audience_set": "[]"});
    let mut doc = lane("in_decision", hop, ctx, json!([]));
    doc["decision"] = json!({"answers": {
        "path": {"choice": "simple", "p": 0.7},
        "intent": {"choice": "task", "p": 0.8},
        "choices.risk": {"choice": "acute", "p": 0.97}
    }, "missing": [], "model": "d"});
    doc
}

fn routes(out: &[Value]) -> Vec<String> {
    out.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or("").to_string())
        .collect()
}

fn on<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| panic!("no {route} in {out:?}"))
}

const HOUR: i64 = 3_600_000;

/// Pin (v): past the bound, an unsure verdict starts no view_ask, no second
/// judge and no route-hold notice; it is stamped with its risk, and the rim
/// copy carries it to the guard.
#[test]
fn gh1562_a_late_unsure_verdict_is_stamped_at_once() {
    let out = run_cell(ROUTE, &knobs(Some(BOUND)), unsure_decision(HOUR)).0;
    assert_eq!(
        routes(&out),
        vec!["stamp".to_string(), "stamp_out".to_string()],
        "a turn that has left is not sharpened: {out:?}"
    );
    assert!(
        !out.iter().any(|m| m["header"].get("wait_ms").is_some()),
        "no route-hold notice: {out:?}"
    );
    let stamp = on(&out, "stamp");
    assert_eq!(stamp["header"]["fallback"], "late", "{stamp}");
    assert_eq!(stamp["stamp"]["path"], "talk", "{stamp}");
    assert_eq!(
        stamp["stamp"]["choices"]["risk"]["value"], "acute",
        "{stamp}"
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
        "the crisis note rides the stamp: {stamp}"
    );
    let rim = on(&out, "stamp_out");
    assert_eq!(
        rim["stamp"]["choices"]["risk"]["value"], "acute",
        "the guard's copy carries the risk: {rim}"
    );
}

/// Pin (vi): the same verdict inside the bound is sharpened as before.
#[test]
fn gh1562_an_unsure_verdict_in_time_is_sharpened() {
    let out = run_cell(ROUTE, &knobs(Some(BOUND)), unsure_decision(-HOUR)).0;
    assert_eq!(routes(&out), vec!["view_ask".to_string()], "{out:?}");
    assert_eq!(on(&out, "view_ask")["header"]["wait_ms"], "5000", "{out:?}");
}

/// Pin (vii): `route_wait_ms` 0 (shipped, as in the collector) is no bound:
/// a verdict an hour late is sharpened, as in 0.89.0.
#[test]
fn gh1562_no_bound_sharpens_as_before() {
    let route = config_of(ROUTE);
    let collector = config_of(ASSEMBLE);
    assert_eq!(route["params"]["route_wait_ms"], json!(0), "shipped");
    assert_eq!(
        route["params"]["route_wait_ms"], collector["params"]["route_wait_ms"],
        "the same default as the collector"
    );
    let said = route["contract"]["settings"]["route_wait_ms"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        said.contains(
            "must equal collector/assemble route_wait_ms; the set writes one value into both"
        ),
        "{said}"
    );
    for over in [knobs(None), knobs(Some(0))] {
        let out = run_cell(ROUTE, &over, unsure_decision(HOUR)).0;
        assert_eq!(routes(&out), vec!["view_ask".to_string()], "{out:?}");
    }
}
