//! D58 target (GH #1483, contract point 5) -- a reported row beats the
//! fallback knobs.
//!
//! WHY this file exists: the curator's fallback knobs for window sizes
//! (`context_window`, `input_soft_fallback`, `input_hard_fallback`,
//! `input_soft_fallback_row`) stay: they are the word for a curator in front
//! of a cell that never reports (a colony without a registry). Once a target
//! has reported its row they no longer count for it. Tiny fallbacks: the
//! window of an unheard target is cut to them; the window of a reported
//! roomy target in the same hive is not.
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
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_target_a_reported_row_beats_the_fallback_knobs() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    for knob in [
        "context_window",
        "input_soft_fallback",
        "input_hard_fallback",
        "input_soft_fallback_row",
    ] {
        assert!(
            policy["params"].get(knob).is_some(),
            "the fallback knob {knob} stays"
        );
    }
    let mut h = Hive::with(&[
        ("policy", "input_soft_fallback", json!(20)),
        ("policy", "input_hard_fallback", json!(30)),
    ]);
    announce(&mut h, "heard", roomy("test/heard"), json!({}), json!([]));

    let unheard = say(
        &mut h,
        Some("unheard"),
        "t1",
        "Tell me about the river walk.",
    );
    assert!(
        has_overflow_cut(&unheard),
        "check of the setup: the fallbacks cut the window of an unheard target: {:?} {:?}",
        unheard.hop,
        h.stderr
    );
    answer(
        &mut h,
        &unheard,
        "stop",
        json!({}),
        json!([said("A fine walk.")]),
    );

    let heard = say(&mut h, Some("heard"), "t2", "Tell me about the river walk.");
    assert_eq!(
        heard.hop.get("target"),
        Some(&json!("heard")),
        "{:?}",
        heard.hop
    );
    assert!(
        !has_overflow_cut(&heard),
        "the reported row counts, the fallbacks do not: {:?}",
        heard.hop
    );
    answer(
        &mut h,
        &heard,
        "stop",
        json!({}),
        json!([said("A fine walk.")]),
    );

    // The unheard target reports its row: its next window is cut by the
    // row, the fallbacks no longer count for it either.
    announce(
        &mut h,
        "unheard",
        roomy("test/unheard"),
        json!({}),
        json!([]),
    );
    let later = say(
        &mut h,
        Some("unheard"),
        "t3",
        "Tell me about the river walk.",
    );
    assert_eq!(
        later.hop.get("target"),
        Some(&json!("unheard")),
        "{:?}",
        later.hop
    );
    assert!(
        !has_overflow_cut(&later),
        "after its report the row counts, the fallbacks do not: {:?}",
        later.hop
    );
}
