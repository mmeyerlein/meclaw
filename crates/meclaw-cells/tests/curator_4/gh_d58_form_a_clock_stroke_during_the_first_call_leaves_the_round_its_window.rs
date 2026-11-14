//! GH #1463 (D58 form, contract point 5) -- a clock stroke during the first
//! call of a round leaves the round its window.
//!
//! WHY this file exists: a round begins the moment its first call leaves,
//! not with the first answer that asks for a tool. The clock struck while
//! that first call was still with the model; the rebuild wrote its plan,
//! the answer then asked for a tool, and the second call of the SAME round
//! read the new plan -- the window changed in its middle. The rule: the
//! second call begins with the whole window of the first, whatever a
//! rebuild wrote while the first was out (GH #1471: the round is cut by
//! what its first call was cut by), and the rebuild is not lost. It holds
//! for every role -- three presets.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn rebuilt(t: &Talk) -> bool {
    t.h.rebuild_in(ROUND_E).get("as_of_ms").is_some()
}

fn a_stroke_during_the_first_call(preset: &str) {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(curator_profiles::profile(preset));
    t.turn(
        "t1",
        "My sister Anna moved to Hamburg.",
        "Hamburg is lovely.",
    );
    assert!(!rebuilt(&t), "check of the setup: no rebuild so far");
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");

    // The first call of the round is with the model when the clock strikes.
    let id = "call_tool_formclk_0001";
    let c0 = t.call("t2", 0, json!([user("Read the file, please.")]));
    t.h.fire(&order);
    if !t.h.summ.is_empty() {
        t.h.answer("What was said before, in short.", "stop");
    }
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    // Roundpin review M-6: the rebuild wrote its plan while the first call
    // was out -- without it the second call would keep its window for
    // nothing.
    assert!(
        rebuilt(&t),
        "{preset}: check of the setup: a plan was written before the second call: {:?}",
        t.h.stderr
    );
    let c1 = t.call(
        "t2",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "{preset}: the second call of the round begins with the whole window before it"
    );

    // The rebuild that was due is not lost, without another stroke of the
    // clock.
    t.answer(&c1, "stop", json!([said("All read.")]));
    if !t.h.summ.is_empty() {
        t.h.answer("What was said before, in short.", "stop");
    }
    assert!(
        rebuilt(&t),
        "{preset}: the rebuild that was due is applied: {:?}",
        t.h.stderr
    );
}

#[test]
fn gh_d58_form_a_clock_stroke_during_the_first_call_leaves_the_round_its_window() {
    a_stroke_during_the_first_call("talky");
}

#[test]
fn the_consultation_preset_keeps_its_window_too() {
    a_stroke_during_the_first_call("consult");
}

#[test]
fn the_coding_preset_keeps_its_window_too() {
    a_stroke_during_the_first_call("coding");
}
