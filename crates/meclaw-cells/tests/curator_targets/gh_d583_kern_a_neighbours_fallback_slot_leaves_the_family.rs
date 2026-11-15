//! D58-3 kern (GH #1524, contract point 1, K.8) -- a neighbour's fallback
//! slot leaves the family of every other target.
//!
//! WHY this file exists: a target reads the family `instructions` without
//! the slots the other targets name (`foreign_slots`, contract point 3). A
//! target names its slot in the profile it reports; before its row is held
//! it names none, so a call to another target -- cut without `only` -- kept
//! that neighbour's slot beside its own. The policy's `profile_fallback`
//! stands in for the reported profile while none is held, and so it names
//! its slot to the other targets too. Here only `reply` has a fallback (its
//! slot `reply`, `only` on), nothing was reported, and the call goes to
//! `spec`, which has no fallback and no `only`: the `reply` leaf is not
//! sent, the `spec` leaf is. The counter-arm is the curator as shipped, no
//! fallback: the `reply` leaf stays in the family beside `spec`.
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
const SPEC: &str = "Write the plan as a list of steps.";

/// The system part of the composition: the slots `reply` and `spec`, both in
/// the family `instructions`.
fn system_with_both() -> Value {
    json!({"instructions": {"reply": {"text": REPLY}, "spec": {"text": SPEC}},
           "persona": {"name": {"text": "A patient companion."}}})
}

/// The first call of the session to `spec`, before any row came home; the
/// system part it sends.
fn spec_call(h: &mut Hive) -> String {
    let calls = curate_with_system(
        h,
        Some("spec"),
        "t1",
        0,
        json!([user("Plan my week.")]),
        json!({}),
        system_with_both(),
    );
    let c = the_call(h, &calls);
    assert_eq!(
        c.hop.get("target"),
        Some(&json!("spec")),
        "check of the setup: the call leaves for `spec`: {:?} {:?}",
        c.hop,
        h.stderr
    );
    Value::Object(c.body.clone())["system"].to_string()
}

#[test]
fn gh_d583_kern_a_neighbours_fallback_slot_leaves_the_family() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[(
        "policy",
        "profile_fallback",
        json!({"reply": {"window": "wall", "system": "full", "tools": "all",
                         "instructions": {"slot": "reply", "text": "", "only": true}}}),
    )]);
    let system = spec_call(&mut h);
    assert!(
        system.contains(SPEC),
        "the call's own slot stays in the family: {system}"
    );
    assert!(
        !system.contains(REPLY),
        "the slot `reply` names in its fallback is another target's -- it is not \
         sent to `spec`, though `spec` has no fallback and no `only`: {system}"
    );

    // Shipped: no fallback, no target names a slot, the family goes whole.
    let mut h = Hive::new();
    let system = spec_call(&mut h);
    assert!(
        system.contains(SPEC) && system.contains(REPLY),
        "without a fallback no target names `reply`: it stays beside `spec`: {system}"
    );
}
