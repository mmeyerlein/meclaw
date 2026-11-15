//! GH #1471 (D58 roundpin, contract point 4) -- the row a turn is cut by
//! ends with the answer that ends the turn.
//!
//! WHY this file exists: a turn keeps what its first call was cut by in a
//! row of its own (`pending:turn_cut:<round>:<session>:<turn>`), and every
//! later call of the turn reads it. The tap takes the row down when the
//! answer ends the turn -- a final answer, an error, a timeout -- and keeps
//! it for an answer whose next call comes: one that asks for a tool, and one
//! whose turn is handed on to the brain (`esc`). No pin read the row after
//! an answer (roundpin review M-1; the form branch `esc` at the tap, form
//! N-5, lives on in this rule). Mutants (run in the close fix round and by
//! the strand before it, logs in their reports): the delete gone -> both
//! tests red, at "takes the row down"; the delete also on `tool_calls` ->
//! the first test red; the delete also on `esc` -> the second test red, at
//! the delete it finds between the quick answer and the brain's call (the
//! brain's call lays a row anew where none stands, so the row alone does
//! not tell).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// What a row cuts by: the cover, the leaves and the legend. The tap of a
/// tool call writes the row anew with the plan it built after the answer
/// (`parked_at`, `plan`), the cut stays.
fn cut_of(row: &Value) -> Value {
    json!({"cover": row["cover"], "leaves": row["leaves"], "legend": row["legend"]})
}

const ID: &str = "call_tool_rpcutrow_0001";

#[test]
fn the_row_stands_over_a_tool_round_and_ends_with_the_final_answer() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(3);
    let c0 = t.call("t4", 0, json!([user("Read the file, please.")]));
    let kept = turn_cut_row(&t.h, ROUND_E, "s1", "t4");
    assert!(
        kept.is_object(),
        "check of the setup: the first call of the turn lays its row: {kept} {:?}",
        t.h.stderr
    );
    t.answer(&c0, "tool_calls", json!([tool_call(ID, "read_file")]));
    assert_eq!(
        cut_of(&turn_cut_row(&t.h, ROUND_E, "s1", "t4")),
        cut_of(&kept),
        "an answer that asks for a tool keeps the row and its cut"
    );
    let c1 = t.call(
        "t4",
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
    );
    assert_eq!(
        cut_of(&turn_cut_row(&t.h, ROUND_E, "s1", "t4")),
        cut_of(&kept),
        "the later call of the turn is cut by the same row"
    );
    t.answer(&c1, "stop", json!([said("All read.")]));
    assert_eq!(
        turn_cut_row(&t.h, ROUND_E, "s1", "t4"),
        Value::Null,
        "the answer that ends the turn takes the row down: {:?}",
        t.h.stderr
    );
}

/// The quick call of a `simple` turn of the talky composite, with the form on.
fn quick_call(h: &mut Hive, turn: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "simple", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("please call my daughter")]}),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one quick call: {:?} {:?}", h.out, h.stderr);
    quick[0].clone()
}

#[test]
fn a_turn_handed_on_to_the_brain_keeps_its_row() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting_composite("talky");
    let quick = quick_call(&mut h, "t1");
    let kept = turn_cut_row(&h, ROUND_E, "s1", "t1");
    assert!(
        kept.is_object(),
        "check of the setup: the quick call lays the row of its turn: {kept} {:?}",
        h.stderr
    );
    // The quick call comes back without an answer: the turn goes to the
    // brain (`esc`), its next call comes.
    h.out.clear();
    let ops_before = h.ledger_ops.len();
    h.tap(&quick, "error", json!({"error_code": "timeout"}), json!([]));
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "check of the setup: the turn goes to the brain: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        turn_cut_row(&h, ROUND_E, "s1", "t1").is_object(),
        "a turn handed on keeps the row for the call that takes it on: {:?}",
        h.stderr
    );
    // Kept, not taken down and laid anew: the brain's call is the next call
    // of the turn and lays a row of its own where none stands, so a row
    // alone does not tell the two apart (close fix round, mutant "delete
    // also on `esc`" survived the check above). No delete of the turn's row
    // between the quick answer and the brain's call.
    let deleted: Vec<&Value> = h.ledger_ops[ops_before..]
        .iter()
        .map(|(_, op)| op)
        .filter(|op| {
            op["operation"] == json!("delete")
                && op["where"]["key"]
                    .as_str()
                    .is_some_and(|k| k.starts_with("pending:turn_cut:") && k.ends_with(":s1:t1"))
        })
        .collect();
    assert!(
        deleted.is_empty(),
        "the answer that hands the turn on does not take its row down: {deleted:?}"
    );
    // The brain's answer ends the turn and takes the row down.
    let call = brain[0].clone();
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said("I will call her.")]));
    assert_eq!(
        turn_cut_row(&h, ROUND_E, "s1", "t1"),
        Value::Null,
        "the answer of the brain ends the turn and its row: {:?}",
        h.stderr
    );
}
