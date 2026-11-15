//! GH #1466 (D58 recall) -- a yes-value over the bound keeps the recall.
//!
//! WHY this file exists: the stamp's `p` is the confidence of the PATH and
//! says nothing about the need. "Sure" is read off the question the decider
//! answered about memory itself: its yes-value, at most `recall_skip_max_yes`
//! (shipped 0.2; a set takes the value its recall bench measured). A value
//! between the bound and the route's own `yes_threshold` is the dangerous
//! band -- too low to name `memory` in `needs`, too high to call the turn
//! sure -- and there the block stays. The bound is inclusive, it is a knob a
//! set can move, and a stamp that names `memory` keeps the block whatever the
//! bound says.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's own recall pair: the id form `call_recall_` plus 16 hex is
/// what the curator reads as "fetched for the turn, not asked by the model".
const AMBIENT_ID: &str = "call_recall_0000000000d58ca1";
const WORDS: &str = "I just got back from the lake.";
const THE_ROW: &str = "My sister Anna moved to Hamburg.";
const KNOB: &str = "recall_skip_sure";

fn said_before() -> Value {
    tool_result(AMBIENT_ID, &format!("WHAT WAS SAID\n  user: \"{THE_ROW}\""))
}

/// The person's words with the recall the collector fetched beside them.
fn with_recall() -> Value {
    json!([
        user(WORDS),
        tool_call(AMBIENT_ID, "memory_recall"),
        said_before()
    ])
}

/// The stamp keys of a judged `talk` turn as the seam carries them: no
/// `memory` among the needs, and the yes-values of the needs as the JSON text
/// of an object (`needs_yes`), `memory` at `yes_memory`.
fn judged(yes_memory: f64) -> Value {
    json!({"path": "talk", "needs": "", "families": "",
           "needs_yes": json!({"memory": yes_memory, "tool": 0.1}).to_string()})
}

/// One call of `turn`/`iter` on `in_curate` with the stamp keys `stamp` on the
/// hop; returns the call that left for the model.
fn call(h: &mut Hive, turn: &str, iter: u32, stamp: &Value, msgs: Value) -> Msg {
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                         "phase": ""});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter.to_string(),
                         "channel": "test", "audience_set": ROUND_E});
    for (k, v) in stamp.as_object().expect("the stamp keys are an object") {
        hop[k] = v.clone();
    }
    if let Some(path) = stamp.get("path") {
        ctx["path"] = path.clone();
    }
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

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

/// The fetched recall is out of the window, the pair whole.
fn left_out(c: &Msg) -> bool {
    let t = text(c);
    t.contains(WORDS) && !t.contains(THE_ROW) && !t.contains(AMBIENT_ID)
}

/// The fetched recall is in the window the call sends.
fn carries(c: &Msg) -> bool {
    text(c).contains(THE_ROW)
}

/// The first call of a turn under `stamp`, on a hive with the rule on and
/// `bound` as `recall_skip_max_yes` (`None`: the shipped bound).
fn first_call(bound: Option<f64>, stamp: &Value) -> Msg {
    let mut over = vec![("policy", KNOB, json!("1"))];
    if let Some(b) = bound {
        over.push(("policy", "recall_skip_max_yes", json!(b)));
    }
    let mut h = Hive::with(&over);
    call(&mut h, "t1", 0, stamp, with_recall())
}

#[test]
fn gh_d58_recall_a_yes_value_over_the_bound_keeps_the_recall() {
    if !shipped() {
        return;
    }
    for yes in [0.21, 0.35, 0.49] {
        let c = first_call(None, &judged(yes));
        assert!(
            carries(&c),
            "a yes-value of {yes} is over the shipped bound: the recall stays: {}",
            text(&c)
        );
    }
    // The bound is inclusive ("at most").
    let c = first_call(None, &judged(0.2));
    assert!(
        left_out(&c),
        "a yes-value AT the bound is sure: {}",
        text(&c)
    );
    // The bound is the knob's: a set that measured 0.4 skips at 0.35, and one
    // that measured 0.01 keeps at 0.05.
    let c = first_call(Some(0.4), &judged(0.35));
    assert!(
        left_out(&c),
        "with recall_skip_max_yes 0.4 a yes-value of 0.35 is sure: {}",
        text(&c)
    );
    let c = first_call(Some(0.01), &judged(0.05));
    assert!(
        carries(&c),
        "with recall_skip_max_yes 0.01 a yes-value of 0.05 is not: {}",
        text(&c)
    );
    // A stamp that names `memory` keeps the block under any bound.
    let named = json!({"path": "talk", "needs": "memory", "families": "",
                       "needs_yes": json!({"memory": 0.9}).to_string()});
    let c = first_call(Some(1.0), &named);
    assert!(
        carries(&c),
        "a stamp that names memory keeps the recall whatever the bound: {}",
        text(&c)
    );
}
