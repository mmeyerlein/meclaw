//! D58-3 kern (GH #1524, contract point 1, K.8; lab case soul2me #247) -- a
//! speculative call before its target's row is cut by `profile_fallback`, and
//! so is every later call of the same state of the plan.
//!
//! WHY this file exists: soul2me #247 (lab `lab_speaker` test_4, red 3 of 3
//! on an idle lane) caught the speculative call of the quick voice carrying
//! the reply contract (`instructions.reply`) beside its own slot on the first
//! turns of a fresh colony. Two causes, both read from the policy script:
//! (a) the first speculative call of a fresh state of the plan goes out
//! before any catalogue row is held (the ask goes beside the call, never
//! instead of it, OR-d38-3.c3b.1), so it was cut on the shipped fields with
//! `instructions.only` off and the whole family stayed; (b) `form_view`
//! (GH #1497) cuts every later call of that target on the same state of the
//! plan with the rows held at the first cut -- none -- so the row that came
//! home within a second took no effect until the plan changed. With
//! `profile_fallback` the target's profile stands in while the view holds no
//! profile: both calls drop the reply leaf. The other file
//! (`…a_first_spec_call_without_a_row_drops_the_reply_leaf`) pins the
//! non-speculative first call and the shipped counter-arm.
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

/// The system part of the composition: the speculating target's own slot
/// `spec` and the slot `reply` of another target, both in `instructions`.
fn system_with_both() -> Value {
    json!({"instructions": {"reply": {"text": REPLY}, "spec": {"text": SPEC}},
           "persona": {"name": {"text": "A patient companion."}}})
}

/// The whole profile of the target `spec`: its own slot, and `only` on.
fn spec_profile() -> Value {
    json!({"window": "wall", "system": "full", "tools": "all",
           "instructions": {"slot": "spec", "text": "", "only": true}})
}

/// A speculating composite whose speculation goes to `spec`, with the
/// fallback set for it -- nothing reported yet.
fn spec_hive() -> Hive {
    Hive::with(&[
        ("lead", "spec_ahead", json!("1")),
        ("lead", "spec_target", json!("spec")),
        (
            "policy",
            "profile_fallback",
            json!({"spec": spec_profile()}),
        ),
    ])
}

/// The collector's early curate of `turn` (`hop.spec` "1", no path): the
/// speculative call, and its system part as sent.
fn spec_call(h: &mut Hive, turn: &str) -> String {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("Plan my week.")], "system": system_with_both()}),
    );
    let calls: Vec<Msg> = model_calls(h)
        .into_iter()
        .filter(|m| m.hop.get("spec") == Some(&json!("1")))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "one speculative call on {turn}: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        calls[0].hop.get("target"),
        Some(&json!("spec")),
        "the speculation leaves for its target: {:?}",
        calls[0].hop
    );
    Value::Object(calls[0].body.clone())["system"].to_string()
}

#[test]
fn gh_d583_kern_a_spec_call_before_its_row_drops_the_reply_leaf() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();

    // (a) soul2me #247: the first speculative call of a fresh state of the
    // plan, before any row is held.
    let system = spec_call(&mut h, "t1");
    assert!(system.contains(SPEC), "the own slot is sent: {system}");
    assert!(
        !system.contains(REPLY),
        "(a) the speculative call before its row is cut by the fallback's `only`: \
         no reply leaf of another target: {system}"
    );
    assert_eq!(
        h.routed("catalog_ask").len(),
        1,
        "the ask goes beside the speculative call, never instead of it: {:?}",
        h.out
    );
}

/// Arm (b) on its own, so that its red line does not hide behind arm (a).
#[test]
fn gh_d583_kern_a_later_call_on_the_same_plan_drops_the_reply_leaf() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    // The first speculative call of the fresh state of the plan; arm (a)
    // judges it, here it only lays the kept rows of the first cut (none).
    spec_call(&mut h, "t1");

    // The row comes home right after, with the profile -- no change of plan
    // between (`form_view` keeps the rows of the first cut).
    announce_profile(&mut h, "spec", spec_profile());

    // (b) soul2me #247: the next turn's speculative call on the same state of
    // the plan still reads the rows kept at the first cut (none); the
    // fallback stands in for them.
    let system = spec_call(&mut h, "t2");
    assert!(system.contains(SPEC), "the own slot is sent: {system}");
    assert!(
        !system.contains(REPLY),
        "(b) a later call on the same state of the plan, the row arrived but not \
         yet taken on, is cut by the fallback: no reply leaf: {system}"
    );
}
