//! GH #1448 (D38 C2, OR-38-65 (b), T-18) -- `recall_by_needs` lowers the
//! recall's priority, it never drops it.
//!
//! WHY this file exists: with `recall_by_needs` "1" (Z2, GH #1441) a turn
//! whose stamp named no `memory` sent its recall as a stub. S8 in full
//! (OR-38-65) showed the cost: missing recall rows, invented facts in long
//! conversations. Now the recall bundle is always a block of the plan; the
//! stamp only lowers its `prio`, so stage 2 cuts it first under budget
//! pressure -- and without pressure the rows reach the model.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); the route cell
//! is stubbed by the hop's `path` and `needs`, no model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_00000000000000a2";
const SAID_BEFORE: &str = "WHAT WAS SAID\n  user: \"My sister Anna moved to Hamburg.\"";
const THE_ROW: &str = "My sister Anna moved to Hamburg.";

/// A stamped `talk` turn with the collector's recall pair; the brain call.
fn stamped(needs: &str) -> (Hive, Msg) {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "recall_by_needs", json!("1")),
    ]);
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "talk", "needs": needs, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    let msgs = json!([
        user("I just got back from the lake."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, SAID_BEFORE)
    ]);
    h.out.clear();
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": msgs, "system": mode("Be brief.")}),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    (h, brain[0].clone())
}

fn recall_prio(h: &Hive, call: &Msg) -> i64 {
    let id = block_id("recall", &tool_result(RECALL_ID, SAID_BEFORE));
    let snap = snapshot_of(h, call).expect("the call's plan snapshot");
    snap.iter()
        .find(|b| b["id"] == json!(id))
        .and_then(|b| b["prio"].as_i64())
        .unwrap_or_else(|| panic!("the recall bundle {id} is a block of the call: {snap:?}"))
}

#[test]
fn an_unneeded_recall_still_reaches_the_model_without_budget_pressure() {
    if !shipped() {
        return;
    }
    let (_, call) = stamped("");
    let text = sj::to_string(&call.messages()).expect("messages");
    assert!(
        text.contains(THE_ROW),
        "the stamp alone never drops the recall: {text}"
    );
}

#[test]
fn an_unneeded_recall_has_the_lower_priority() {
    if !shipped() {
        return;
    }
    let (h_no, c_no) = stamped("");
    let (h_mem, c_mem) = stamped("memory");
    let (low, high) = (recall_prio(&h_no, &c_no), recall_prio(&h_mem, &c_mem));
    assert!(
        low < high,
        "a turn that needs no memory lowers the recall's prio: {low} !< {high}"
    );
}

/// The brain call of `turn`/`iter` with `needs` on the hive `h` (the recall
/// pair in the messages on iter 0 only).
fn call_in(h: &mut Hive, turn: &str, iter: &str, needs: &str, words: &str) -> Msg {
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                     "path": "talk", "needs": needs, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    let msgs = if iter == "0" {
        json!([
            user(words),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(RECALL_ID, SAID_BEFORE)
        ])
    } else {
        json!([user(words)])
    };
    h.out.clear();
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": msgs, "system": mode("Be brief.")}),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    brain[0].clone()
}

fn preset(kind: &str) -> i64 {
    let raw = cell_config("policy")["params"]["block_prio"].clone();
    let table: sj::Value = match raw.as_str() {
        Some(s) => sj::from_str(s).expect("block_prio"),
        None => raw,
    };
    table[kind].as_i64().expect("a preset prio")
}

/// Review I2 (a): the prio is the call's, never the plan's -- the recall of
/// t1 (lowered there) carries the preset `recall` in t2, which needs memory.
#[test]
fn a_later_turn_that_needs_memory_sees_the_recall_at_its_preset() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "recall_by_needs", json!("1")),
    ]);
    let c1 = call_in(&mut h, "t1", "0", "", "I just got back from the lake.");
    assert_eq!(
        recall_prio(&h, &c1),
        preset("recall_unneeded"),
        "t1 lowers it"
    );
    h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
    let c2 = call_in(&mut h, "t2", "0", "memory", "Where does Anna live?");
    assert_eq!(
        recall_prio(&h, &c2),
        preset("recall"),
        "t2 needs memory: the recall of t1 at its preset"
    );
}

/// Review I2 (b): a recall of the running turn the plan already holds (an
/// earlier call of the turn planned it) is lowered as well.
#[test]
fn a_planned_recall_of_the_running_turn_is_lowered() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "recall_by_needs", json!("1")),
    ]);
    let c1 = call_in(
        &mut h,
        "t1",
        "0",
        "memory",
        "I just got back from the lake.",
    );
    assert_eq!(
        recall_prio(&h, &c1),
        preset("recall"),
        "needs memory: the preset"
    );
    h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
    let c2 = call_in(&mut h, "t1", "1", "", "And the boat?");
    assert_eq!(
        recall_prio(&h, &c2),
        preset("recall_unneeded"),
        "the running turn's planned recall is lowered on this call"
    );
}
