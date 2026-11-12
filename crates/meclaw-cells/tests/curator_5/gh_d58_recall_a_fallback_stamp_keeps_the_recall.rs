//! GH #1466 (D58 recall) -- a fallback stamp keeps the recall.
//!
//! WHY this file exists: a stamp sent on a fallback (`unsure`, `late`,
//! `error`) is no judgement. Its empty `needs` say nothing about the turn,
//! and whatever yes-values ride beside it were not good enough for the route
//! cell itself to stand behind. A wrong "needs no memory" costs most when the
//! person tells something without asking; so the rule reads a fallback as
//! "unsure" and the block stays as it does today -- even when the seam still
//! carries a low yes-value for `memory`. The control arm is the same stamp
//! without the fallback: there the block is left out, so it is the fallback
//! that kept it.
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

#[test]
fn gh_d58_recall_a_fallback_stamp_keeps_the_recall() {
    if !shipped() {
        return;
    }
    for why in ["unsure", "late", "error"] {
        let mut stamp = judged(0.05);
        stamp["fallback"] = json!(why);
        let mut h = on();
        let c = call(&mut h, "t1", 0, &stamp, with_recall());
        assert!(
            carries(&c),
            "a stamp on the fallback {why:?} keeps the recall: {}",
            text(&c)
        );
        assert_eq!(
            skip_marks(&h),
            Vec::<(String, Value)>::new(),
            "{why}: nothing was skipped, nothing is marked"
        );
    }
    // The control: the same stamp, judged.
    let mut h = on();
    let c = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    assert!(
        left_out(&c),
        "without the fallback the same stamp leaves the recall out: {}",
        text(&c)
    );
}
