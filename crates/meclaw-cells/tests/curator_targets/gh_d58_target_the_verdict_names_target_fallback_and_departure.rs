//! D58 target (GH #1483, contract points 4 and 9) -- the verdict names
//! target, fallback and departure.
//!
//! WHY this file exists: the policy derived the target, the hand-off of a
//! failed call and the errand that leaves the composite from way names
//! (`quick` hands to `brain`, `consult`/`research` leave). Now the lead's
//! verdict says it in fields of its own and the policy reads them: `target`
//! (a name the composite wires, never `held` or `backend`), `fallback` (the
//! target a failed or unspeakable call goes to), `departs` (`{class,
//! wait_ms}` of an errand that leaves the composite, empty otherwise). The
//! lead takes all of it from knobs: `path_routes` maps a path to a target or
//! an errand class, `departures` names each class with its wait, `fallback`
//! maps a target to its fallback. Unset, the lead names no target at all.
//!
//! Pins the pure decision `lead_verdict(stamp, head, state)` of the shipped
//! lead script, like `gh_d38_c1_the_lead_names_way_and_target`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn head() -> Value {
    json!({"channel": "text", "live": "", "session": "s1",
           "turn": "t1", "iter": 0, "consult_id": "", "consult_class": "",
           "spec": "", "escalate": ""})
}

fn no_state() -> Value {
    json!({"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}})
}

fn verdict(knobs: Value, path: &str) -> (Value, String) {
    assert!(lead_shipped(), "the lead cell is shipped");
    cell_scope(
        "lead",
        knobs,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([{"path": path, "needs": [], "families": []}, head(), no_state()]),
    )
}

#[test]
fn gh_d58_target_the_verdict_names_target_fallback_and_departure() {
    if !shipped() {
        return;
    }
    let knobs = json!({"path_routes": "talk:north,simple:south,complex:advice",
                       "departures": "advice:15000",
                       "fallback": "south:north"});
    let (v, err) = verdict(knobs.clone(), "simple");
    assert_eq!(v["target"], json!("south"), "{v} {err}");
    assert_eq!(v["fallback"], json!("north"), "{v} {err}");
    assert_eq!(v["departs"], json!({}), "stays in the composite: {v}");

    let (v, err) = verdict(knobs.clone(), "talk");
    assert_eq!(v["target"], json!("north"), "{v} {err}");
    assert_eq!(v["fallback"], json!(""), "no fallback named for north: {v}");

    let (v, err) = verdict(knobs.clone(), "complex");
    assert_eq!(
        v["departs"],
        json!({"class": "advice", "wait_ms": 15000}),
        "an errand class leaves the composite with its wait: {v} {err}"
    );
    assert!(
        v["target"] != json!("backend") && v["target"] != json!("held"),
        "`target` carries names only: {v}"
    );

    let (v, err) = verdict(json!({}), "simple");
    assert_eq!(
        v["target"],
        json!(""),
        "unset knobs name no target -- no built-in way: {v} {err}"
    );
    assert_eq!(v["departs"], json!({}), "{v}");
    let lead = lead_config("lead");
    for knob in ["path_routes", "departures", "fallback"] {
        assert_eq!(
            lead["params"][knob],
            json!(""),
            "the lead ships `{knob}` empty -- the composite names its targets"
        );
        assert!(
            lead["contract"]["settings"].get(knob).is_some(),
            "a set can turn `{knob}`"
        );
    }
}
