//! GH #1471 (D58 roundpin, contract point 1) -- a tool round is cut by the
//! plan of its first call.
//!
//! WHY this file exists: every call of a turn read the plan of the window
//! anew, so the promise "a tool round never changes its prefix" held only
//! as long as no rebuild wrote a plan while the round ran -- the rebuild had
//! to wait, and the waiting had two holes (GH #1463, fix round 2). The rule
//! now: the first call of a tool round keeps the plan it was cut by, and
//! every further call of the same round is cut by that plan, not by the
//! newest one. A rebuild writes its plan whenever it is done.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! rebuild is reserved before the round and its summary comes home in the
//! middle of it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_roundpin_a_round_is_cut_by_the_plan_of_its_first_call() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    // The rebuild is reserved before the tool round: the summarizer is asked.
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

    let tid = |j: u32| format!("call_tool_rpfirst_{j:04}");
    let mut c = t.call("t7", 0, json!([user("Read the files, please.")]));
    let began = cut_epoch(&t.h, &c);
    assert!(
        !began.is_empty(),
        "check of the setup: the row of the call names the plan it was cut by"
    );
    t.answer(&c, "tool_calls", json!([tool_call(&tid(1), "read_file")]));

    // The summary comes home in the middle of the tool round: the plan of
    // the rebuild is written at once.
    t.h.answer("- The person told items 1 to 4.", "stop");
    assert!(
        t.cover() > 0,
        "the plan of the rebuild is written while the tool round runs: {:?}",
        t.h.stderr
    );
    assert_ne!(
        standing_epoch(&t.h, ROUND_E),
        began,
        "check of the setup: a newer plan stands than the one the tool round began with"
    );

    // Three more calls of the same tool round: each is cut by the plan of
    // the first one.
    for j in 1..=3 {
        c = t.call(
            "t7",
            j,
            json!([
                tool_call(&tid(j), "read_file"),
                tool_result(&tid(j), &format!("file {j}: abc"))
            ]),
        );
        assert_eq!(
            cut_epoch(&t.h, &c),
            began,
            "call {j} of the tool round is cut by the plan of its first call: {:?}",
            t.h.stderr
        );
        assert!(
            c.messages().contains(&said("Noted item 1.")),
            "call {j} keeps the window the tool round began with: {:?}",
            c.messages()
        );
        if j < 3 {
            t.answer(
                &c,
                "tool_calls",
                json!([tool_call(&tid(j + 1), "read_file")]),
            );
        }
    }
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "every call of the tool round begins with the whole window before it"
    );
    t.answer(&c, "stop", json!([said("All read.")]));
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
    assert_eq!(w["window_lost"], json!(0), "{w}");
}
