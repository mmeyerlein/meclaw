//! GH #1494 (D58 close, roundpin review M-3) -- after the way through
//! `./handover` the row of the turn names the new handover leaf.
//!
//! WHY this file exists: the first call of a turn lays the row its turn is
//! cut by (`pending:turn_cut:...`) in `win-hashes` -- with the handover leaf
//! that stands then, when the window served its session last. If another
//! session takes over before `win` reads (`handover_for` moves), `win` sends
//! the call through `./handover` after all; the leaf is laid anew, and the
//! pass after it found the row its own and left it as it was. The turn kept
//! the leaf of before and did not show its own handover. The pass after
//! `./handover` now sets the leaves of a row that is still the call's, as it
//! does after the summary leaf was laid again (`relay`).
//!
//! `./handover` lays a leaf for a NEW session only (no wall row outside the
//! turn now running), so the case needs a turn of a new session that comes
//! again after its first call: here the first call ended in an error, which
//! took the row down, and the turn's next first call met the takeover.
//! Mutant: the pass after `./handover` leaves the row as it is -> red.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn curate_msg(session: &str, turn: &str, words: &str) -> (Value, Value, Value) {
    (
        json!({"session_id": session, "turn_id": turn, "iter": "0",
               "channel": "test", "audience_set": ROUND_E}),
        json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(words)], "system": mode("Be brief.")}),
    )
}

fn handover_slot(h: &Hive) -> Value {
    h.rows("SELECT hash FROM slots WHERE path = 'history.handover'")
        .first()
        .map(|r| r[0].clone())
        .unwrap_or(Value::Null)
}

fn topic(h: &mut Hive, back_s: i64, name: &str) {
    h.mark(
        backdated(back_s),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": name})),
    );
}

/// The wall rows of `session` outside its turn `turn`: none means a new
/// session to `./handover`.
fn wall_rows_of(h: &Hive, session: &str, turn: &str) -> usize {
    h.rows(&format!(
        "SELECT seq FROM wall WHERE session_id = '{session}' AND turn_id != '{turn}'"
    ))
    .len()
}

#[test]
fn gh_d58_roundpin_after_the_handover_the_row_names_the_new_leaf() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting_composite("talky");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Good.",
        json!({}),
    );
    // The open topic: what the leaf of a new session carries -- without one
    // (and without a note) `./handover` lays no leaf at all.
    topic(&mut h, 5, "the garden");
    turn(&mut h, "s1", "t2", "And beans.", "Beans climb.", json!({}));
    // The first call of `s2` goes through `./handover` (the window serves
    // `s2` now), lays the row of its turn after it and comes back as an
    // error, which ends the turn and takes the row down.
    let first = h.curate(
        "s2",
        "u1",
        0,
        json!([user("Hello again.")]),
        mode("Be brief."),
    );
    let before = handover_slot(&h);
    assert!(
        before.is_string(),
        "check of the setup: the second session started with a handover leaf: {:?}",
        h.stderr
    );
    h.tap(&first, "error", json!({"error_code": "timeout"}), json!([]));
    assert_eq!(
        h.state("handover_for"),
        "s2",
        "check of the setup: the window serves s2"
    );
    assert!(
        turn_cut_row(&h, ROUND_E, "s2", "u1").is_null(),
        "check of the setup: the error ended the turn and its row: {:?}",
        h.stderr
    );
    assert_eq!(
        wall_rows_of(&h, "s2", "u1"),
        0,
        "check of the setup: s2 keeps no wall row outside its turn u1 -- a new session"
    );
    // The topic moves on: the next leaf of `s2` is a new one.
    topic(&mut h, 1, "the beans");
    // The turn comes again. Its first call finds the window serving `s2` and
    // lays the row with the leaf that stands; right before `win` reads, a
    // call of `s1` comes in and runs to rest -- the window serves `s1` now,
    // `win` sends the call of `s2` through `./handover` after all, and as
    // `s2` is still new, the leaf is laid anew.
    let (ctx, hop, body) = curate_msg("s1", "t3", "The beans need poles.");
    h.lane_at_phase("win", "in_curate", ctx, hop, body);
    let (ctx, hop, body) = curate_msg("s2", "u1", "Hello again.");
    h.out.clear();
    h.lane("in_curate", ctx, hop, body);
    let of = |s: &str| {
        h.routed("brain")
            .into_iter()
            .filter(|m| m.context.get("session_id") == Some(&json!(s)))
            .count()
    };
    assert_eq!(
        of("s1"),
        1,
        "check of the setup: the call of s1 came in between: {:?}",
        h.stderr
    );
    assert_eq!(of("s2"), 1, "{:?} {:?}", h.out, h.stderr);
    let now = handover_slot(&h);
    assert_ne!(
        now, before,
        "check of the setup: `./handover` laid a new leaf for s2"
    );
    let row = turn_cut_row(&h, ROUND_E, "s2", "u1");
    assert_eq!(
        row["leaves"]["history.handover"][0], now,
        "the turn keeps the leaf it went through `./handover` for: {row}"
    );
}
