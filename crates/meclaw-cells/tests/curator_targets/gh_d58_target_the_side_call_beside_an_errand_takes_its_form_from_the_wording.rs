//! D58 close (GH #1494; review 3 m2 of the target strand) -- the side call
//! beside an errand takes its form from `bridge_wording` and leaves only
//! for a named side target.
//!
//! WHY this file exists: the form of the side call beside an errand moved
//! from the curator to the lead (`bridge_wording`, D58 moveout, GH #1476):
//! `soon` -- the answer follows in this turn, the side call sees the
//! person's turn alone (`window` `turn`) --, `later` -- the result comes in
//! a later turn, the side call keeps its target's window (`window`
//! `wall`). A class the knob does not name speaks `later`, a form other
//! than these two falls away. Every pin so far ran talky's value
//! (`consult:soon,research:later`), so a lead that ignored the knob and
//! spoke talky's forms by class would have passed. Here the knob is
//! swapped, emptied and given an unknown form, and the verdict follows it.
//! And the rule of the side target holds beside an errand as beside a
//! required tool round (`gh_d58_target_a_bridge_needs_a_side_target`): a
//! `bridge_target` that names no target gives the errand no side entry.
//!
//! Runs the shipped lead's verdict (`lead_verdict`, `support/curator_hive.rs`);
//! no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The `side` entries of the lead's verdict for a turn of the stamp class
/// `path` under talky's wiring with the knobs `over` on top:
/// `(when, target, window)` each, and the errand class the verdict departs
/// for.
fn side_of(over: Value, path: &str) -> (Vec<(Value, Value, Value)>, Value) {
    let mut params = json!({});
    for (k, v) in LEAD_WIRING {
        params[k] = json!(v);
    }
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let head = json!({"channel": "text", "live": "", "session": "s1", "turn": "t1",
                      "iter": 0, "consult_id": "", "consult_class": "", "spec": "",
                      "escalate": ""});
    let state = json!({"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}});
    let stamp = json!({"path": path, "needs": [], "families": []});
    let (got, err) = cell_scope(
        "lead",
        params,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([stamp, head, state]),
    );
    let side = got["side"]
        .as_array()
        .unwrap_or_else(|| panic!("the verdict carries `side`: {got} ({err})"))
        .iter()
        .map(|e| (e["when"].clone(), e["target"].clone(), e["window"].clone()))
        .collect();
    (side, got["departs"]["class"].clone())
}

fn external(target: &str, window: &str) -> Vec<(Value, Value, Value)> {
    vec![(json!("external"), json!(target), json!(window))]
}

#[test]
fn gh_d58_target_the_side_call_beside_an_errand_takes_its_form_from_the_wording() {
    if !shipped() || !lead_shipped() {
        return;
    }
    // Check of the setup: talky's wiring, talky's forms.
    let talky = json!({});
    assert_eq!(
        side_of(talky.clone(), "complex"),
        (external("quick", "turn"), json!("consult"))
    );
    assert_eq!(
        side_of(talky, "research"),
        (external("quick", "wall"), json!("research"))
    );
    // The knob swapped: the forms follow the knob, not the class.
    let swapped = json!({"bridge_wording": "consult:later,research:soon"});
    assert_eq!(
        side_of(swapped.clone(), "complex").0,
        external("quick", "wall"),
        "consult:later keeps the target's window"
    );
    assert_eq!(
        side_of(swapped, "research").0,
        external("quick", "turn"),
        "research:soon sees the person's turn alone"
    );
    // The shipped value (empty): every class speaks `later`.
    let shipped_value = json!({"bridge_wording": ""});
    for path in ["complex", "research"] {
        assert_eq!(
            side_of(shipped_value.clone(), path).0,
            external("quick", "wall"),
            "{path}: a class the knob does not name speaks later"
        );
    }
    // A form other than `soon` or `later` falls away: the class speaks
    // `later`, the other pair of the knob stands.
    let unknown = json!({"bridge_wording": "consult:quickly,research:soon"});
    assert_eq!(
        side_of(unknown.clone(), "complex").0,
        external("quick", "wall"),
        "an unknown form falls away"
    );
    assert_eq!(
        side_of(unknown, "research").0,
        external("quick", "turn"),
        "and the named pair beside it stands"
    );
}

#[test]
fn gh_d58_target_an_errand_without_a_side_target_leaves_alone() {
    if !shipped() || !lead_shipped() {
        return;
    }
    for path in ["complex", "research"] {
        let (side, class) = side_of(json!({"bridge_target": ""}), path);
        assert!(
            side.is_empty(),
            "{path}: no side target, no side call beside the errand: {side:?}"
        );
        assert_ne!(class, Value::Null, "{path}: the errand still departs");
    }
}
