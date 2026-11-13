//! GH #1471 (D58 roundpin, contract point 2) -- a turn whose first call goes
//! a side way lays its row after it.
//!
//! WHY this file exists: the first call of a turn lays the row the turn is
//! cut by in the bundle of its hash reads (`win-hashes`). Three ways hold
//! that back and lay it on a later pass, and none of them had a pin
//! (roundpin review M-5): the first call of a new session goes through
//! `./handover` first, which lays the handover leaf the row names; a call
//! whose round's own summary leaf fell with its plan has the leaf laid
//! again first (`relay`); and a call that held back for one of the two and
//! then found neither due (a read that was stale a step later) lays the row
//! on one more pass (`turn_claim_now`). On every way the row is laid once,
//! names the leaves the call went out with, and the second call of the turn
//! begins with the whole window of the first.
//!
//! Mutants (run in the close fix round, logs in its report): the row laid on
//! the first pass despite `./handover` -> the first test red; despite
//! `relay` -> the second red -- both at "writes the row of its turn once":
//! the leaves of a row laid too early are set again after the leaf changed
//! (inventory 33), so the row alone looks right and only its writes tell;
//! with that later set gone as well, the second is red at "the row names
//! the summary leaf laid again". The pass of `turn_claim_now` gone -> the
//! third red (no row written). A row laid BEFORE a late turn to
//! `./handover` (roundpin review M-3) is held by
//! `gh_d58_roundpin_after_the_handover_the_row_names_the_new_leaf`.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ID: &str = "call_tool_rpsideway_0001";

fn form() -> Hive {
    Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(2)),
    ])
}

/// What a row cuts by: the cover, the leaves and the legend. The tap of a
/// tool call writes the row anew with the plan it built after the answer
/// (`parked_at`, `plan`), the cut stays.
fn cut_of(row: &Value) -> Value {
    json!({"cover": row["cover"], "leaves": row["leaves"], "legend": row["legend"]})
}

fn slot_hash(h: &Hive, path: &str) -> Value {
    h.rows(&format!("SELECT hash FROM slots WHERE path = '{path}'"))
        .first()
        .map(|r| r[0].clone())
        .unwrap_or(Value::Null)
}

/// A tool round of two calls in `session`/`turn`; the first call's row is
/// read right after it. Returns the row and both calls.
fn tool_round(h: &mut Hive, session: &str, turn: &str) -> (Value, Msg, Msg) {
    let ops_before = h.ledger_ops.len();
    let c0 = h.curate(
        session,
        turn,
        0,
        json!([user("Read the file, please.")]),
        mode("Be brief."),
    );
    let row = turn_cut_row(h, ROUND_E, session, turn);
    // Laid once, after the side way -- not laid first and set again when
    // the leaf changed (close fix round: the hold-back and the later set of
    // the leaves, inventory 33, give the same row; only the writes tell
    // them apart).
    let suffix = format!(":{session}:{turn}");
    let writes: Vec<&Value> = h.ledger_ops[ops_before..]
        .iter()
        .map(|(_, op)| op)
        .filter(|op| {
            (op["operation"] == json!("insert") || op["operation"] == json!("update"))
                && [&op["row"]["key"], &op["where"]["key"]].iter().any(|k| {
                    k.as_str()
                        .is_some_and(|k| k.starts_with("pending:turn_cut:") && k.ends_with(&suffix))
                })
        })
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "the first call writes the row of its turn once: {writes:?}"
    );
    let epoch = cut_epoch(h, &c0);
    let w0 = window_of(h, &c0);
    h.tap(
        &c0,
        "tool_calls",
        warm_usage(),
        json!([tool_call(ID, "read_file")]),
    );
    let c1 = h.curate(
        session,
        turn,
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
        mode("Be brief."),
    );
    let w1 = window_of(h, &c1);
    assert_eq!(
        w1.leaves, w0.leaves,
        "the second call keeps the system part of the first"
    );
    assert_eq!(
        w1.messages.get(..w0.messages.len()),
        Some(&w0.messages[..]),
        "the second call begins with the whole window of the first"
    );
    assert_eq!(
        cut_epoch(h, &c1),
        epoch,
        "both calls name the plan of the first"
    );
    assert_eq!(
        cut_of(&turn_cut_row(h, ROUND_E, session, turn)),
        cut_of(&row),
        "the later call is cut by the row of the first"
    );
    (row, c0, c1)
}

/// The first call of a new session goes through `./handover`: the row is
/// laid after it and names the handover leaf the call went out with.
#[test]
fn the_first_call_of_a_new_session_lays_its_row_after_the_handover() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = form().reporting_composite("talky");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    // A topic and a promise of `s1`: what the handover of `s2` carries.
    h.mark(
        backdated(5),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "the garden"})),
    );
    h.lane(
        "in_pin",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"pins": [{"text": "Call Hannah on Sunday.", "source": "commitment"}]}),
    );
    let (row, c0, _) = tool_round(&mut h, "s2", "n1");
    let leaf = c0.body["system"]["history"]["handover"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !leaf.is_empty(),
        "check of the setup: the first call of the new session went through `./handover`: {:?}",
        h.stderr
    );
    let handover = slot_hash(&h, "history.handover");
    assert!(handover.is_string(), "check of the setup: the leaf stands");
    assert_eq!(
        row["leaves"]["history.handover"][0], handover,
        "the row names the handover leaf the call went out with: {row}"
    );
    assert_eq!(
        h.state("handover_for"),
        "s2",
        "the handover served the new session"
    );
}

/// The round's own summary leaf fell with its plan: the first call of the
/// next turn has it laid again (`relay`), then lays its row, which names it.
#[test]
fn a_call_whose_summary_leaf_is_laid_again_lays_its_row_after_it() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    t.strike("- The person told items 1 to 4.");
    assert!(
        t.cover() > 0,
        "check of the setup: a rebuild with a summary: {:?}",
        t.h.stderr
    );
    // The plan of the round falls, and the summary leaf with it; the
    // summary itself stays.
    let rk = round_key(ROUND_E);
    t.h.db
        .execute_batch(&format!(
            "DELETE FROM state WHERE key = 'window_plan:{rk}'; \
             DELETE FROM slots WHERE path = '{}';",
            summary_slot(ROUND_E)
        ))
        .expect("the plan and the leaf fall");
    let (row, _, _) = tool_round(&mut t.h, "s1", "t7");
    let summary = slot_hash(&t.h, &summary_slot(ROUND_E));
    assert!(
        summary.is_string(),
        "check of the setup: the relay laid the summary leaf again: {:?}",
        t.h.stderr
    );
    assert_eq!(
        row["leaves"]["history.summary"][0], summary,
        "the row names the summary leaf laid again: {row}"
    );
}

/// The first call held back for `./handover`, and a step later the handover
/// was no longer due (another call served the session in between): it lays
/// its row on one more pass (`turn_claim_now`).
#[test]
fn a_call_that_held_back_for_nothing_lays_its_row_on_one_more_pass() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = form();
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    // Between the hash reads and the build of the first call of `s2`, the
    // handover serves `s2` already.
    h.sql_at_phase = Some((
        "win".to_string(),
        "INSERT OR REPLACE INTO state (key, value) VALUES ('handover_for', 's2');".to_string(),
    ));
    let (row, c0, _) = tool_round(&mut h, "s2", "n1");
    assert!(
        h.sql_at_phase.is_none(),
        "check of the setup: the handover was served in between"
    );
    assert!(
        c0.body["system"]["history"].get("handover").is_none(),
        "check of the setup: the call did not go through `./handover`: {:?}",
        c0.body["system"]
    );
    assert!(
        row.is_object(),
        "the call lays its row on one more pass: {row} {:?}",
        h.stderr
    );
}
