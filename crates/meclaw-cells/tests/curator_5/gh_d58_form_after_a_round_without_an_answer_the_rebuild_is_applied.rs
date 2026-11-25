//! GH #1463 (D58 form, contract point 5) -- after a round without an answer
//! the rebuild is applied.
//!
//! WHY this file exists: a call whose answer never comes (not even an
//! error) must not hold a rebuild for good. The clock strikes while that
//! call is out, the person speaks on, and when the next round has ended
//! the plan of the rebuild stands (GH #1471: a rebuild waits for no round;
//! a round is cut by what its first call was cut by).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn cover(t: &Talk) -> i64 {
    t.h.rebuild_in(ROUND_E)["cover"].as_i64().unwrap_or(0)
}

#[test]
fn gh_d58_form_after_a_round_without_an_answer_the_rebuild_is_applied() {
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
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");

    // A call leaves and no answer ever comes home. The clock strikes.
    t.call("t7", 0, json!([user("Are you there?")]));
    t.h.fire(&order);

    // The person speaks on: the next round begins and ends, and the plan of
    // the rebuild stands -- without another stroke of the clock.
    let next = t.call("t8", 0, json!([user("Hello?")]));
    t.answer(&next, "stop", json!([said("Here I am.")]));
    if !t.h.summ.is_empty() {
        t.h.answer("- The person told items 1 to 4.", "stop");
    }
    assert!(cover(&t) > 0, "the rebuild is applied: {:?}", t.h.stderr);
}
