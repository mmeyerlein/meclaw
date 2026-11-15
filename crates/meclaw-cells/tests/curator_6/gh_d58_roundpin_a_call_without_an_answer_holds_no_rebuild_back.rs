//! GH #1471 (D58 roundpin, contract point 3) -- a call without an answer
//! holds no rebuild back.
//!
//! WHY this file exists: while a rebuild waited for the end of a tool round
//! (GH #1463), a call whose answer never came home kept the round open, and
//! the rebuild waited until the person spoke again. Now nothing waits: the
//! rebuild starts on the stroke of the clock and writes its plan while the
//! call is out, and the next turn is cut by it.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_roundpin_a_call_without_an_answer_holds_no_rebuild_back() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");

    // A call leaves and no answer ever comes home. The clock strikes.
    let _lost = t.call("t7", 0, json!([user("Are you there?")]));
    t.h.fire(&order);
    assert_eq!(
        t.h.summ.len(),
        1,
        "the rebuild starts although a call is out without an answer: {:?}",
        t.h.stderr
    );
    t.h.answer("- The person told items 1 to 4.", "stop");
    assert!(
        t.cover() > 0,
        "the plan of the rebuild is written without any answer to the call: {:?}",
        t.h.stderr
    );

    // The person speaks on: the next turn is cut by the plan that stands,
    // and nothing was kept waiting for it.
    let c8 = t.turn("t8", "Hello?", "Here I am.");
    assert!(
        !c8.messages().contains(&said("Noted item 1.")),
        "the next turn is cut by the plan of the rebuild: {:?}",
        c8.messages()
    );
    assert_eq!(
        cut_epoch(&t.h, &c8),
        standing_epoch(&t.h, ROUND_E),
        "the row of that call names the plan that stands"
    );
    assert!(
        t.h.summ.is_empty(),
        "no rebuild waited for the next turn: {:?}",
        t.h.summ
    );
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}
