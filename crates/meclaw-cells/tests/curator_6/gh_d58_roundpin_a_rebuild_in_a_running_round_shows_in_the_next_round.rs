//! GH #1471 (D58 roundpin, contract point 3) -- a rebuild in a running tool
//! round shows in the next round.
//!
//! WHY this file exists: a rebuild that fell due while a tool round ran had
//! to wait for the end of the round (GH #1463): the trigger was kept, the
//! summary was held, and the answer that ended the round applied both. Now
//! the rebuild does not wait. It starts on the stroke of the clock, writes
//! its plan when its summary is home, and the tool round that runs goes on
//! with the plan it began with; the first call of the next round is the
//! first one the new plan cuts. The end of the round applies nothing.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! clock strikes while the first call of the round is with the model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_roundpin_a_rebuild_in_a_running_round_shows_in_the_next_round() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");

    // The first call of the tool round is with the model when the clock
    // strikes: the rebuild starts.
    let id = "call_tool_rpnext_0001";
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    t.h.fire(&order);
    assert_eq!(
        t.h.summ.len(),
        1,
        "the rebuild starts while the first call of the tool round is out: {:?}",
        t.h.stderr
    );
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    t.h.answer("- The person told items 1 to 4.", "stop");
    assert!(
        t.cover() > 0,
        "the plan of the rebuild is written while the tool round runs: {:?}",
        t.h.stderr
    );
    let written = t.h.rebuild_in(ROUND_E);

    // The tool round goes on with the window it began with.
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "the second call of the tool round begins with the whole window before it: {:?}",
        t.h.stderr
    );
    assert!(
        c1.messages().contains(&said("Noted item 1.")),
        "the tool round keeps the window it began with: {:?}",
        c1.messages()
    );

    // The end of the tool round applies nothing: the plan stands as the
    // rebuild wrote it, and nobody is asked again.
    t.answer(&c1, "stop", json!([said("All read.")]));
    assert!(
        t.h.summ.is_empty(),
        "no request to the summarizer at the end of the tool round: {:?}",
        t.h.summ
    );
    assert_eq!(
        t.h.rebuild_in(ROUND_E),
        written,
        "the plan stands as the rebuild wrote it in the middle of the tool round"
    );

    // The next round is the first one the new plan cuts.
    let c8 = t.turn("t8", "Go on.", "Yes.");
    assert!(
        !c8.messages().contains(&said("Noted item 1.")),
        "the first call of the next round is cut by the plan of the rebuild: {:?}",
        c8.messages()
    );
    assert_eq!(
        cut_epoch(&t.h, &c8),
        standing_epoch(&t.h, ROUND_E),
        "the row of that call names the plan that stands"
    );
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
    assert_eq!(w["window_lost"], json!(0), "{w}");
}
