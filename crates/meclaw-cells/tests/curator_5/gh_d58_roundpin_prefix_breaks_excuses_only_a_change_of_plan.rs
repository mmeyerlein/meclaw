//! GH #1471 (D58 roundpin, contract point 6) -- `prefix_breaks` excuses only
//! a change of plan.
//!
//! WHY this file exists: the watch of a call over its own window gives no
//! verdict when a rebuild lay between the two windows it compares -- the
//! rebuild is the one place a prefix may change. Between two calls of ONE
//! tool round no rebuild lies any more: the round keeps the plan of its
//! first call. So a second call of a tool round that was cut by another
//! plan than the first is exactly the fault the watch is there for, and it
//! is counted; the excuse holds between two rounds only (and for the guard
//! at the hard bound, which has its own pin). The hole of GH #1463 fix
//! round 2 -- a plan written between the two calls of a round -- was
//! excused and never counted.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! fault is put into the ledger by hand: the row of the call before reads
//! as another plan would have left it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// A state of a plan no rebuild of these tests wrote.
const OTHER: &str = "1:1";

/// The snapshot row of `call` reads as another window: the first message
/// it sent is gone from its list, and with `epoch` it names that state of
/// the plan as the one it was cut by.
fn rewrite_the_row(h: &Hive, call: &Msg, epoch: Option<&str>) {
    let id = call.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let rows = h.rows(&format!(
        "SELECT cut, sent FROM snapshots WHERE call_id = '{id}'"
    ));
    let row = rows
        .first()
        .expect("check of the setup: the snapshot row of the call");
    let mut cut: Value = sj::from_str(row[0].as_str().unwrap_or("{}")).expect("cut");
    let mut sent: Vec<Value> = sj::from_str(row[1].as_str().unwrap_or("[]")).expect("sent");
    let first = sent
        .iter()
        .position(|e| e.get("carried").is_none() && e.get("why").is_none() && e["kind"] != "tools")
        .expect("check of the setup: the call sent a message");
    sent.remove(first);
    if let Some(epoch) = epoch {
        assert_ne!(
            cut["plan_epoch"],
            json!(epoch),
            "check of the setup: the call was cut by another state of the plan"
        );
        cut["plan_epoch"] = json!(epoch);
    }
    h.db.execute(
        "UPDATE snapshots SET cut = ?1, sent = ?2 WHERE call_id = ?3",
        (canonical(&cut), canonical(&Value::Array(sent)), id),
    )
    .expect("the snapshot row");
}

fn last_call(t: &Talk) -> Msg {
    t.sent.last().expect("a call").call.clone()
}

/// Pin: inside a tool round a change of plan is no excuse. The second call
/// does not begin with the window of the first, and the first was cut by
/// another state of the plan -- counted.
#[test]
fn gh_d58_roundpin_prefix_breaks_excuses_only_a_change_of_plan() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(3);
    let id = "call_tool_rpwatch_0001";
    let c0 = t.call("t4", 0, json!([user("Read the file, please.")]));
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    rewrite_the_row(&t.h, &c0, Some(OTHER));
    let c1 = t.call(
        "t4",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    t.answer(&c1, "stop", json!([said("All read.")]));
    let w = t.window_figure();
    assert_eq!(
        w["prefix_breaks"],
        json!(1),
        "two calls of one tool round cut by two plans: a break, counted: {w}"
    );
}

/// Lock: between two rounds a change of plan excuses -- the same two
/// windows, the same two states of the plan, no verdict.
#[test]
fn between_two_turns_a_change_of_plan_excuses() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(3);
    rewrite_the_row(&t.h, &last_call(&t), Some(OTHER));
    t.turn("t4", "Go on.", "Yes.");
    let w = t.window_figure();
    assert_eq!(w["n"], json!(4), "{w}");
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}

/// Lock: between two rounds the same two windows under ONE state of the
/// plan are a break -- nothing but a change of plan excuses.
#[test]
fn between_two_turns_the_same_plan_excuses_nothing() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(3);
    rewrite_the_row(&t.h, &last_call(&t), None);
    t.turn("t4", "Go on.", "Yes.");
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(1), "{w}");
}

/// Lock (review I-1): with `recall_in_wall` "0" no turn keeps a row, so a
/// rebuild in the middle of a turn changes the window of the next call by
/// design. That call has no verdict, as before GH #1471 -- the stricter
/// rule belongs to a turn that keeps a row.
#[test]
fn with_the_form_off_a_rebuild_inside_a_turn_still_excuses() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("0")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(2)),
    ]);
    for i in 1..=6 {
        t.turn(
            &format!("t{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");
    let id = "call_tool_rpwatch_0002";
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    t.h.fire(&order);
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    t.h.answer("- The person told items 1 to 4.", "stop");
    assert!(
        t.cover() > 0,
        "check of the setup: the rebuild wrote its plan between the two calls: {:?}",
        t.h.stderr
    );
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    assert!(
        c0.messages().contains(&said("Noted item 1."))
            && !c1.messages().contains(&said("Noted item 1.")),
        "check of the setup: the second call is cut by the plan of the rebuild: {:?}",
        c1.messages()
    );
    assert_ne!(
        cut_epoch(&t.h, &c0),
        cut_epoch(&t.h, &c1),
        "check of the setup: the two calls name two states of the plan"
    );
    t.answer(&c1, "stop", json!([said("All read.")]));
    let w = t.window_figure();
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "no row of the turn: a call after a rebuild has no verdict: {w}"
    );
}
