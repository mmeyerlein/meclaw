//! GH #1426 -- a goal measured at a role raises an `alert` when its value
//! lies over the charter's `alert_at` (an additive `int` column of `goals`,
//! NULL = no alarm), deterministically and with no judge asked:
//! `{goal, metric, value, window_minutes, role}` on the hive's `alert` lane.
//! A role goal without `alert_at` and every goal of the colony read as before.
//!
//! Table tests against the shipped `script_inline` (`gh927/harness.rs`): the
//! meter resumed in phase `wait` with the answer of a `stats` ask.

// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use harness::{
    METER, assert_the_declaration_admits, can_run, config, on_route, run, store_answer, store_call,
};
use meclaw_core::serde_json::{self, Value, json};

fn goal(direction: &str, alert_at: Value) -> Value {
    json!({"id": "goal:talky-window_p95", "metric": "window_p95", "source": "talky",
           "direction": direction, "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 10, "quality_gate": "", "enabled": 1, "alert_at": alert_at})
}

fn answer(p95: u64) -> Value {
    json!({"stats": {"from": "a", "to": "b",
                     "counts": {"calls": 12,
                                "window": {"n": 12, "p50": 40000, "p95": p95, "max": p95,
                                           "cached_pct": 70, "cold_pct": 8,
                                           "cost_cents_sum": 30, "raw_p95": 0}},
                     "samples": [], "truncated": false},
           "error_code": "", "detail": "", "stats_tag": "wait:s1"})
}

fn resume(goal: Value, answer: Value) -> Vec<Value> {
    let carry = json!({"goal": goal, "prices": {}, "rules": [], "cycle_id": "cycle:7",
                       "ask": "baseline"});
    let rows = json!([{"id": "wait:s1", "at": "2026-01-01T00:00:00.000000Z", "kind": "stats",
                       "carry": carry, "status": "open"}]);
    run(METER, store_answer(rows, "wait", answer))
}

fn alert_text(out: &[Value]) -> Value {
    let alerts = on_route(out, "alert");
    assert_eq!(alerts.len(), 1, "exactly one alert: {out:?}");
    serde_json::from_str(alerts[0]["messages"][0]["text"].as_str().expect("a text")).expect("JSON")
}

#[test]
fn gh_d38_k1_a_role_goal_over_its_threshold_alerts_without_a_judge() {
    if !can_run() {
        return;
    }
    let out = resume(goal("observe", json!(100000)), answer(120000));
    assert_eq!(
        alert_text(&out),
        json!({"goal": "goal:talky-window_p95", "metric": "window_p95", "value": 120000.0,
               "window_minutes": 1440, "role": "talky"})
    );
    assert!(
        on_route(&out, "judge").is_empty() && store_call(&out, "select", "hints").is_none(),
        "no model anywhere on this path: {out:?}"
    );
    assert_the_declaration_admits(METER, &out);

    // An optimisation goal over its threshold alerts in the same breath: the
    // alert does not wait for the judge.
    let out = resume(goal("lower", json!(100000)), answer(120000));
    assert_eq!(alert_text(&out)["value"], 120000.0);
    assert!(on_route(&out, "judge").is_empty(), "{out:?}");
    assert_the_declaration_admits(METER, &out);
}

#[test]
fn gh_d38_k1_at_or_under_the_threshold_or_without_one_nothing_alerts() {
    if !can_run() {
        return;
    }
    for (g, p95) in [
        (goal("observe", json!(100000)), 100000),
        (goal("observe", json!(100000)), 90000),
        (goal("observe", Value::Null), 500000),
    ] {
        let out = resume(g.clone(), answer(p95));
        assert!(on_route(&out, "alert").is_empty(), "{g} at {p95}: {out:?}");
    }
    // Below `min_samples` nothing is concluded, and nothing alerts.
    let mut g = goal("observe", json!(100000));
    g["min_samples"] = json!(30);
    let out = resume(g, answer(120000));
    assert!(on_route(&out, "alert").is_empty(), "{out:?}");
}

#[test]
fn gh_d38_k1_the_charter_carries_alert_at_as_an_int_column() {
    if !can_run() {
        return;
    }
    let cfg = config("../../templates/argus/charter/config.json");
    assert_eq!(cfg["params"]["schema"]["goals"]["alert_at"], "int");
    let seed = std::fs::read_to_string("../../templates/argus/charter/seed/goals.jsonl")
        .expect("the goals seed");
    let schema: Value =
        serde_json::from_str(seed.lines().next().expect("a schema line")).expect("JSON");
    assert_eq!(schema["schema"]["alert_at"], "int", "{schema}");
}
