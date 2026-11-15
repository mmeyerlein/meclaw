//! GH #1463 (D58 form, contract point 7) -- a call sent while a rebuild ran
//! is no prefix break.
//!
//! WHY this file exists: `prefix_breaks` counts a call whose window does not
//! begin with the one before it WITHOUT a rebuild between the two. The watch
//! told "a rebuild lay between" by time: the plan's `as_of_ms` against the
//! time of the window before. `as_of_ms` is the moment the rebuild was
//! reserved, not the moment its plan was written -- a call that left while
//! the summarizer worked was younger than the reservation, and the call
//! after the rebuild counted a break. With a summarizer that takes seconds
//! and a person who speaks on, that is the usual case: one false alarm per
//! rebuild on a number whose target is 0. The watch compares no time: each
//! call keeps the state of the plan it was built on, and two calls built on
//! different states had a rebuild between them.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_form_a_call_sent_while_a_rebuild_ran_is_no_prefix_break() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
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
    // The rebuild is reserved: the summarizer is asked and works.
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");
    t.h.fire(&order);
    assert_eq!(
        t.h.summ.len(),
        1,
        "check of the setup: the rebuild asks for a summary: {:?}",
        t.h.stderr
    );
    // A whole turn goes out and ends while the summarizer works: its call is
    // younger than the reservation and built on the plan of before.
    let c7 = t.turn("t7", "Go on.", "Yes.");
    assert!(
        c7.messages().contains(&said("Noted item 1.")),
        "check of the setup: the call left with the window of before: {:?}",
        c7.messages()
    );
    // The summary comes home, no round is open: the plan is written.
    t.h.answer("- The person told items 1 to 4.", "stop");
    let plan = t.h.rebuild_in(ROUND_E);
    assert!(
        plan["cover"].as_i64().unwrap_or(0) > 0,
        "check of the setup: the plan covers old rounds: {plan} {:?}",
        t.h.stderr
    );
    // The first call after the rebuild: its window changed, by the rebuild.
    let c8 = t.turn("t8", "And?", "Done.");
    assert!(
        !c8.messages().contains(&said("Noted item 1.")),
        "check of the setup: the covered answer left the window: {:?}",
        c8.messages()
    );
    let w = t.window_figure();
    assert_eq!(w["n"], json!(8), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "the rebuild lay between the two calls -- no prefix break: {w}"
    );
    assert_eq!(w["window_lost"], json!(0), "{w}");
}
