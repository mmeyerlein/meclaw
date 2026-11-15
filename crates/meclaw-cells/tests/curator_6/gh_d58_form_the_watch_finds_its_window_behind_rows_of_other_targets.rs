//! GH #1494 (D58 close, form review M-1 second part) -- the watch finds the
//! window before it behind the rows of other targets.
//!
//! WHY this file exists: a call judges its window against the newest window
//! sent to the same target. It read the four newest snapshot rows of its
//! round (`WATCH_ROWS`) and looked among them; with more rows of other
//! targets in between (side calls, a second way) it found none and judged
//! nothing -- an element lost from the window went unseen. The watch now
//! reads as many rows as the round keeps (`snapshot_keep`): a window before
//! that the round still holds is found.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ANSWER: &str = "Tomatoes need sun and water.";

/// Five rows of another target behind the newest snapshot row, as the
/// windows of other targets would leave them.
fn rows_of_another_target(t: &Talk) {
    let newest = t.h.rows(
        "SELECT seq, call_id, session_id, turn_id, body, cut, sent, at, audience_set \
         FROM snapshots ORDER BY seq DESC LIMIT 1",
    );
    let r = newest.first().expect("a snapshot row");
    let s = |i: usize| r[i].as_str().unwrap_or_default().to_string();
    let mut cut: Value = sj::from_str(&s(5)).expect("the cut of the row");
    cut["target"] = json!("other/target");
    for k in 1..=5 {
        t.h.db
            .execute(
                "INSERT INTO snapshots (seq, call_id, session_id, turn_id, body, cut, sent, at, \
                 audience_set) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    r[0].as_i64().expect("seq") + k,
                    format!("{}-other-{k}", s(1)),
                    s(2),
                    s(3),
                    s(4),
                    sj::to_string(&cut).expect("cut"),
                    s(6),
                    s(7),
                    s(8)
                ],
            )
            .expect("a row of another target");
    }
}

fn run(sabotage: bool) -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.turn("t1", "We planted tomatoes.", ANSWER);
    t.turn("t2", "And beans.", "Beans climb.");
    rows_of_another_target(&t);
    if sabotage {
        t.drop_from_the_window(&said(ANSWER));
    }
    t.turn("t3", "What did you say about tomatoes?", "They need sun.");
    t
}

#[test]
fn gh_d58_form_the_watch_finds_its_window_behind_rows_of_other_targets() {
    if !shipped() {
        return;
    }
    let mut healthy = run(false);
    let w = healthy.window_figure();
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
        "the window of t2 stands behind five rows of another target and is found: {w}"
    );
}
