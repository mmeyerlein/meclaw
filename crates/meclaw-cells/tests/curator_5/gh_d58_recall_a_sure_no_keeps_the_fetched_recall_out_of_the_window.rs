//! GH #1466 (D58 recall) -- a recall the turn surely does not need stays out
//! of the window.
//!
//! WHY this file exists: the collector asks the memory for every turn the
//! moment the turn opens, before the route classifier has judged it, so most
//! turns that need no memory carry a recall block all the same. `recall_by_needs`
//! only lowers that block's priority; without budget pressure it is sent, and
//! every sent block is window the model reads and the cache writes. With
//! `recall_skip_sure` "1" the block of the running round does not enter the
//! plan when the turn's stamp is judged, names no `memory` need and its
//! yes-value for `memory` is at most `recall_skip_max_yes`. The lookup itself
//! still runs beside the classifier: the rule saves window, never latency.
//! The decision falls once, where the block would stand at the tail, and it
//! stays (the rule of the form, GH #1463): no later call brings the block
//! back. The rule is the curator's for every role, so it is run on two.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); the stamp is
//! stubbed by the keys of the seam's hop, no model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

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

fn hive(role: &str, knob: &str) -> Hive {
    let mut h = Hive::with(&[("policy", KNOB, json!(knob))]);
    if !role.is_empty() {
        h.report_profile(curator_profiles::profile(role));
    }
    h
}

fn recall_blocks(h: &Hive) -> usize {
    plan_blocks(h, ROUND_E)
        .iter()
        .filter(|b| b["kind"] == "recall")
        .count()
}

#[test]
fn gh_d58_recall_a_sure_no_keeps_the_fetched_recall_out_of_the_window() {
    if !shipped() {
        return;
    }
    for role in ["", "talky"] {
        // Check of the setup: off, the same turn under the same stamp carries
        // the recall and the plan keeps its block.
        let mut off = hive(role, "0");
        let c = call(&mut off, "t1", 0, &judged(0.05), with_recall());
        assert!(
            carries(&c),
            "role {role:?}, off: the recall is sent: {}",
            text(&c)
        );
        off.tap(&c, "stop", json!({}), json!([said("Nice.")]));
        assert_eq!(
            recall_blocks(&off),
            1,
            "role {role:?}, off: the plan keeps the block"
        );

        let mut h = hive(role, "1");
        let c1 = call(&mut h, "t1", 0, &judged(0.05), with_recall());
        assert!(
            left_out(&c1),
            "role {role:?}: a sure no keeps the fetched recall out, the pair whole, \
             and the person's words stay: {}",
            text(&c1)
        );
        h.tap(&c1, "stop", json!({}), json!([said("Nice.")]));
        assert_eq!(
            recall_blocks(&h),
            0,
            "role {role:?}: the block left out is no block of the plan"
        );
        // Decided once, and it stays: a later turn that does need memory does
        // not get the block of the turn before back.
        let needs_memory = json!({"path": "talk", "needs": "memory", "families": "",
                                  "needs_yes": json!({"memory": 0.9}).to_string()});
        let c2 = call(
            &mut h,
            "t2",
            0,
            &needs_memory,
            json!([user("Where does Anna live?")]),
        );
        let t = text(&c2);
        assert!(
            !t.contains(THE_ROW) && !t.contains(AMBIENT_ID),
            "role {role:?}: the decision stays: {t}"
        );
    }
}
