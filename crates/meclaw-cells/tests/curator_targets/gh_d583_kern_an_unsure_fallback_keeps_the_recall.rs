//! D58-3 kern (GH #1524, contract point 2, N1) -- every fallback but `late`
//! keeps the recall.
//!
//! WHY this file exists: the rule `recall_skip_sure` takes a `late` stamp at
//! its word, because its decider answered. Every other fallback stays
//! "unsure": `unsure` is a verdict the route cell does not stand behind,
//! `error` has none, and a fallback word the curator does not know is no
//! judgement either. Even with a low yes for `memory` on the seam the recall
//! stays. The control is the same stamp judged: there it is left out.
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

fn stamp(fallback: Option<&str>) -> Value {
    let mut s = json!({"path": "talk", "needs": "", "families": "",
                       "needs_yes": json!({"memory": 0.05, "tool": 0.1}).to_string()});
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

fn on() -> Hive {
    Hive::with(&[("policy", KNOB, json!("1"))])
}

#[test]
fn gh_d583_kern_an_unsure_fallback_keeps_the_recall() {
    if !shipped() {
        return;
    }
    for why in ["unsure", "error", "timeout"] {
        let mut h = on();
        let c = call(&mut h, &stamp(Some(why)));
        assert!(
            text(&c).contains(THE_ROW),
            "a stamp on the fallback {why:?} keeps the recall: {}",
            text(&c)
        );
        assert!(
            h.rows("SELECT value FROM marks WHERE kind = 'recall_skipped'")
                .is_empty(),
            "{why}: nothing skipped, nothing booked"
        );
    }
    let mut h = on();
    let c = call(&mut h, &stamp(None));
    assert!(
        !text(&c).contains(THE_ROW),
        "the control: judged, the same stamp leaves the recall out: {}",
        text(&c)
    );
}
