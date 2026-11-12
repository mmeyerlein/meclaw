//! GH #1463 (D58 form) -- the argus reads `prefix_breaks` and `window_lost`.
//!
//! WHY this file exists: the curator counts two things about its own window
//! that should never happen -- a call whose window does not begin with the
//! one before (`window.prefix_breaks`, a cache write nobody asked for) and an
//! element a window lost without a summary covering it (`window.window_lost`).
//! A number the watcher cannot name is a number nobody watches: the meter
//! reads a role's figures off a fixed table, and a figure missing there is
//! out of reach of every charter row, whatever threshold a colony would set.
//!
//! Both are role FIGURES (GH #1426): a role goal on either asks the `window`
//! kind and reads the sum off it, whole. The curator answers the two keys
//! only when its marks carry the watch (`snapshot_window` on); without them
//! the figure is unknown, never zero -- like a figure over nothing counted.
//!
//! Table tests against the shipped `script_inline` (`gh927/harness.rs`).

// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use harness::{METER, call, can_run};
use meclaw_core::serde_json::{Value, json};

fn goal(metric: &str, gate: &str) -> Value {
    json!({"id": format!("goal:talky-{metric}"), "metric": metric, "source": "talky",
           "direction": "lower", "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 10, "quality_gate": gate, "enabled": 1})
}

fn window(watch: Value) -> Value {
    let mut w = json!({"n": 40, "p50": 30000, "p95": 95000, "max": 120000, "cached_pct": 60,
                       "cached_n": 40, "cold_pct": 5, "cost_cents_sum": 120, "raw_p95": 70000});
    for (k, v) in watch.as_object().expect("an object") {
        w[k] = v.clone();
    }
    w
}

/// Pin: both sums are read off the `window` figure, whole -- a count of
/// calls and a count of elements, no rate over `calls`.
#[test]
fn gh_d58_form_the_argus_reads_prefix_breaks_and_window_lost() {
    if !can_run() {
        return;
    }
    let counts = json!({"calls": 40,
                        "window": window(json!({"prefix_breaks": 3, "window_lost": 2,
                                                "thinking_dropped": 0}))});
    let r = call(
        METER,
        "rates",
        json!([{"counts": counts}, "prefix_breaks", "window_lost"]),
    );
    assert_eq!(r["rate"], json!(3.0), "{r}");
    assert_eq!(r["gate_rate"], json!(2.0), "{r}");
    // The number that should be read: a clean window is a measured 0.
    let clean = json!({"calls": 40,
                       "window": window(json!({"prefix_breaks": 0, "window_lost": 0,
                                               "thinking_dropped": 0}))});
    let r = call(
        METER,
        "rates",
        json!([{"counts": clean}, "prefix_breaks", "window_lost"]),
    );
    assert_eq!(r["rate"], json!(0.0), "{r}");
    assert_eq!(r["gate_rate"], json!(0.0), "{r}");
}

/// Lock: a curator that does not watch its window answers no such key, and
/// the meter reads that as unknown -- never as the 0 that means "clean". So
/// does a figure over no calls at all.
#[test]
fn gh_d58_form_the_argus_reads_an_unwatched_window_as_unknown() {
    if !can_run() {
        return;
    }
    for counts in [
        json!({"calls": 40, "window": window(json!({}))}),
        json!({"calls": 0, "window": {"n": 0, "prefix_breaks": 0, "window_lost": 0}}),
    ] {
        let r = call(
            METER,
            "rates",
            json!([{"counts": counts}, "prefix_breaks", "window_lost"]),
        );
        assert_eq!(r["rate"], Value::Null, "{r}");
        assert_eq!(r["gate_rate"], Value::Null, "{r}");
    }
}

/// Pin: a charter row that names one of them gets its number -- the goal
/// asks the `window` kind on the `stats` lane, and the measured window hands
/// the value on to the threshold (`alert_at`) and to a quality gate.
#[test]
fn gh_d58_form_the_argus_reads_them_for_a_goal_of_the_charter() {
    if !can_run() {
        return;
    }
    for (metric, gate) in [("prefix_breaks", ""), ("window_lost", "prefix_breaks")] {
        assert_eq!(
            call(METER, "stats_kinds", json!([goal(metric, gate)])),
            json!(["calls", "window"]),
            "{metric}/{gate}"
        );
    }
    let measured = json!({"rates": {"prefix_breaks": 3.0, "window_lost": 2.0}});
    for (metric, value) in [("prefix_breaks", 3.0), ("window_lost", 2.0)] {
        assert_eq!(
            call(METER, "value_of", json!([measured, metric])),
            json!(value),
            "{metric}"
        );
    }
}
