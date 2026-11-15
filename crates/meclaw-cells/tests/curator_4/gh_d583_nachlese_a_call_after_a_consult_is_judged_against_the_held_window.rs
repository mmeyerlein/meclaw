//! d58-3 nachlese (GH #1539, B1 break 1) -- a call after a consult is judged
//! against the window the target holds.
//!
//! WHY this file exists: the watch of a call over its own window compared
//! its messages with the newest call to the same target, and took the state
//! of the plan from that row. A consult to the same target sends no system
//! part and other messages; the call after it was compared with the
//! consult's messages under the consult's state of the plan. In the S8 run
//! (diag B1, probe-10) a change of plan lay between the window the target
//! held and that call -- an excuse -- yet the watch counted a break, because
//! the consult was cut by the new plan already. The tree was already judged
//! against the held window (GH #1497); now the messages, the round and the
//! state of the plan are too.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! consult is put into the ledger by hand: the row of the call before reads
//! as a call without a system part and with other messages.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// A state of a plan no rebuild of these tests wrote.
const OTHER: &str = "1:1";

/// Rewrite the snapshot row of `call`: with `epoch` the state of the plan it
/// names, with `consult` no system part, with `first_gone` without its first
/// message.
fn rewrite(h: &Hive, call: &Msg, epoch: Option<&str>, consult: bool, first_gone: bool) {
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
    if let Some(epoch) = epoch {
        assert_ne!(cut["plan_epoch"], json!(epoch), "check of the setup");
        cut["plan_epoch"] = json!(epoch);
    }
    if consult {
        assert!(
            sent.iter().any(|e| e.get("carried").is_some()),
            "check of the setup: the call listed a system part"
        );
        sent.retain(|e| e.get("carried").is_none());
    }
    if first_gone {
        let first = sent
            .iter()
            .position(|e| {
                e.get("carried").is_none() && e.get("why").is_none() && e["kind"] != "tools"
            })
            .expect("check of the setup: the call sent a message");
        sent.remove(first);
    }
    h.db.execute(
        "UPDATE snapshots SET cut = ?1, sent = ?2 WHERE call_id = ?3",
        (canonical(&cut), canonical(&Value::Array(sent)), id),
    )
    .expect("the snapshot row");
}

/// `prefix_break` of the call `t4` after three told turns: `held` rewrites
/// the row of `t2` (the window the target holds), `consult` the row of `t3`.
fn verdict(held: (Option<&str>, bool), consult_first_gone: bool) -> Value {
    clear_of_midnight();
    let mut t = Talk::told(3);
    let c2 = t.sent[1].call.clone();
    let c3 = t.sent[2].call.clone();
    rewrite(&t.h, &c2, held.0, false, held.1);
    rewrite(&t.h, &c3, None, true, consult_first_gone);
    let c4 = t.call("t4", 0, json!([user("Go on.")]));
    let id = c4.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let est: Value = sj::from_str(&t.h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    t.answer(&c4, "stop", json!([said("Yes.")]));
    est["prefix_break"].clone()
}

/// Pin: a change of plan lay between the held window and the consult; the
/// consult's messages are not the call's. Judged against the held window,
/// the change of plan excuses.
#[test]
fn gh_d583_nachlese_a_call_after_a_consult_is_judged_against_the_held_window() {
    if !shipped() {
        return;
    }
    assert_eq!(verdict((Some(OTHER), false), true), json!(0));
}

/// Lock: under one state of the plan, a held window the call does not begin
/// with is a break -- though the consult's messages would have matched.
#[test]
fn the_held_window_under_the_same_plan_excuses_nothing() {
    if !shipped() {
        return;
    }
    assert_eq!(verdict((None, true), false), json!(1));
}

/// Lock: nothing rewritten but the consult's system part -- no break.
#[test]
fn a_consult_that_changed_nothing_breaks_nothing() {
    if !shipped() {
        return;
    }
    assert_eq!(verdict((None, false), false), json!(0));
}
