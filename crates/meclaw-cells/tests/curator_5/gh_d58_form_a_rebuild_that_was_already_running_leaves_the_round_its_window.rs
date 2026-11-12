//! GH #1463 (D58 form, contract point 5) -- a rebuild that was already
//! running leaves the round its window.
//!
//! WHY this file exists: a rebuild that needs a summary takes seconds. One
//! reserved BEFORE a round keeps running while the person speaks on; when
//! its summary came home in the middle of the next tool round, the plan was
//! written at once and the next call of the SAME round read it -- the window
//! changed in its middle, under the signed thinking blocks of the round. The
//! rule holds for a rebuild whenever it started: its plan reaches no further
//! call of a round that runs (GH #1471: the round is cut by what its first
//! call was cut by), it shows in the next round, and the summary is not
//! asked for twice.
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
fn gh_d58_form_a_rebuild_that_was_already_running_leaves_the_round_its_window() {
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
    // The rebuild is reserved before the round: the summarizer is asked.
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

    // The next round begins and asks for a tool while the summarizer works.
    let id = "call_tool_formrun_0001";
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    // The summary comes home in the middle of the round.
    t.h.answer("- The person told items 1 to 4.", "stop");
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "the second call of the round begins with the whole window before it"
    );
    assert!(
        c1.messages().contains(&said("Noted item 1.")),
        "the round keeps the window it began with: {:?}",
        c1.messages()
    );

    // The round ends: the plan of the rebuild stands, from the summary that
    // came home -- the summarizer is not asked again.
    t.answer(&c1, "stop", json!([said("All read.")]));
    assert!(cover(&t) > 0, "the rebuild is applied: {:?}", t.h.stderr);
    assert!(
        t.h.summ.is_empty(),
        "no second request to the summarizer: {:?}",
        t.h.summ
    );
    let c8 = t.turn("t8", "Go on.", "Yes.");
    assert!(
        !c8.messages().contains(&said("Noted item 1.")),
        "the next round reads the plan of the rebuild: {:?}",
        c8.messages()
    );
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
    assert_eq!(w["window_lost"], json!(0), "{w}");
}
