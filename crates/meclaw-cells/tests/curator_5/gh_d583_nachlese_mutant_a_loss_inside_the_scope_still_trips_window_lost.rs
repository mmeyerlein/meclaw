//! d58-3 nachlese (GH #1539, B1 break 2) -- sabotage mutant: an answer lost
//! INSIDE the scope of a way still trips `window_lost`.
//!
//! WHY this file exists: the watch counts nothing that left a way's short
//! scope -- the curator marks such a slide itself as a rebuild of the scope
//! (pin `gh_d583_nachlese_a_scope_that_slides_is_a_rebuild_not_a_break`).
//! That excuse must not swallow a real loss: an answer the scope still
//! holds, sent by the window before, that the next call does not send, is a
//! loss whatever else the call slid. The curator is broken where a loss
//! would come from: the plan lists an answer the scope still holds as left
//! out. The call after it slides the scope and loses the answer at once; the
//! figure `window_lost` of the hive's own `stats` door rises above its 0, the
//! healthy run beside it stays at 0.
//!
//! The setup (nachlese-h, GH #1539): the lost answer must be one the window
//! before SENT -- the answer of the round before is not (it is the answer to
//! that very call), so with `keep_recent` 2 no answer can be lost inside the
//! scope. With `keep_recent` 3 the call of t4 slides t1 out and still holds
//! the answer of t2, which the call of t3 sent.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const ANSWER: &str = "Beans climb.";
const SLID: &str = "Tomatoes need sun.";

fn run(sabotage: bool) -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(json!({"system": "full", "keep_recent": 3}));
    t.turn("t1", "We planted tomatoes.", SLID);
    t.turn("t2", "And beans.", ANSWER);
    t.turn("t3", "What did you say about beans?", "They climb.");
    if sabotage {
        t.drop_from_the_window(&said(ANSWER));
    }
    t.turn("t4", "And the tomatoes?", "Sun.");
    t
}

#[test]
fn gh_d583_nachlese_mutant_a_loss_inside_the_scope_still_trips_window_lost() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut healthy = run(false);
    let c3 = healthy.sent[2].call.messages();
    assert!(
        c3.contains(&said(ANSWER)) && c3.contains(&said(SLID)),
        "check of the setup: the window of t3 sent the answers of t1 and t2: {c3:?}"
    );
    let c4 = healthy.sent[3].call.messages();
    assert!(
        c4.contains(&said(ANSWER)) && !c4.contains(&said(SLID)),
        "check of the setup: the scope of t4 holds the answer of t2, not the one of t1: {c4:?}"
    );
    let w = healthy.window_figure();
    assert_eq!(w["n"], json!(4), "{w}");
    assert_eq!(
        w["window_lost"],
        json!(0),
        "the slide of t1 is no loss: {w}"
    );

    let mut broken = run(true);
    let c4 = broken.sent[3].call.messages();
    assert!(
        !c4.contains(&said(ANSWER)) && !c4.contains(&said(SLID)),
        "check of the sabotage: the window of t4 slid t1 and is sent without the answer of t2: {c4:?}"
    );
    let w = broken.window_figure();
    assert_eq!(
        w["window_lost"],
        json!(1),
        "the answer inside the scope is lost and counted, the slid round is not: {w}"
    );
}
