//! GH #1463 (D58 form, R-58-8) -- sabotage mutant: an answer dropped from
//! the window.
//!
//! The curator is broken where a loss would come from: the plan of the round
//! lists an answer of the conversation as left out, so the shipped policy
//! sends its next window without it -- the answer stands in the wall, in the
//! raw part, and no summary holds it. The figure is read off the policy's own
//! `curate` marks through the hive's `stats` door: `window_lost` rises above
//! its 0. The healthy run beside it, the same turns, stays at 0.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const ANSWER: &str = "Tomatoes need sun and water.";

fn run(sabotage: bool) -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.turn("t1", "We planted tomatoes.", ANSWER);
    t.turn("t2", "And beans.", "Beans climb.");
    if sabotage {
        t.drop_from_the_window(&said(ANSWER));
    }
    t.turn("t3", "What did you say about tomatoes?", "They need sun.");
    t.turn("t4", "Thanks.", "Gladly.");
    t
}

#[test]
fn gh_d58_form_mutant_a_dropped_answer_trips_window_lost() {
    if !shipped() {
        return;
    }
    let mut healthy = run(false);
    let w = healthy.window_figure();
    assert_eq!(w["n"], json!(4), "{w}");
    assert_eq!(w["window_lost"], json!(0), "nothing is lost: {w}");

    let mut broken = run(true);
    let c3 = broken.sent[2].call.messages();
    assert!(
        !c3.contains(&said(ANSWER)),
        "check of the sabotage: the window of t3 is sent without the answer: {c3:?}"
    );
    let w = broken.window_figure();
    assert!(
        w["window_lost"].as_u64().unwrap_or(0) > 0,
        "the watch number rises: {w}"
    );
}
