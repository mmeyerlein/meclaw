//! GH #1466 (D58 recall) -- a stamp without yes-values keeps the recall.
//!
//! WHY this file exists: the yes-values are an added field of the stamp. A
//! stamp of before, a classifier that does not send them, a route cell that
//! was not asked the memory question, a seam that lost the key: all of them
//! leave the curator without the one number the rule stands on. Then the turn
//! counts as unsure and the block stays -- an empty `needs` alone never skips
//! a recall. The same holds for a value that is no probability (a word, a
//! bool, a number outside 0..1) and for a key that cannot be read.
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

/// A judged `talk` stamp without `memory` among the needs, with `needs_yes`
/// as given (`None`: the key is absent).
fn stamp_with(needs_yes: Option<&str>) -> Value {
    let mut s = json!({"path": "talk", "needs": "", "families": ""});
    if let Some(v) = needs_yes {
        s["needs_yes"] = json!(v);
    }
    s
}

#[test]
fn gh_d58_recall_a_stamp_without_yes_values_keeps_the_recall() {
    if !shipped() {
        return;
    }
    let unsure = [
        None,
        Some(""),
        Some("{}"),
        Some("{\"tool\": 0.1}"),
        Some("{\"memory\": null}"),
        Some("{\"memory\": \"low\"}"),
        Some("{\"memory\": true}"),
        Some("{\"memory\": false}"),
        Some("{\"memory\": -0.1}"),
        Some("[0.05]"),
        Some("0.05"),
        Some("not json"),
    ];
    for needs_yes in unsure {
        let mut h = on();
        let c = call(&mut h, "t1", 0, &stamp_with(needs_yes), with_recall());
        assert!(
            carries(&c),
            "needs_yes {needs_yes:?} says nothing sure about memory: the recall stays: {}",
            text(&c)
        );
        assert_eq!(
            skip_marks(&h),
            Vec::<(String, Value)>::new(),
            "needs_yes {needs_yes:?}: nothing was skipped, nothing is marked"
        );
    }
    // `recall_skip_sure` on while the route cell's `stamp_needs_yes` is off
    // (a set that switched one of the two): the seam carries the stamp of
    // before on every turn -- a path, the needs, no `needs_yes` -- and every
    // block stays, turn after turn, in the window and in the plan.
    let mut h = on();
    let of_before = stamp_with(None);
    let c1 = call(&mut h, "t1", 0, &of_before, with_recall());
    assert!(
        carries(&c1),
        "without the route's knob the recall is sent: {}",
        text(&c1)
    );
    h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
    let c2 = call(&mut h, "t2", 0, &of_before, json!([user("And the boat?")]));
    assert!(
        carries(&c2),
        "and it stays in the window of the next turn: {}",
        text(&c2)
    );
    assert_eq!(
        plan_blocks(&h, ROUND_E)
            .iter()
            .filter(|b| b["kind"] == "recall")
            .count(),
        1,
        "the plan keeps the block"
    );
    assert_eq!(
        skip_marks(&h),
        Vec::<(String, Value)>::new(),
        "nothing was skipped, nothing is marked"
    );

    // The control: the same stamp with a readable, low yes-value.
    let mut h = on();
    let c = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    assert!(
        left_out(&c),
        "with a yes-value for memory the same stamp leaves the recall out: {}",
        text(&c)
    );
}
