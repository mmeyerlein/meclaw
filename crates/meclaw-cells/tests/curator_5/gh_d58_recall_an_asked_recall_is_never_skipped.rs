//! GH #1466 (D58 recall) -- a recall the model asked for is never skipped.
//!
//! WHY this file exists: the rule is about the recall the collector fetched
//! on its own, before anyone knew whether the turn needs it. A `memory_recall`
//! the model called itself is the opposite case: the model has seen the turn
//! and said it needs memory, whatever the classifier thought. Leaving that
//! result out would make the model call it again -- or answer without what it
//! asked for. So the model's own pair stays under the surest "no" of the
//! stamp, also in a turn whose fetched recall is left out beside it.
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

fn on() -> Hive {
    Hive::with(&[("policy", KNOB, json!("1"))])
}

/// A provider's id: none of the collector's own.
const ASKED_ID: &str = "toolu_01d58recallaskedbymodel";
const ASKED_ROW: &str = "We sold the old car last spring.";

fn asked_pair() -> Value {
    json!([
        tool_call(ASKED_ID, "memory_recall"),
        tool_result(ASKED_ID, &format!("WHAT WAS SAID\n  user: \"{ASKED_ROW}\""))
    ])
}

#[test]
fn gh_d58_recall_an_asked_recall_is_never_skipped() {
    if !shipped() {
        return;
    }
    // A turn without a fetched recall: the model asks, the result comes with
    // the second call, the stamp is a sure no on both.
    let mut h = on();
    let c1 = call(&mut h, "t1", 0, &judged(0.05), json!([user(WORDS)]));
    h.tap(
        &c1,
        "tool_calls",
        json!({}),
        json!([tool_call(ASKED_ID, "memory_recall")]),
    );
    let c2 = call(&mut h, "t1", 1, &judged(0.05), asked_pair());
    let t = text(&c2);
    assert!(
        t.contains(ASKED_ROW) && t.contains(ASKED_ID),
        "the recall the model asked for is sent: {t}"
    );
    assert_eq!(
        skip_marks(&h),
        Vec::<(String, Value)>::new(),
        "an asked recall is never marked as skipped"
    );

    // The same with a fetched recall beside it: that one is left out, the
    // model's own stays.
    let mut h = on();
    let c1 = call(&mut h, "t1", 0, &judged(0.05), with_recall());
    assert!(
        left_out(&c1),
        "the fetched recall of the turn is left out: {}",
        text(&c1)
    );
    h.tap(
        &c1,
        "tool_calls",
        json!({}),
        json!([tool_call(ASKED_ID, "memory_recall")]),
    );
    let c2 = call(&mut h, "t1", 1, &judged(0.05), asked_pair());
    let t = text(&c2);
    assert!(
        t.contains(ASKED_ROW) && t.contains(ASKED_ID),
        "the model's own recall stays beside a skipped one: {t}"
    );
    assert!(
        !t.contains(THE_ROW),
        "and the skipped one does not come back with it: {t}"
    );
    assert_eq!(
        skip_marks(&h).len(),
        1,
        "one mark, for the fetched block alone: {:?}",
        skip_marks(&h)
    );
}
