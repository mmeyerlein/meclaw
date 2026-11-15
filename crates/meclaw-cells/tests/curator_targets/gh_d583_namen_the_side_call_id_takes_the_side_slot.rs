//! d58-3 namen (GH #1525, contract point 5, N9b) -- the side call's id
//! takes the side slot.
//!
//! WHY this file exists: the first side call of a round left under the id
//! `<call>-bridge` whatever the composition calls its side slot -- the word
//! of one composition (talky's bridge) in the policy's code. The id takes
//! its ending from the slot the side call stands under (`side_slot` of the
//! lead, or the target's own); for talky, whose slot is `bridge`, the id is
//! byte-equal to before, so the corpus does not move.
//!
//! Runs the shipped curator hive with the lead (`support/curator_hive.rs`);
//! no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::json;

/// A slot name no composition uses.
const SLOT: &str = "probe_slot";

#[test]
fn gh_d583_namen_the_side_call_id_takes_the_side_slot() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("lead", "path_routes", json!("*:seat")),
        ("lead", "fallback", json!("seat:main")),
        ("lead", "bridge_target", json!("*:side")),
        ("lead", "side_slot", json!(SLOT)),
    ]);
    announce_profile(&mut h, "seat", json!({"must_call": true}));
    announce_profile(&mut h, "side", json!({"window": "turn"}));
    let calls = curate_for(
        &mut h,
        Some("seat"),
        "t1",
        0,
        json!([user("Will it rain on the walk? Look it up.")]),
        json!({"path": "", "needs": "tool", "families": "web"}),
    );
    // The main call is the one to `seat`, told apart by its target: the
    // helper `the_call` tells a side call by the ending `-bridge`, the very
    // name this pin takes out of the side call's id.
    let mains: Vec<&Msg> = calls
        .iter()
        .filter(|m| m.hop.get("target") == Some(&json!("seat")))
        .collect();
    assert_eq!(
        mains.len(),
        1,
        "check of the setup: one main call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let main_id = mains[0].hop["curator_call"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !main_id.is_empty(),
        "the main call has an id: {:?}",
        mains[0].hop
    );
    let sides: Vec<&Msg> = calls
        .iter()
        .filter(|m| m.hop.get("target") == Some(&json!("side")))
        .collect();
    assert_eq!(
        sides.len(),
        1,
        "check of the setup: one side call: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        sides[0].hop["curator_call"],
        json!(format!("{main_id}-{SLOT}")),
        "the side call's id ends in its slot: {:?}",
        sides[0].hop
    );
    let script = script_of("policy");
    assert!(
        !script.contains("\"-bridge\"") && !script.contains("'-bridge'"),
        "the policy's code names no side slot of its own"
    );
}
