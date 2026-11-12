//! GH #1466 (D58 recall) -- a skipped block leaves a mark and a snapshot line.
//!
//! WHY this file exists: a block that never reaches the plan is invisible
//! afterwards. "Was the context there?" must have an answer for it, and the
//! watch must be able to count how often the rule fires -- a rule that skips
//! half of all recalls is a different thing from one that skips a tenth, and a
//! wrong "needs no memory" shows only in that share and in the yes-values
//! behind it. So every block left out writes ONE mark `recall_skipped` with
//! the yes-value it was decided on, and the snapshot of the deciding call
//! lists the pair it left out -- the result and its call, each with the form
//! `cut` and the reason -- beside the window it sent. Only the deciding call
//! says so: decided once, marked once.
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

#[test]
fn gh_d58_recall_a_skipped_block_leaves_a_mark_and_a_snapshot_line() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", KNOB, json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    let c1 = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    assert!(left_out(&c1), "check of the setup: {}", text(&c1));

    // The mark: one per block (the pair is one block left out), under the
    // turn, with the yes-value and the call that decided.
    let marks = skip_marks(&h);
    assert_eq!(
        marks.len(),
        1,
        "one block, one mark: {marks:?} {:?}",
        h.stderr
    );
    let (turn, value) = &marks[0];
    assert_eq!(turn, "t1", "the mark names the turn");
    assert_eq!(
        value["yes"],
        json!(0.05),
        "the mark carries the yes-value the decision stood on: {value}"
    );
    assert_eq!(
        value["call"], c1.hop["curator_call"],
        "the mark names the call that decided: {value}"
    );

    // The snapshot line: the pair, each element `cut` with the reason,
    // beside the window sent.
    let sent = sent_of(&h, &c1).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(
        w.messages,
        c1.messages(),
        "the entries without `why` are the window sent"
    );
    let mut gone: Vec<(String, String, String)> = w
        .left_out
        .iter()
        .map(|(kind, why, el)| {
            assert_eq!(
                el["id"],
                json!(AMBIENT_ID),
                "the element left out is of the fetched pair: {el}"
            );
            (
                kind.clone(),
                why.clone(),
                el["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    gone.sort();
    assert_eq!(
        gone,
        vec![
            (
                "recall".to_string(),
                KNOB.to_string(),
                "tool_result".to_string()
            ),
            (
                "tool_call".to_string(),
                KNOB.to_string(),
                "tool_call".to_string()
            ),
        ],
        "the result and its call, each `cut` with the reason: {sent:?}"
    );
    assert!(
        w.left_out.iter().any(|(_, _, el)| *el == said_before()),
        "the entry names the block of the wall, the bytes that were not sent: {sent:?}"
    );

    // Decided once: the next call marks nothing and lists nothing left out.
    h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
    let c2 = call(
        &mut h,
        "t2",
        0,
        &judged(0.05),
        json!([user("And the boat?")]),
    );
    assert_eq!(
        skip_marks(&h).len(),
        1,
        "the later call writes no second mark: {:?}",
        skip_marks(&h)
    );
    let sent = sent_of(&h, &c2).expect("the snapshot row of the later call");
    assert!(
        sent_window(&h, &sent).left_out.is_empty(),
        "the later snapshot lists nothing left out: {sent:?}"
    );
}
