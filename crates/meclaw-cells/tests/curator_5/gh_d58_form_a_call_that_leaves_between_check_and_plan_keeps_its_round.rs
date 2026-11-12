//! GH #1463 (D58 form, contract point 5), moved to GH #1471 -- a call that
//! leaves between the check and the plan keeps its round.
//!
//! WHY this file exists: "is a round open?" and the write of a rebuild's
//! plan were two steps at the store, and a call named itself as the open
//! round only when it left, several store rounds after it read its plan.
//! A summary that came home in that span found no open round and wrote
//! its plan; the call left with the window of before, its answer asked for
//! a tool, and the second call of the SAME round read the new plan. Since
//! GH #1471 no check stands between a rebuild and its plan: the tool round
//! keeps the plan its first call was cut by, whenever a newer one is
//! written.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! interleaving is made by the harness: the summary is delivered right
//! before the store answers the last read of the call that is being built.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn cover(t: &Talk) -> i64 {
    t.h.rebuild_in(ROUND_E)["cover"].as_i64().unwrap_or(0)
}

#[test]
fn gh_d58_form_a_call_that_leaves_between_check_and_plan_keeps_its_round() {
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

    // The summary comes home while the first call of the next round is
    // being built: after the call read its plan, before it left.
    t.h.summary_at_phase("win", "- The person told items 1 to 4.");
    let id = "call_tool_formgap_0001";
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    assert!(
        !t.h.waits_at_phase(),
        "check of the setup: the summary came home while the call was built"
    );
    assert!(
        c0.messages().contains(&said("Noted item 1.")),
        "check of the setup: the call left with the window of before: {:?}",
        c0.messages()
    );
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "the second call of the round begins with the whole window before it: {:?}",
        t.h.stderr
    );
    assert!(
        c1.messages().contains(&said("Noted item 1.")),
        "the round keeps the window it began with: {:?}",
        c1.messages()
    );

    // The round ends: the plan of the rebuild stands.
    t.answer(&c1, "stop", json!([said("All read.")]));
    assert!(
        cover(&t) > 0,
        "the plan of the rebuild stands after the round: {:?}",
        t.h.stderr
    );
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}
