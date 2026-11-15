//! GH #1526 (d58-3 grenzen, N5) -- one fallback rate per cell, the core's.
//!
//! WHY this file exists: without a rate of its target a cell turns tokens
//! into characters at a fallback. The policy had two: the content-budget
//! block's `CHARS_PER_TOKEN` (3, the core's estimate in
//! `crates/meclaw-cells/src/content_budget.rs`) and the param
//! `chars_per_token_fallback` (4), which the fallback row of a tap without a
//! package wrote into the state as the rate of the next rebuild. Two numbers
//! for one unknown is a second truth (contract point 1): the param goes, the
//! fallback row takes the block's 3, and every cell of the curator that
//! measures a budget carries the block that takes a rate (`content-budget
//! v2`) and no number of its own.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// The cells of the curator that size a budget in characters, and the one
/// of the objects hive.
const RATE_CELLS: [&str; 7] = [
    "templates/curator/policy/config.json",
    "templates/curator/intake/config.json",
    "templates/curator/history/config.json",
    "templates/curator/handover/config.json",
    "templates/curator/push/config.json",
    "templates/curator/reader/config.json",
    "templates/objects/brief/config.json",
];

#[test]
fn a_tap_without_a_package_leaves_the_cores_rate() {
    if !shipped() {
        return;
    }
    // No package, no window known: the fallback row stands in (GH #1225) and
    // its rate is the next rebuild's estimate -- the core's 3.
    let mut h = Hive::new();
    let call = h.curate("s1", "t1", 0, json!([user("hello")]), mode("Be brief."));
    h.tap(&call, "stop", json!({}), json!([said("hi")]));
    assert_eq!(
        h.state("chars_per_token"),
        "3.0",
        "the fallback row's rate is the core's estimate: {:?}",
        h.stderr
    );
}

#[test]
fn the_policy_names_no_rate_of_its_own() {
    if !shipped() {
        return;
    }
    let params = &cell_config("policy")["params"];
    assert!(
        params.get("chars_per_token_fallback").is_none(),
        "no second fallback rate beside the block's"
    );
    let (rate, _) = policy_scope(
        json!({}),
        "FALLBACK_LIMITS.get('chars_per_token')",
        json!(null),
    );
    assert_eq!(
        rate.as_f64(),
        Some(3.0),
        "the fallback row counts at the block's 3: {rate}"
    );
}

#[test]
fn every_rate_cell_carries_the_block_that_takes_a_rate() {
    if !shipped() {
        return;
    }
    for cfg in RATE_CELLS {
        let script = read_json(&repo(cfg))["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{cfg}: script_inline"))
            .to_string();
        assert_eq!(
            script.matches("# >>> content-budget v2").count(),
            1,
            "{cfg}: one copy of the block that takes the target's rate"
        );
        assert!(
            !script.contains("# >>> content-budget v1"),
            "{cfg}: no copy of the block without a rate"
        );
        let own: Vec<&str> = script
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                (t.starts_with("CHARS_PER_TOKEN") || t.contains("_PER_TOKEN ="))
                    && t.contains('=')
                    && !t.contains("==")
            })
            .collect();
        assert_eq!(
            own,
            vec!["CHARS_PER_TOKEN = 3"],
            "{cfg}: the block's constant is the one rate number"
        );
    }
}
