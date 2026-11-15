//! GH #1463 (D58 form, contract point 5), moved to GH #1471 -- the answer of
//! a younger call leaves the older call its window.
//!
//! WHY this file exists: one place per audience named the ONE call that
//! held the tool round open, the one that left last. With two calls of the
//! same audience out at once, the answer of the younger one closed the
//! round while the older one was still with the model; a rebuild that
//! waited was applied, the older answer asked for a tool, and its next call
//! read the new plan. Since GH #1471 no answer closes anything for another
//! call: each tool round keeps the plan its first call was cut by.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! two calls are two sessions of one audience.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_form_the_answer_of_a_younger_call_leaves_the_older_call_its_window() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");

    // Two calls of the audience are with the model at the same time.
    let older = t.call("t7", 0, json!([user("Read the file, please.")]));
    let younger = t.h.curate(
        "s2",
        "u1",
        0,
        json!([user("Hello from the other session.")]),
        mode("Be brief."),
    );
    t.h.fire(&order);
    if !t.h.summ.is_empty() {
        t.h.answer(SUMMARY, "stop");
    }

    // The younger call is answered first; the older one is still out.
    t.h.tap(&younger, "stop", warm_usage(), json!([said("Hello.")]));
    if !t.h.summ.is_empty() {
        t.h.answer(SUMMARY, "stop");
    }
    assert!(
        t.cover() > 0,
        "check of the setup: a rebuild wrote its plan while the older call was out: {:?}",
        t.h.stderr
    );

    // The older answer asks for a tool: its next call is a call of the same
    // tool round and begins with the window the round began with.
    let id = "call_tool_formtwo_0001";
    t.answer(&older, "tool_calls", json!([tool_call(id, "read_file")]));
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "the next call of the older tool round begins with the whole window before it: {:?}",
        t.h.stderr
    );
    assert!(
        c1.messages().contains(&said("Noted item 1.")),
        "the older tool round keeps the window it began with: {:?}",
        c1.messages()
    );
}

const SUMMARY: &str = "- The person told items 1 to 4.";
