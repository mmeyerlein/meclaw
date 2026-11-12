//! GH #1464 (D58 coldgc, contract point 3) -- a cold collection books no cost.
//!
//! WHY this file exists: the watcher reads three figures off the marks of
//! the curator -- how often a window is rebuilt (`rebuild_rate`), what the
//! rebuilds cost (`rebuild_cost`) and how much of them is collection
//! (`gc_share`). A cold collection calls no model, so it counts as one
//! rebuild of the trigger `cold` at 0 cents: the mark says 0, and the hive's
//! own `stats` door answers the trigger and adds nothing to the cost.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_cold_a_cold_collection_books_no_cost() {
    if !shipped() {
        return;
    }
    let (mut t, _, _) = garbage_talk("talky", &cold_knobs(Some("1")));
    let (before, _) = rebuild_and_gc_figures(&mut t.h);
    cache_expires(&mut t);

    let cold = cold_marks(&t.h);
    assert_eq!(cold.len(), 1, "one mark of the trigger `cold`: {cold:?}");
    assert_eq!(
        cold[0]["cost_cents"],
        json!(0),
        "no model was called: the mark books 0 cents: {cold:?}"
    );
    assert!(
        !cold[0]["call"].as_str().unwrap_or("").is_empty(),
        "the mark names the call whose cache ran out: {cold:?}"
    );

    let (rebuild, gc) = rebuild_and_gc_figures(&mut t.h);
    assert_eq!(
        rebuild["by_mark"]["cold"],
        json!(1),
        "the `stats` door counts the trigger: {rebuild}"
    );
    assert_eq!(
        rebuild["cost_cents_sum"], before["cost_cents_sum"],
        "the collection adds nothing to the cost of the rebuilds: {rebuild}"
    );
    assert_eq!(gc["n"], json!(1), "one `gc` mark is counted: {gc}");
    assert_eq!(gc["classes"]["shrunk_tools"], json!(1), "{gc}");
    assert_eq!(gc["active_pct"], json!(100), "{gc}");
}
