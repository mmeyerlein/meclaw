//! GH #1463 (D58 form) -- sabotage mutant: the cut per call of before.
//!
//! The curator is broken where the form lives: after every answer the plan
//! forgets what it decided (no block says `decided`, nothing is `left_out`),
//! so every call checks every recall block again -- the behaviour of before
//! the form. A block that was sent leaves the window in its middle as soon as
//! the person says the quoted words. The windows are the shipped policy's
//! own, the figure is read off its own `curate` marks through the hive's
//! `stats` door: the watch number rises above its 0. The healthy run beside
//! it, the same turns, stays at 0 -- the alarm is the invariant's.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const RECALL_ID: &str = "call_recall_0000000000formma";
const SAID: &str = "My sister Anna moved to Hamburg.";
const LATER: &str = "We sold the old car last spring.";

fn run(sabotage: bool) -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    let after = |t: &Talk| {
        if sabotage {
            t.forget_decisions();
        }
    };
    t.turn("t1", SAID, "Hamburg is lovely.");
    after(&t);
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
    after(&t);
    for (turn, words, reply) in [
        ("t3", "And?", "I do not know more."),
        ("t4", LATER, "A good price, I hope."),
        ("t5", "It was.", "Good."),
    ] {
        t.turn(turn, words, reply);
        after(&t);
    }
    t
}

#[test]
fn gh_d58_form_mutant_a_cut_per_call_trips_the_watch() {
    if !shipped() {
        return;
    }
    let mut healthy = run(false);
    let w = healthy.window_figure();
    assert_eq!(w["n"], json!(5), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "the form only appends: {w} {:?}",
        healthy.breaks()
    );

    let mut broken = run(true);
    let last = broken.sent.last().expect("a call").call.messages();
    assert!(
        !last.iter().any(|m| m["id"] == json!(RECALL_ID)),
        "check of the sabotage: the cut per call took the block that was sent: {last:?}"
    );
    let w = broken.window_figure();
    assert!(
        w["prefix_breaks"].as_u64().unwrap_or(0) > 0,
        "the watch number rises: {w}"
    );
}
