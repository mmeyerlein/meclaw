//! GH #1444 -- two numbers the curator already answers were out of the
//! watcher's reach: the share of cold calls (`window.cold_pct`, a call whose
//! cache covered less than the cold share of its window) and the cost of the
//! rebuilds (`rebuild.cost_cents_sum`, the cents of the calls that rebuilt).
//! A charter row could not name either, so the gate of the next wave (a cold
//! share over thirty percent) had no metric to stand on.
//!
//! Both are role FIGURES (GH #1426): a role goal on `cold_pct` asks the
//! `window` kind and reads `window.cold_pct`; one on `rebuild_cost` asks the
//! `rebuild` kind and reads `rebuild.cost_cents_sum`. A figure over nothing
//! counted (`n` 0) is unknown, never zero; `cold_pct` stands on a narrower
//! base, the calls whose usage measured a cache (`window.cached_n`), and over
//! none of those -- or without the base at all -- it is unknown as well.
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

fn figures() -> Value {
    json!({
        "calls": 40,
        "window": {"n": 40, "p50": 30000, "p95": 95000, "max": 120000, "cached_pct": 60,
                   "cached_n": 40, "cold_pct": 35, "cost_cents_sum": 120, "raw_p95": 70000},
        "rebuild": {"n": 40, "by_mark": {"none": 30, "target": 10}, "rate_pct": 25,
                    "cost_cents_sum": 48}
    })
}

#[test]
fn gh_d38_w1_cold_pct_and_rebuild_cost_ask_their_kinds() {
    if !can_run() {
        return;
    }
    for (metric, gate, kinds) in [
        ("cold_pct", "", json!(["calls", "window"])),
        ("rebuild_cost", "", json!(["calls", "rebuild"])),
        (
            "cold_pct",
            "rebuild_cost",
            json!(["calls", "window", "rebuild"]),
        ),
    ] {
        assert_eq!(
            call(METER, "stats_kinds", json!([goal(metric, gate)])),
            kinds,
            "{metric}/{gate}"
        );
    }
}

#[test]
fn gh_d38_w1_cold_pct_and_rebuild_cost_are_read_off_their_figures() {
    if !can_run() {
        return;
    }
    let r = call(
        METER,
        "rates",
        json!([{"counts": figures()}, "cold_pct", "rebuild_cost"]),
    );
    assert_eq!(r["rate"], json!(35.0), "{r}");
    assert_eq!(r["gate_rate"], json!(48.0), "{r}");
    // Over nothing counted, unknown -- an old ledger, or a rebuild figure
    // written before it carried its cost.
    let empty = json!({"calls": 3,
                       "window": {"n": 0, "cold_pct": 0},
                       "rebuild": {"n": 4, "by_mark": {"none": 4}, "rate_pct": 0}});
    let r = call(
        METER,
        "rates",
        json!([{"counts": empty}, "cold_pct", "rebuild_cost"]),
    );
    assert_eq!(r["rate"], Value::Null, "{r}");
    assert_eq!(r["gate_rate"], Value::Null, "{r}");
}

#[test]
fn gh_d38_w1_cold_pct_over_no_measured_cache_is_unknown() {
    if !can_run() {
        return;
    }
    // Review d38_o11-r1 I-1: the curator takes `cold_pct` over the calls whose
    // usage measured a cache and answers `pct(0, 0)` = 0 when none did. Forty
    // calls with a window and not one cache stamp is no cold share of 0 --
    // read as 0.0 it would let the next wave's gate (cold share over 30) pass
    // blind. The base the curator answers decides, not `window.n`.
    for window in [
        json!({"n": 40, "p95": 95000, "cached_pct": 0, "cached_n": 0, "cold_pct": 0}),
        // A figure from before the base was answered: the base is unknown.
        json!({"n": 40, "p95": 95000, "cached_pct": 0, "cold_pct": 0}),
    ] {
        let counts = json!({"calls": 40, "window": window});
        let r = call(
            METER,
            "rates",
            json!([{"counts": counts}, "cold_pct", "window_p95"]),
        );
        assert_eq!(r["rate"], Value::Null, "{r}");
        // The other figures of the kind stay on `n`.
        assert_eq!(r["gate_rate"], json!(95000.0), "{r}");
    }
}
