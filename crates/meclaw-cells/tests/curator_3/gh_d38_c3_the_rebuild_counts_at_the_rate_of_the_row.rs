//! GH #1450 (D38 C3b Z1.10, OR-d38-3.c3b.5) -- the rebuild counts at the
//! rate of the catalogue row, like the call.
//!
//! WHY this file exists: since Z1.7 the call's budget (`cut_budget`) takes
//! the catalogue row of its cell before the tap's copy of it, its
//! `chars_per_token` included (OR-d38-3.c3b.14). The rebuild read the rate
//! for its claim straight from the tap's state (`rb-probe`), so with a row
//! whose rate differs from the tap's the call estimated at one rate and the
//! rebuild planned at another. Pinned here: a rebuild of a brain with a row
//! plans exactly like a brain without a row whose tap reports the row's rate;
//! without a row the rate stays the tap's (the c0 corpus and the
//! `gh_d38_k6b_*` pins hold that byte for byte).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const SOFT: i64 = 2_000;
const HARD: i64 = 8_000;

/// One rebuild after two rounds of tool work; `row` the brain's catalogue
/// row (none when `None`), `tap_rate` the rate the tap reports.
fn rebuilt(row: Option<f64>, tap_rate: f64) -> Value {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "gc", json!("0")),
    ]);
    if let Some(rate) = row {
        h.lane(
            "in_catalog",
            json!({}),
            json!({"package_of": "/talky/brain"}),
            json!({"messages": [], "package": {
                "model": "test/rated-row", "input_soft": SOFT, "input_hard": HARD,
                "max_output": 100, "price_tiers": null, "cache_mode": null,
                "cache_ttl_s": null, "wire_dialect": "chat_completions",
                "supported_params": [], "chars_per_token": rate}}),
        );
    }
    let extra = json!({"input_soft": SOFT, "input_hard": HARD, "chars_per_token": tap_rate,
                       "cache_expires_at": "2099-01-01T00:00:00Z"});
    for (t, n) in [("t1", 1), ("t2", 2)] {
        let c = format!("c{n}");
        let call = h.curate(
            "s",
            t,
            0,
            json!([
                user(&format!("look up {t}")),
                tool_call(&c, "lookup"),
                tool_result(&c, &format!("found: {}", "r".repeat(1_500)))
            ]),
            mode("x"),
        );
        h.tap(&call, "stop", extra.clone(), json!([said("ok")]));
    }
    h.fire(&last_add(&h));
    // What the rate decides (the clock fields `at`/`as_of_ms`/`cover`/
    // `blocks_to` differ between two hives by construction).
    let plan = h.plan();
    assert!(
        plan.get("blocks").is_some(),
        "a rebuild ran and left a plan: {plan}"
    );
    let ids: Vec<Value> = plan["blocks"]
        .as_array()
        .map(|b| b.iter().map(|x| x["id"].clone()).collect())
        .unwrap_or_default();
    json!({"blocks": ids, "shrunk": plan["shrunk"], "keep": plan["keep"],
           "stubs": plan["stubs"], "released": plan["released"], "summary": plan["summary"]})
}

#[test]
fn the_rebuild_plans_at_the_rows_rate() {
    if !shipped() {
        return;
    }
    let with_row = rebuilt(Some(1.0), 4.0);
    let tap_one = rebuilt(None, 1.0);
    let tap_four = rebuilt(None, 4.0);
    assert_ne!(
        tap_one, tap_four,
        "the rate moves the plan (else this pin proves nothing): {tap_one}"
    );
    assert_eq!(
        with_row, tap_one,
        "with a row the rebuild counts at the row's rate (1.0), not the tap's (4.0)"
    );
}
