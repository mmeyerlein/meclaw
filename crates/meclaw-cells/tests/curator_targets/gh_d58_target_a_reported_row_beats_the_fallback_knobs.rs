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

    // The unheard target reports its row. Its window is cut by the rows it
    // was first cut by on the state of the plan that stands (GH #1497,
    // `form_view`, budget included since review M1 of the prefix strand):
    // the call after the report still stands under the fallbacks -- the
    // prefix holds -- and orders the change of plan that takes the row on.
    // From that change on the row counts, the fallbacks no longer do.
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
        has_overflow_cut(&later),
        "inside the plan that stands the window keeps the rows it was cut by: {:?}",
        later.hop
    );
    answer(
        &mut h,
        &later,
        "stop",
        json!({}),
        json!([said("A fine walk.")]),
    );
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("the reported row orders a change of plan: {:?}", h.clock));
    // t3 stands over the hard line of the fallbacks: that excess gives the
    // order the reason `hard`, which goes before `catalog` -- the rebuild that
    // takes the reported row on is the same one. The reason `catalog` itself
    // is pinned in `gh_d58_prefix_a_profile_that_arrives_between_two_calls_
    // keeps_the_prefix`.
    assert_eq!(
        order.body["emit_body"]["reason"],
        json!("hard"),
        "{:?}",
        order.body
    );
    h.fire(&order);
    let after = say(
        &mut h,
        Some("unheard"),
        "t4",
        "Tell me about the river walk.",
    );
    assert_eq!(
        after.hop.get("target"),
        Some(&json!("unheard")),
        "{:?}",
        after.hop
    );
    assert!(
        !has_overflow_cut(&after),
        "after its report and the change of plan the row counts, the fallbacks do not: {:?}",
        after.hop
    );
}
