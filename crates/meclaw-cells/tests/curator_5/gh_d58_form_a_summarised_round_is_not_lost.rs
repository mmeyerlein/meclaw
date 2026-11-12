//! GH #1463 (D58 form, R-58-8) -- a summarised round is not lost.
//!
//! WHY this file exists: a rebuild takes old rounds out of the window on
//! purpose and leaves their summary. Those answers and words are gone from
//! the next window and are NOT lost: the plan's cover says a summary holds
//! them. `window_lost` counts only what lies in the raw part of the plan --
//! otherwise every rebuild would trip the watch and the number would say
//! nothing.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn what_the_summary_covers_is_no_loss() {
    if !shipped() {
        return;
    }
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(2)),
    ]);
    for i in 1..=6 {
        t.turn(
            &format!("t{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");
    t.h.fire(&order);
    assert!(
        !t.h.summ.is_empty(),
        "check of the setup: the rebuild asks for a summary: {:?}",
        t.h.stderr
    );
    t.h.answer("- The person told items 1 to 4.", "stop");
    let plan = t.h.rebuild_in(ROUND_E);
    assert!(
        plan["cover"].as_i64().unwrap_or(0) > 0,
        "check of the setup: the plan covers old rounds: {plan} {:?}",
        t.h.stderr
    );
    let c7 = t.turn("t7", "Go on.", "Yes.");
    assert!(
        !c7.messages().contains(&said("Noted item 1.")),
        "check of the setup: the covered answer left the window: {:?}",
        c7.messages()
    );
    t.turn("t8", "And?", "Done.");
    let w = t.window_figure();
    assert_eq!(w["n"], json!(8), "{w}");
    assert_eq!(
        w["window_lost"],
        json!(0),
        "covered by the summary, not lost: {w}"
    );
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "a rebuild is no prefix break: {w}"
    );
}
