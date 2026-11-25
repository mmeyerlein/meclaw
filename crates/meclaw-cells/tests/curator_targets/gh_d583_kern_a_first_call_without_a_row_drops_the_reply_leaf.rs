//! D58-3 kern (GH #1524, contract point 1, K.8) -- the first call to a
//! target without a row takes its profile from `profile_fallback`.
//!
//! WHY this file exists: a target's profile reaches the curator beside its
//! catalog row, on `in_catalog`, after the cell asked for it. The first call
//! of a session to a target goes before that answer (the ask goes beside the
//! call and never instead of it, OR-d38-3.c3b.1), so it was cut with the
//! shipped fields: `instructions.only` off, and the family `instructions`
//! stayed whole beside the target's own slot. Measured in the lab: the first
//! call of a fresh colony carried the reply instructions of another
//! target. The policy's param `profile_fallback` (`{<target>: <profile>}`,
//! shipped `{}`) stands in for the reported profile while none is held: its
//! `instructions.slot`/`only` and its knobs cut the call as a reported one
//! would. The counter-arm is the curator as shipped, no fallback: the family
//! stays beside the slot, as today -- a set that wants the first call cut
//! sets the fallback.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const REPLY: &str = "Answer the person in one short sentence.";
const PLAN: &str = "Write the plan as a list of steps.";

/// The system part of the composition: the target's own slot `plan` and the
/// slot `reply` of another target, both in the family `instructions`.
fn system_with_both() -> Value {
    json!({"instructions": {"reply": {"text": REPLY}, "plan": {"text": PLAN}},
           "persona": {"name": {"text": "A patient companion."}}})
}

/// The whole profile of the target `plan`, as its cell would report it:
/// its own slot, and `only` on.
fn plan_profile() -> Value {
    json!({"window": "wall", "system": "full", "tools": "all",
           "instructions": {"slot": "plan", "text": "", "only": true}})
}

/// The first call of the session to `plan`, before any row came home.
fn first_call(h: &mut Hive) -> (Msg, String) {
    let calls = curate_with_system(
        h,
        Some("plan"),
        "t1",
        0,
        json!([user("Plan my week.")]),
        json!({}),
        system_with_both(),
    );
    let c = the_call(h, &calls);
    let system = Value::Object(c.body.clone())["system"].to_string();
    (c, system)
}

#[test]
fn gh_d583_kern_a_first_call_without_a_row_drops_the_reply_leaf() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[(
        "policy",
        "profile_fallback",
        json!({"plan": plan_profile()}),
    )]);
    let (c, system) = first_call(&mut h);
    assert_eq!(
        c.hop.get("target"),
        Some(&json!("plan")),
        "the call leaves for its target: {:?} {:?}",
        c.hop,
        h.stderr
    );
    assert!(
        system.contains(PLAN),
        "the target's own slot is sent: {system}"
    );
    assert!(
        !system.contains(REPLY),
        "with the fallback's `only` the slot takes the place of the family -- the \
         reply leaf of another target is not sent on the first call: {system}"
    );
    assert_eq!(
        h.routed("catalog_ask").len(),
        1,
        "the ask still goes beside the call, never instead of it: {:?}",
        h.out
    );

    // Shipped: no fallback, byte for byte the curator of before.
    assert_eq!(
        cell_config("policy")["params"]["profile_fallback"],
        json!({}),
        "`profile_fallback` ships empty"
    );
    let mut h = Hive::new();
    let (_, system) = first_call(&mut h);
    assert!(
        system.contains(PLAN) && system.contains(REPLY),
        "without a fallback the first call is cut on the shipped fields: the family \
         stays beside the slot: {system}"
    );
}
