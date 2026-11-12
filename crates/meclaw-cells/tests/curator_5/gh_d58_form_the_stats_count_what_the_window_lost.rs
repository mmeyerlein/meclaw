//! GH #1463 (D58 form, R-58-8) -- the stats count what the window lost.
//!
//! WHY this file exists: a window may stay byte-stable and still be wrong --
//! when something the model must keep in view is simply gone. The curator
//! counts that too: an element of the kinds words of the person, answer or
//! failed tool result that stood in the window sent before, lies in the raw
//! part of the plan (no summary holds it) and is no part of the next window,
//! is one `window_lost`. The rule is one per block kind, for every role. A
//! healthy tool result or a recall block the call decided to leave out is no
//! loss. The figure `window` of the `stats` door sums it; it should read 0.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const FAILED: &str = "{\"error\": \"exit status 1\"}";

fn talk() -> Talk {
    Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
}

/// Three turns, the second a tool round with a healthy and a failed result.
fn three_turns(t: &mut Talk) {
    t.turn("t1", "We planted tomatoes.", "Lovely.");
    let c = t.call("t2", 0, json!([user("Run the tests.")]));
    t.answer(&c, "tool_calls", json!([tool_call("call_a1", "run")]));
    let c = t.call(
        "t2",
        1,
        json!([
            tool_call("call_a1", "run"),
            tool_result("call_a1", "12 files")
        ]),
    );
    t.answer(&c, "tool_calls", json!([tool_call("call_a2", "run")]));
    let c = t.call(
        "t2",
        2,
        json!([tool_call("call_a2", "run"), tool_result("call_a2", FAILED)]),
    );
    t.answer(&c, "stop", json!([said("One test fails.")]));
    t.turn("t3", "Which one?", "The second.");
}

fn lost_after(drop: &[Value]) -> Value {
    let mut t = talk();
    three_turns(&mut t);
    for el in drop {
        t.drop_from_the_window(el);
    }
    let c4 = t.turn("t4", "Go on.", "Yes.");
    for el in drop {
        assert!(
            !c4.messages().contains(el),
            "check of the setup: the window is sent without {el}"
        );
    }
    t.turn("t5", "And then?", "Done.");
    t.window_figure()
}

#[test]
fn a_whole_window_loses_nothing() {
    if !shipped() {
        return;
    }
    let w = lost_after(&[]);
    assert_eq!(w["n"], json!(7), "{w}");
    assert_eq!(w["window_lost"], json!(0), "{w}");
}

/// Pin: each of the three kinds counts, once -- at the call whose window
/// misses it, not again at every call after.
#[test]
fn an_answer_words_and_a_failed_result_count_once_each() {
    if !shipped() {
        return;
    }
    let w = lost_after(&[said("Lovely.")]);
    assert_eq!(w["window_lost"], json!(1), "an answer: {w}");
    let w = lost_after(&[user("We planted tomatoes.")]);
    assert_eq!(w["window_lost"], json!(1), "words of the person: {w}");
    let w = lost_after(&[tool_result("call_a2", FAILED)]);
    assert_eq!(w["window_lost"], json!(1), "a failed result: {w}");
    let w = lost_after(&[
        said("Lovely."),
        user("We planted tomatoes."),
        tool_result("call_a2", FAILED),
    ]);
    assert_eq!(w["window_lost"], json!(3), "all three: {w}");
}

/// Lock: the rule names three kinds. A healthy tool result that leaves the
/// window is no loss of this watch.
#[test]
fn a_healthy_tool_result_is_no_loss() {
    if !shipped() {
        return;
    }
    let w = lost_after(&[tool_result("call_a1", "12 files")]);
    assert_eq!(w["window_lost"], json!(0), "{w}");
}
