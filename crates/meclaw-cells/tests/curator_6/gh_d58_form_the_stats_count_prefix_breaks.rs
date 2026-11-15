//! GH #1463 (D58 form) -- the stats count prefix breaks.
//!
//! WHY this file exists: a window that stops beginning with the one before
//! costs a cache write on every call and nobody sees it -- the measured run
//! paid 121 % more per call before anyone looked. The curator counts it
//! itself: each call compares the window it sends with the `sent` list of
//! the call before (within a round the whole of it, across the round
//! boundary up to that call's running round; not after a rebuild), the
//! `curate` mark carries the verdict, and the figure `window` of the `stats`
//! door sums it as `prefix_breaks`. Beside it `thinking_dropped`: the model
//! cell names the replayed thinking blocks a provider dropped in the body of
//! its answer (`meta.thinking_dropped`, a list, only when there is one), the
//! tap counts them into the same mark. Both should read 0.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const RECALL_ID: &str = "call_recall_0000000000formpb";
const SAID: &str = "My sister Anna moved to Hamburg.";
const LATER: &str = "We sold the old car last spring.";

fn talk(snapshot: &str) -> Talk {
    Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!(snapshot)),
    ])
}

/// Five turns: a recall block whose quote stands outside the wall when it is
/// decided (`t3`), and the person says the quoted words at `t4`. `after`
/// runs behind every answer.
fn five_turns(t: &mut Talk, after: fn(&Talk)) {
    t.turn("t1", SAID, "Hamburg is lovely.");
    after(t);
    let c = t.call(
        "t2",
        0,
        json!([
            user("Tell me about Anna."),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{LATER}\""))
        ]),
    );
    t.answer(&c, "stop", json!([said("She likes it there.")]));
    after(t);
    for (turn, words, reply) in [
        ("t3", "And?", "I do not know more."),
        ("t4", LATER, "A good price, I hope."),
        ("t5", "It was.", "Good."),
    ] {
        t.turn(turn, words, reply);
        after(t);
    }
}

#[test]
fn a_healthy_conversation_counts_none() {
    if !shipped() {
        return;
    }
    let mut t = talk("1");
    five_turns(&mut t, |_| {});
    assert_eq!(t.breaks(), Vec::<String>::new(), "check of the setup");
    let w = t.window_figure();
    assert_eq!(w["n"], json!(5), "{w}");
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
    assert_eq!(w["thinking_dropped"], json!(0), "{w}");
}

/// Pin: the figure is the number of breaks the windows themselves show --
/// counted here from the snapshot rows, by the test's own comparison.
#[test]
fn the_figure_counts_the_breaks_the_windows_show() {
    if !shipped() {
        return;
    }
    let mut t = talk("1");
    five_turns(&mut t, Talk::forget_decisions);
    let seen = t.breaks();
    assert!(
        !seen.is_empty(),
        "check of the setup: a window that forgets its decisions breaks its prefix"
    );
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(seen.len()), "{w} {seen:?}");
}

/// Pin: `meta.thinking_dropped` of the model cell's answer reaches the
/// figure, counted per call and summed over the calls. The header carries
/// nothing of it -- no contract of a model cell names such a key.
#[test]
fn thinking_dropped_is_passed_through() {
    if !shipped() {
        return;
    }
    let mut t = talk("1");
    for (turn, dropped) in [("t1", 2), ("t2", 0), ("t3", 3)] {
        let c = t.call(turn, 0, json!([user("Go on.")]));
        let mut body = json!({"messages": [said("Yes.")],
                              "meta": {"provider": "anthropic", "model": "test-model"}});
        if dropped > 0 {
            let drops: Vec<Value> = (0..dropped)
                .map(|i| {
                    json!({"path": format!("messages[1].content[{i}]"),
                           "reason": "prefix_binding_mismatch"})
                })
                .collect();
            body["meta"]["thinking_dropped"] = json!(drops);
        }
        t.h.tap_body(&c, "stop", warm_usage(), body);
    }
    let w = t.window_figure();
    assert_eq!(w["thinking_dropped"], json!(5), "{w}");
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}

/// Lock: without the snapshot of the window there is nothing to compare, and
/// the figure says nothing rather than a 0 nobody measured.
#[test]
fn off_the_figure_names_no_watch() {
    if !shipped() {
        return;
    }
    let mut t = talk("0");
    t.turn("t1", SAID, "Hamburg is lovely.");
    t.turn("t2", "And?", "Nothing more.");
    let w = t.window_figure();
    assert_eq!(w["n"], json!(2), "{w}");
    for k in ["prefix_breaks", "thinking_dropped", "window_lost"] {
        assert_eq!(w.get(k), None::<&Value>, "{k}: {w}");
    }
}
