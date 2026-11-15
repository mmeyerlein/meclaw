//! D58-3 kern (GH #1524, contract point 1, K.8) -- a reported profile beats
//! the fallback.
//!
//! WHY this file exists: `profile_fallback` stands in for a target's profile
//! only while the curator holds none for it. Once the cell has told its row
//! and profile on `in_catalog`, that profile cuts the call, as before: a
//! fallback that outlived the report would keep a set's guess over the cell's
//! own word. Here the fallback says `only`, the reported profile does not --
//! the family stays beside the slot, so it was the report that cut.
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

fn system_with_both() -> Value {
    json!({"instructions": {"reply": {"text": REPLY}, "spec": {"text": SPEC}},
           "persona": {"name": {"text": "A patient companion."}}})
}

fn call(h: &mut Hive, turn: &str) -> String {
    let calls = curate_with_system(
        h,
        Some("spec"),
        turn,
        0,
        json!([user("Plan my week.")]),
        json!({}),
        system_with_both(),
    );
    let c = the_call(h, &calls);
    Value::Object(c.body.clone())["system"].to_string()
}

#[test]
fn gh_d583_kern_a_reported_profile_beats_the_fallback() {
    if !shipped() {
        return;
    }
    let fallback = json!({"spec": {"window": "wall", "system": "full", "tools": "all",
                                   "instructions": {"slot": "spec", "text": "", "only": true}}});
    let mut h = Hive::with(&[("policy", "profile_fallback", fallback)]);
    // The cell reports before the first call: its own profile, `only` off.
    announce_profile(
        &mut h,
        "spec",
        json!({"instructions": {"slot": "spec", "text": ""}}),
    );
    let system = call(&mut h, "t1");
    assert!(
        system.contains(SPEC) && system.contains(REPLY),
        "the reported profile cuts the call, not the fallback: without `only` the \
         family stays beside the slot: {system}"
    );
    assert!(
        h.routed("catalog_ask").is_empty(),
        "a heard target asks nothing: {:?}",
        h.out
    );
}
