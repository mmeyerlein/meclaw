//! GH #1469 (D58 errand) -- the outcome survives a rebuild unshrunk.
//!
//! WHY this file exists: a rebuild collects the scaffold of a closed errand,
//! and the `(delegation)` row of a failed one is such scaffold. The outcome
//! row is not: it is the only thing left that says what became of the
//! errand. The rebuild treats it like a failed tool result -- never
//! shortened, never counted as scaffold. Measured on the probe of the bank
//! (F6): the `gc` mark of a rebuild behind a failed errand counts
//! `scaffold`, so the collection does run over that round.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn the_outcome_survives_a_rebuild_unshrunk() {
    if !shipped() {
        return;
    }
    let now = start() + Duration::minutes(30);
    let mut h = hive(
        Some("1"),
        &[
            ("policy", "gc", json!("1")),
            ("policy", "window_target", json!(1500)),
        ],
    )
    .reporting(curator_profiles::profile("consult"));
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    fails(
        &mut h,
        start() + Duration::seconds(20),
        &e,
        "upstream_error",
    );
    // Plain talk behind the errand, so the wall stands over its target.
    for i in 0..10 {
        let at = start() + Duration::minutes(20) + Duration::seconds(10 * i);
        let turn_id = format!("f{i:02}");
        h.row(
            at,
            S,
            &turn_id,
            "user",
            &user(&format!(
                "Plain turn {i}: {}",
                curator_bank::filler(i as u64, 12)
            )),
            0,
        );
        h.row(
            at + Duration::seconds(1),
            S,
            &turn_id,
            "assistant",
            &said(&format!(
                "Plain answer {i}: {}",
                curator_bank::filler(100 + i as u64, 12)
            )),
            1,
        );
    }

    // The turn that files the outcome and finds the window at its target.
    let live = ask(&mut h, now, "live-1");
    let before = outcome_lines(&live, &e);
    assert_eq!(before.len(), 1, "the outcome is filed: {:?}", texts(&live));
    h.tap(
        &live,
        "stop",
        json!({"input_soft": 250000, "input_hard": 1000000, "tokens_prompt": 240000}),
        json!([said("Nothing that cannot wait.")]),
    );
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("the target orders a rebuild: {:?}", h.stderr));
    assert_eq!(order.body["emit_body"]["reason"], "target");
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer("What came before, condensed.", "stop");
    }
    assert!(
        h.plan_in(ROUND_E)["cover"].as_i64().is_some(),
        "the rebuild wrote a plan: {:?}",
        h.stderr
    );
    let gc = h.rows("SELECT value FROM marks WHERE kind = 'gc'");
    assert_eq!(gc.len(), 1, "one rebuild, one gc mark: {gc:?}");

    let next = ask(&mut h, now + Duration::minutes(1), "live-2");
    let after = outcome_lines(&next, &e);
    assert_eq!(
        after.len(),
        1,
        "the outcome stands after the rebuild: {:?}",
        texts(&next)
    );
    let rows = outcome_rows(&h, &e);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        after[0].ends_with(&rows[0]) && after[0].contains("upstream_error"),
        "whole, as it was filed: {after:?} {rows:?}"
    );
}
