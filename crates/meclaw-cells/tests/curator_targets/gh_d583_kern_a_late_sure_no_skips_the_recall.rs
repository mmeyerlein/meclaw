//! D58-3 kern (GH #1524, contract point 2, N1) -- a late stamp that surely
//! says "no memory" skips the recall.
//!
//! WHY this file exists: `recall_sure_no` read every stamp on a fallback as
//! unsure. A `late` stamp is no guess, though: its decider answered, only
//! after `late_ms`, and with the route fix it carries the yes-values of those
//! answers (`needs_yes`). Its path is the fallback class and its `needs` are
//! empty, so the other locks of the rule do not hold it. With
//! `recall_skip_sure` on, a late stamp with a low yes for `memory` leaves the
//! fetched recall out of the call, as a judged one does, and books the same
//! mark. A late stamp with a high yes keeps it.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const AMBIENT_ID: &str = "call_recall_0000000000d58ca1";
const WORDS: &str = "I just got back from the lake.";
const THE_ROW: &str = "My sister Anna moved to Hamburg.";
const KNOB: &str = "recall_skip_sure";

fn with_recall() -> Value {
    json!([
        user(WORDS),
        tool_call(AMBIENT_ID, "memory_recall"),
        tool_result(AMBIENT_ID, &format!("WHAT WAS SAID\n  user: \"{THE_ROW}\""))
    ])
}

/// The stamp keys of a `talk` turn as the seam carries them, `memory` at
/// `yes_memory`; `fallback` when the stamp was sent on one.
fn stamp(yes_memory: f64, fallback: Option<&str>) -> Value {
    let mut s = json!({"path": "talk", "needs": "", "families": "",
                       "needs_yes": json!({"memory": yes_memory, "tool": 0.1}).to_string()});
    if let Some(f) = fallback {
        s["fallback"] = json!(f);
    }
    s
}

fn call(h: &mut Hive, stamp: &Value) -> Msg {
    let mut hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": ""});
    let mut ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                         "channel": "test", "audience_set": ROUND_E});
    for (k, v) in stamp.as_object().expect("the stamp keys are an object") {
        hop[k] = v.clone();
    }
    ctx["path"] = stamp["path"].clone();
    h.out.clear();
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": with_recall(), "system": mode("Be brief.")}),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    brain[0].clone()
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

fn left_out(c: &Msg) -> bool {
    let t = text(c);
    t.contains(WORDS) && !t.contains(THE_ROW) && !t.contains(AMBIENT_ID)
}

fn skip_marks(h: &Hive) -> usize {
    h.rows("SELECT value FROM marks WHERE kind = 'recall_skipped'")
        .len()
}

fn on() -> Hive {
    Hive::with(&[("policy", KNOB, json!("1"))])
}

#[test]
fn gh_d583_kern_a_late_sure_no_skips_the_recall() {
    if !shipped() {
        return;
    }
    // The control: judged, the recall is left out and one mark booked.
    let mut judged = on();
    let c = call(&mut judged, &stamp(0.05, None));
    assert!(left_out(&c), "judged: {}", text(&c));
    let judged_marks = skip_marks(&judged);
    assert!(judged_marks > 0, "a skip is booked");

    let mut h = on();
    let c = call(&mut h, &stamp(0.05, Some("late")));
    assert!(
        left_out(&c),
        "a late stamp that surely says no memory leaves the recall out: {}",
        text(&c)
    );
    assert_eq!(
        skip_marks(&h),
        judged_marks,
        "and books the mark a judged stamp books"
    );

    // A late stamp with a high yes keeps it.
    let mut h = on();
    let c = call(&mut h, &stamp(0.9, Some("late")));
    assert!(
        text(&c).contains(THE_ROW),
        "a late stamp that says yes keeps the recall: {}",
        text(&c)
    );
    assert_eq!(skip_marks(&h), 0, "nothing skipped, nothing booked");
}
