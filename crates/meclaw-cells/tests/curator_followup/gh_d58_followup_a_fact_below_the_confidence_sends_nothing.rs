//! D58 followup (GH #1479, contract point 3) -- a classed fact below the
//! least confidence sends nothing.
//!
//! WHY this file exists: the curator weighed a fact by its confidence
//! (`wellbeing_min_confidence` 0.6, OR-d38-3.o15.1): "maybe tired" at 0.1 is
//! no concern to ask after. The knob moves with the rule to the cell
//! (`min_confidence`, shipped 0.6 like the curator's). The bound is "at
//! least": 0.6 passes; a fact without a confidence passes (the fallback of
//! OR-d38-3.v.9 -- the sidecar need not carry one); the knob set higher holds
//! back a fact it held before. Nothing sent means nothing into any child and
//! nothing of the cell out of the hive.
//!
//! Red until the cell exists.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn notes_for(over: &[(&str, Value)], conf: Option<f64>) -> usize {
    let mut knobs = vec![(KNOB_ON, json!("1")), (KNOB_CLOCK, json!(NOON))];
    knobs.extend(over.iter().cloned());
    let mut a = Assistant::with(&knobs);
    a.leave(
        "./talky",
        memory_out(
            "text-1",
            "t1",
            json!([fact(
                "Ada",
                "has_symptom",
                "Ada is maybe tired.",
                conf,
                true
            )]),
        ),
    );
    assert!(
        a.out.iter().all(|m| m.route() == "sidecar"),
        "nothing of the cell leaves the hive: {:?}",
        a.out
    );
    // The hearing session ends: a held concern would go now
    // (OR-d58-2.followup.19).
    session_ends(&mut a, "./talky", "text-1");
    COMPOSITES
        .iter()
        .map(|c| live_notes(&a.notes_into(c)).len())
        .sum()
}

#[test]
fn gh_d58_followup_a_fact_below_the_confidence_sends_nothing() {
    if !shipped() {
        return;
    }
    let shipped_min = child_config(CELL)["params"][KNOB_MIN].clone();
    assert_eq!(
        shipped_min.as_f64(),
        Some(0.6),
        "the bound ships as the curator's did: {shipped_min}"
    );
    assert_eq!(notes_for(&[], Some(0.1)), 0, "0.1 is under the bound");
    assert_eq!(notes_for(&[], Some(0.59)), 0, "0.59 is under the bound");
    assert_eq!(
        notes_for(&[], Some(0.6)),
        COMPOSITES.len(),
        "0.6 is at least the bound: one note per child"
    );
    assert_eq!(
        notes_for(&[], None),
        COMPOSITES.len(),
        "no confidence: the fact passes (OR-d38-3.v.9)"
    );
    assert_eq!(
        notes_for(&[(KNOB_MIN, json!(0.95))], Some(0.9)),
        0,
        "the knob raised holds 0.9 back"
    );
}
