//! GH #1426 -- the argus meter knows the curator's three new figures: a role
//! goal on `window_p95`, `rebuild_rate`, `gc_share`, `tool_share` or
//! `recall_rows` asks the `stats` lane for the kind that carries it, and the
//! answer is read off that kind's object (`window.p95`, `rebuild.rate_pct`,
//! `gc.active_pct`, `gc.tool_share_pct`, `gc.recall_rows_p95`). A figure over
//! nothing counted (`n` 0) is unknown, never zero. The charter is read with
//! its `alert_at` column.
//!
//! Table tests against the shipped `script_inline` (`gh927/harness.rs`).

// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use harness::{METER, call, can_run, run, store_answer, store_call};
use meclaw_core::serde_json::{self, Value, json};

fn goal(metric: &str, gate: &str) -> Value {
    json!({"id": format!("goal:talky-{metric}"), "metric": metric, "source": "talky",
           "direction": "lower", "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 10, "quality_gate": gate, "enabled": 1})
}

fn figures() -> Value {
    json!({
        "calls": 40,
        "window": {"n": 40, "p50": 30000, "p95": 95000, "max": 120000, "cached_pct": 60,
                   "cold_pct": 10, "cost_cents_sum": 120, "raw_p95": 70000},
        "rebuild": {"n": 40, "by_mark": {"none": 30, "target": 10}, "rate_pct": 25},
        "gc": {"n": 8, "classes": {"shrunk_tools": 12}, "active_pct": 75,
               "tool_share_pct": 35, "recall_rows_p95": 6}
    })
}

#[test]
fn gh_d38_k1_the_meter_asks_the_new_kinds() {
    if !can_run() {
        return;
    }
    for (metric, gate, kinds) in [
        ("window_p95", "", json!(["calls", "window"])),
        ("rebuild_rate", "", json!(["calls", "rebuild"])),
        ("gc_share", "tool_share", json!(["calls", "gc"])),
        ("tool_share", "", json!(["calls", "gc"])),
        (
            "recall_rows",
            "window_p95",
            json!(["calls", "gc", "window"]),
        ),
    ] {
        assert_eq!(
            call(METER, "stats_kinds", json!([goal(metric, gate)])),
            kinds,
            "{metric}/{gate}"
        );
    }
    let m = call(
        METER,
        "stats_ask",
        json!([goal("window_p95", "gc_share"), 60, "wait:x", {}]),
    );
    let asked: Value = serde_json::from_str(
        m["header"]["stats_kinds"]
            .as_str()
            .expect("the kinds as JSON text"),
    )
    .expect("JSON");
    assert_eq!(asked, json!(["calls", "window", "gc"]), "{m}");
}

#[test]
fn gh_d38_k1_a_figure_is_read_off_its_kind() {
    if !can_run() {
        return;
    }
    for (metric, gate, rate, gate_rate) in [
        ("window_p95", "rebuild_rate", json!(95000.0), json!(25.0)),
        ("gc_share", "tool_share", json!(75.0), json!(35.0)),
        ("recall_rows", "", json!(6.0), Value::Null),
    ] {
        let r = call(METER, "rates", json!([{"counts": figures()}, metric, gate]));
        assert_eq!(r["rate"], rate, "{metric}: {r}");
        assert_eq!(r["gate_rate"], gate_rate, "{gate}: {r}");
        assert_eq!(r["calls"], 40, "{r}");
    }
    // A figure over nothing counted -- an old ledger -- is unknown.
    let empty = json!({"calls": 3,
                       "window": {"n": 0, "p50": 0, "p95": 0, "max": 0, "cached_pct": 0,
                                  "cold_pct": 0, "cost_cents_sum": 0, "raw_p95": 0}});
    let r = call(METER, "rates", json!([{"counts": empty}, "window_p95", ""]));
    assert_eq!(r["rate"], Value::Null, "{r}");
    // The int counts stay what they were beside the figures.
    let window = figures()["window"].clone();
    let r = call(
        METER,
        "rates",
        json!([{"counts": {"calls": 40, "gap": 10, "window": window}},
               "gap_rate", "window_p95"]),
    );
    assert_eq!(r["rate"], json!(25.0), "{r}");
    assert_eq!(r["gate_rate"], json!(95000.0), "{r}");
    assert_eq!(r["counts"], json!({"calls": 40, "gap": 10}), "{r}");
}

#[test]
fn gh_d38_k1_the_charter_is_read_with_its_alert_column() {
    if !can_run() {
        return;
    }
    let out = run(METER, store_answer(json!([]), "stale", json!({})));
    let args = store_call(&out, "select", "goals").expect("the charter is read");
    assert!(
        args["columns"]
            .as_array()
            .is_some_and(|c| c.contains(&json!("alert_at"))),
        "{args}"
    );
}
