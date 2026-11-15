//! d58-3 nachlese (GH #1539, re-check m1 on OR-583-84) -- the bridge of a
//! tool round is the bridge of before, also for a target whose profile says
//! `instructions.only`.
//!
//! WHY this file exists: since OR-583-84 the required call of a tool round
//! keeps the round's tree, so the profile's text goes into the target's
//! empty slot in that round too. The bridge beside it keeps OR-WG-6: it
//! carries neither the contract nor the text put into the slot. With
//! `instructions.only` the slot put there also took the other leaves of
//! `instructions` out of the call's tree -- and the bridge, built from that
//! tree, lost them as well, though the bridge before OR-583-84 carried them
//! (the slot was empty in the tool round then, and `only` filtered
//! nothing). Now the bridge is built from the leaves before that step.
//!
//! The bridge of before is the bridge of the same call on a profile without
//! a text: nothing goes into the slot, `only` filters nothing -- the path
//! the tool round took before OR-583-84.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The tool round of `simple`: the person's words, the tree with a leaf of
/// `instructions` that is neither a slot, the contract nor the session's.
fn tool_round(h: &mut Hive) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "simple", "families": "web", "needs": "tool"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let body = json!({"messages": [user("how is the weather in Potsdam today?")],
                      "system": {"instructions": {
                          "mode": {"text": "Be brief."},
                          "style": {"text": "Speak warmly and plainly."},
                          "sidecar": {"text": "After your answer append ONE sidecar block."}}}});
    h.lane("in_curate", ctx, hop, body);
}

/// The bridge beside the tool round, on the `talky` composite with the
/// quick cell's profile patched by `patch`; its system without the moment
/// (`frame`, a wall-clock line) and its messages.
fn bridge_with(patch: Value) -> (Value, Value) {
    let mut h = Hive::with(&[]).reporting_composite_with("talky", &[("quick", patch)]);
    tool_round(&mut h);
    let bridge = h
        .routed("quick")
        .into_iter()
        .find(|m| m.hop["call_kind"] == json!("side"))
        .unwrap_or_else(|| {
            panic!(
                "no bridge beside the tool round: {:?} {:?}",
                h.out, h.stderr
            )
        });
    let mut system = bridge.body["system"].clone();
    if let Some(s) = system.as_object_mut() {
        s.remove("frame");
    }
    (system, bridge.body["messages"].clone())
}

/// Pin: with `only` on and the slot empty, the bridge of the tool round
/// carries the same system and messages as the bridge of before -- the
/// other leaves of `instructions` included, no contract, no profile text.
#[test]
fn gh_d583_nachlese_the_bridge_of_a_tool_round_is_unchanged_under_only() {
    if !shipped() {
        return;
    }
    let (sys_now, msgs_now) = bridge_with(json!({"instructions": {"only": true}}));
    let (sys_before, msgs_before) =
        bridge_with(json!({"instructions": {"only": true, "text": ""}}));
    let ins = &sys_now["instructions"];
    assert!(
        ins.get("sidecar").is_none() && ins.get("quick").is_none(),
        "the bridge carries neither the contract nor `instructions.quick`: {ins:?}"
    );
    assert!(
        ins["style"]["text"].as_str().is_some(),
        "the bridge keeps the other leaves of `instructions`, as before: {ins:?}"
    );
    let a = meclaw_core::serde_json::to_string(&sys_now).expect("system");
    let b = meclaw_core::serde_json::to_string(&sys_before).expect("system");
    assert_eq!(
        a, b,
        "the bridge's system is byte-equal to the bridge of before"
    );
    assert_eq!(
        msgs_now, msgs_before,
        "the bridge's messages are the bridge of before"
    );
}
