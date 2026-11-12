//! GH #1426 -- sabotage mutant: a window over its target that is never rebuilt
//!
//! The marks are the curator's own (review I-1): every turn runs through the
//! shipped `./policy`, whose `curate` mark is the one `stats` reads, so a
//! renamed key of that mark breaks this pin instead of blinding the watch in
//! silence. The curator is broken by its knob, not rebuilt for the test:
//! `window_target` 0 never orders the rebuild the healthy one orders at its
//! target. The model's window grows by one step a turn and falls back to its
//! floor after a rebuild ran, as a model's prompt does. The hive's own `stats`
//! door counts the trail, and the argus meter -- the shipped script, fed that
//! very answer -- raises the alert of a role goal whose `alert_at` the broken
//! invariant crosses. The healthy trail beside it raises nothing, so the alert
//! is the invariant's, not the threshold's. Deterministic: no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use curator_hive::{Hive, ROUND_E, clear_of_midnight, mode, read_json, repo, said, shipped, user};
use harness::{METER, can_run, on_route, run, store_answer};
use meclaw_core::serde_json::{self, Value, json};

const TURNS: u32 = 12;
const FLOOR: u64 = 3000;
const STEP: u64 = 800;
const TARGET: u64 = 4000;
const ALERT_AT: u64 = 6000;

/// The talky instance as it ships (the marker's overrides), plus `more`.
fn talky(more: &[(&str, &str, Value)]) -> Hive {
    let marker = read_json(&repo("templates/talky/curator/config.json"));
    let mut over: Vec<(String, String, Value)> = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            over.push((cell.clone(), k, v));
        }
    }
    let mut all: Vec<(&str, &str, Value)> = over
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    all.extend(more.iter().cloned());
    Hive::with(&all).reporting_composite("talky")
}

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

/// One turn of `window` tokens through the shipped policy. The user turn
/// holds more characters than three per token of the window, so the policy's
/// own projection lies over the measure and the measure is the window. The
/// package keeps soft, hard and the cost rule out of it (a cached price of 0):
/// only the role's target can order a rebuild. Returns the turn's mark; an
/// ordered rebuild is fired, the way the clock would.
fn turn(h: &mut Hive, i: u32, window: u64) -> String {
    let chars = usize::try_from(window * 3 + 600).expect("a size");
    let call = h.curate(
        "s",
        &format!("t{i}"),
        0,
        json!([user(&"x".repeat(chars))]),
        mode("Be brief."),
    );
    let usage = json!({"tokens_prompt": window, "tokens_cached": window / 2,
                       "input_soft": 5_000_000, "input_hard": 10_000_000,
                       "cost_in": 10, "cost_cached_in": 0});
    h.tap(&call, "stop", usage, json!([said("a")]));
    let s = stamp(h);
    assert_eq!(
        s["window_tokens"],
        json!(window),
        "turn {i}: {s} {:?}",
        h.stderr
    );
    let mark = s["curate_mark"].as_str().unwrap_or("").to_string();
    if mark != "none" {
        let order = h
            .clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("turn {i}: `{mark}` orders a rebuild: {s}"));
        h.fire(&order);
    }
    mark
}

/// `TURNS` turns, the window one `STEP` larger each, back on the `FLOOR`
/// after a rebuild. The marks of the policy, in order.
fn trail(h: &mut Hive) -> Vec<String> {
    let mut window = FLOOR;
    let mut marks = Vec::new();
    for i in 0..TURNS {
        let mark = turn(h, i, window);
        window = if mark == "none" { window + STEP } else { FLOOR };
        marks.push(mark);
    }
    marks
}

/// The curator's `stats` answer to `kinds` over the last day and the next.
fn stats(h: &mut Hive, kinds: &[&str]) -> Value {
    let now = chrono::Utc::now();
    let day = chrono::Duration::days(1);
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "wait:s1",
               "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_kinds": serde_json::to_string(&kinds).unwrap()}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    m.body["stats"].clone()
}

/// That answer handed to the meter as the answer of a `stats` ask of a role
/// goal; what the meter emits.
fn watch(answer: Value, goal: Value) -> Vec<Value> {
    let answer = json!({"stats": answer, "error_code": "", "detail": "",
                        "stats_tag": "wait:s1"});
    let carry = json!({"goal": goal, "prices": {}, "rules": [], "cycle_id": "cycle:7",
                       "ask": "baseline"});
    let rows = json!([{"id": "wait:s1", "at": "2026-01-01T00:00:00.000000Z", "kind": "stats",
                       "carry": carry, "status": "open"}]);
    run(METER, store_answer(rows, "wait", answer))
}

fn goal(metric: &str, alert_at: u64) -> Value {
    json!({"id": format!("goal:talky-{metric}"), "metric": metric, "source": "talky",
           "direction": "observe", "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 0, "quality_gate": "", "enabled": 1, "alert_at": alert_at})
}

fn alerted(out: &[Value]) -> Option<Value> {
    let alerts = on_route(out, "alert");
    assert!(alerts.len() <= 1, "{out:?}");
    alerts.first().map(|m| {
        serde_json::from_str(m["messages"][0]["text"].as_str().expect("a text")).expect("JSON")
    })
}

#[test]
fn gh_d38_k1_mutant_window_without_rebuild_trips_the_watch() {
    if !shipped() || !can_run() {
        return;
    }
    // talky's horizon is the day: a turn across midnight would mark `day`.
    clear_of_midnight();
    let target = json!(TARGET);
    let mut healthy = talky(&[("policy", "window_target", target)]);
    let marks = trail(&mut healthy);
    let rebuilt = marks.iter().filter(|m| *m == "target").count();
    assert!(
        rebuilt >= 2,
        "the healthy curator rebuilds at its target: {marks:?}"
    );
    let answer = stats(&mut healthy, &["calls", "window", "rebuild"]);
    let counts = &answer["counts"];
    // The coupling: the figures are read off the policy's own marks.
    assert_eq!(counts["window"]["n"], TURNS, "{counts}");
    assert_eq!(counts["rebuild"]["n"], TURNS, "{counts}");
    assert_eq!(counts["rebuild"]["by_mark"]["target"], rebuilt, "{counts}");
    assert_eq!(counts["window"]["cached_pct"], 50, "{counts}");
    let out = watch(answer, goal("window_p95", ALERT_AT));
    assert_eq!(
        alerted(&out),
        None,
        "a curator that rebuilds stays under: {out:?}"
    );

    let mut broken = talky(&[("policy", "window_target", json!(0))]);
    let marks = trail(&mut broken);
    assert!(marks.iter().all(|m| m == "none"), "{marks:?}");
    let answer = stats(&mut broken, &["calls", "window"]);
    assert_eq!(answer["counts"]["window"]["n"], TURNS, "{answer}");
    let out = watch(answer, goal("window_p95", ALERT_AT));
    let alert = alerted(&out).unwrap_or_else(|| panic!("the watch trips: {out:?}"));
    assert_eq!(alert["metric"], "window_p95", "{alert}");
    assert_eq!(alert["role"], "talky", "{alert}");
    assert!(
        alert["value"].as_f64().unwrap_or(0.0) > ALERT_AT as f64,
        "{alert}"
    );
}
