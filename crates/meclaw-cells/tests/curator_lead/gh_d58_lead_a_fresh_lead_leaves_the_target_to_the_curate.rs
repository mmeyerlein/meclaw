//! D58 lead (GH #1474, contract point 5) -- a fresh lead leaves the target
//! to the curate.
//!
//! WHY this file exists: the targets are knobs of the composite. The lead
//! names no target of its own -- without a path, with an unknown path and
//! with empty `path_routes` (once fixed to a built-in name) it leaves
//! `target` empty in its verdict and the policy takes the curate's
//! `hop.target`. So the lead ships every target knob empty, and a composite
//! that wires a lead sets them at its lead itself (none relied on the
//! shipped values before, they would vanish without a word) and names its
//! main model at its curate.
//!
//! Runs the shipped lead (its knobs as shipped) beside the shipped curator
//! (`support/curator_hive.rs`) and reads the shipped composites; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_lead::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn gh_d58_lead_a_fresh_lead_leaves_the_target_to_the_curate() {
    if !shipped() {
        return;
    }
    // The lead ships without a target.
    let lead = lead_hive_cell("lead");
    for knob in TARGET_KNOBS {
        assert_eq!(
            lead["params"][knob],
            json!(""),
            "{LEAD_TEMPLATE}/lead ships `{knob}` empty"
        );
    }

    // A wired lead with every knob as shipped: the curate's target decides.
    let shipped_knobs: Vec<(&str, &str, Value)> = TARGET_KNOBS
        .iter()
        .map(|k| ("lead", *k, lead["params"][*k].clone()))
        .collect();
    let mut h = wired(&shipped_knobs);
    announce_profile(&mut h, "north", json!({}));
    for (turn, path) in [("t1", "talk"), ("t2", "simple"), ("t3", "")] {
        let calls = curate_for(
            &mut h,
            Some("north"),
            turn,
            0,
            json!([user("hello there")]),
            json!({"path": path, "needs": [], "families": []}),
        );
        let call = the_call(&h, &calls);
        assert_eq!(
            call.hop.get("target"),
            Some(&json!("north")),
            "stamp {path:?}: a fresh lead names no target, the curate's decides: {:?} {:?}",
            call.hop,
            h.stderr
        );
        answer(&mut h, &call, "stop", warm_usage(), json!([said("Hello.")]));
    }

    // talky wires a lead: it sets the targets at its lead and names its main
    // model at its curate.
    let talky_lead = read_json(&repo("templates/talky/lead/config.json"));
    assert!(
        talky_lead["cell"]["template"]
            .as_str()
            .is_some_and(|t| t.starts_with("lead@")),
        "talky's lead is the template `lead`: {}",
        talky_lead["cell"]
    );
    for knob in ["path_routes", "fallback", "spec_target", "bridge_target"] {
        assert!(
            talky_lead["override_params"]["lead"][knob]
                .as_str()
                .is_some_and(|v| !v.is_empty()),
            "talky sets `{knob}` at its lead: {}",
            talky_lead["override_params"]
        );
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let curate = edges_of(&talky)
        .into_iter()
        .find(|e| {
            e["to"] == "./curator"
                && e["modifier"]["set_hop"]["route"].as_str() == Some("'in_curate'")
        })
        .expect("talky's curate edge");
    assert!(
        curate["modifier"]["set_hop"]["target"]
            .as_str()
            .is_some_and(|t| !t.is_empty()),
        "talky names its main model at the curate (`hop.target`): {curate}"
    );
}
