//! GH #1466 (D58 recall) -- a stamp that comes too late keeps the recall.
//!
//! WHY this file exists: the recall is looked up while the classifier still
//! runs, and a call does not wait for a stamp longer than the collector's
//! fan-in does. A first call that leaves without the stamp sends the block.
//! When the stamp arrives for a later call of the same turn, the block already
//! stands in the window the model's cache holds; taking it out then would
//! change the window in its middle (the measured cost of that in the run
//! before the form: +121 % per call, almost all cache write). So the decision
//! falls once, at the call where the block is new -- without a stamp there:
//! it stays, on every later call of the turn and in the turns after it. The
//! control arm is the stamp in time: there the block is left out.
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

/// The `recall_skipped` marks of the ledger: `(turn, value)`.
fn skip_marks(h: &Hive) -> Vec<(String, Value)> {
    h.rows("SELECT turn_id, value FROM marks WHERE kind = 'recall_skipped' ORDER BY seq")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                sj::from_str(r[1].as_str().unwrap_or("{}")).expect("the mark is JSON"),
            )
        })
        .collect()
}

fn on() -> Hive {
    Hive::with(&[("policy", KNOB, json!("1"))])
}

const TOOL_ID: &str = "call_tool_d58recall_late_0001";

#[test]
fn gh_d58_recall_a_late_stamp_keeps_the_recall() {
    if !shipped() {
        return;
    }
    let mut h = on();
    // The first call leaves before any stamp reached the seam: no path, no
    // needs, no yes-values on its hop.
    let c1 = call(&mut h, "t1", 0, &json!({}), with_recall());
    assert!(
        carries(&c1),
        "without a stamp the recall is sent as today: {}",
        text(&c1)
    );
    h.tap(
        &c1,
        "tool_calls",
        json!({}),
        json!([tool_call(TOOL_ID, "read_file")]),
    );
    // The stamp is there for the second call of the turn, and it is a sure no.
    let c2 = call(
        &mut h,
        "t1",
        1,
        &judged(0.05),
        json!([
            tool_call(TOOL_ID, "read_file"),
            tool_result(TOOL_ID, "file 1: ok")
        ]),
    );
    assert!(
        carries(&c2),
        "the block was sent with the first call, a late stamp does not take it back: {}",
        text(&c2)
    );
    h.tap(&c2, "stop", json!({}), json!([said("All read.")]));
    // And the next turn, stamped a sure no itself, keeps the block of the turn
    // before: the rule is about the recall of the running round only.
    let c3 = call(
        &mut h,
        "t2",
        0,
        &judged(0.05),
        json!([user("And the boat?")]),
    );
    assert!(
        carries(&c3),
        "a block that stayed is not decided again by a later turn: {}",
        text(&c3)
    );
    assert_eq!(
        skip_marks(&h),
        Vec::<(String, Value)>::new(),
        "nothing was skipped, nothing is marked"
    );

    // The control: the same stamp in time for the first call.
    let mut h = on();
    let c = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    assert!(
        left_out(&c),
        "in time, the same stamp leaves the recall out: {}",
        text(&c)
    );
}
