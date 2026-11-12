//! GH #1464 (D58 coldgc, addendum 10.11.) -- the cold aim is its own value.
//!
//! WHY this file exists: the order of an expiring cache is written on a
//! turn that crossed no line, so the aim it carried was the aim of a `soft`
//! rebuild -- for talky 180000 tokens at a package of 200000, a window
//! talky never reaches, because its `target` rebuilds at 100000. "Over the
//! aim" meant "never" for the cold cache. The knob `cold_to` gives the
//! cold cache an aim of its own: a share of the role's `window_target`,
//! whatever the package of the last answer says. A role without a
//! `window_target` has no such aim and only collects (aim 0). Left empty,
//! the inherited aim stands as before.
//!
//! Runs the pure half of the shipped policy (`support/curator_cold.rs`).

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_cold_the_cold_aim_is_its_own_value() {
    if !shipped() {
        return;
    }
    // talky: window_target 100000 x cold_to, never the inherited aim.
    for inherited in [180000, 90000, 4_500_000, 0] {
        assert_eq!(
            cold_aim(
                curator_profiles::set_knobs("talky", json!({"cold_to": "0.45"})),
                inherited
            ),
            json!(45000),
            "talky with cold_to 0.45, inherited {inherited}: 100000 x 0.45"
        );
    }
    // The share of the role's own target, as a set turns it.
    assert_eq!(
        cold_aim(
            curator_profiles::set_knobs("talky", json!({"cold_to": "0.2", "window_target": 3000})),
            180000
        ),
        json!(600),
        "window_target 3000 x cold_to 0.2"
    );
    assert_eq!(
        cold_aim(
            curator_profiles::set_knobs("quick", json!({"cold_to": 0.5})),
            140000
        ),
        json!(6000),
        "quick: window_target 12000 x 0.5, a number as well as a string"
    );
    // A role without a target has no cold aim: it only collects.
    for role in ["consult", "coding", "research"] {
        assert_eq!(
            cold_aim(
                curator_profiles::set_knobs(role, json!({"cold_to": "0.45"})),
                140000
            ),
            json!(0),
            "{role} has no window_target: no cold aim, the cache only collects"
        );
    }
    // Empty: the aim the order inherited, as before.
    for (role, inherited) in [("talky", 180000), ("consult", 140000), ("quick", 0)] {
        assert_eq!(
            cold_aim(curator_profiles::set_knobs(role, json!({})), inherited),
            json!(inherited),
            "{role} without cold_to keeps the inherited aim"
        );
        assert_eq!(
            cold_aim(
                curator_profiles::set_knobs(role, json!({"cold_to": ""})),
                inherited
            ),
            json!(inherited),
            "{role} with an empty cold_to keeps the inherited aim"
        );
    }
}

/// Review M5: an infinite share is no share. `cold_to` "inf" (or any
/// spelling Python reads as infinite) acts as the empty knob -- the
/// inherited aim -- instead of breaking the rebuild on `round(inf)`.
#[test]
fn gh_d58_cold_an_infinite_cold_to_is_an_empty_knob() {
    if !shipped() {
        return;
    }
    for v in ["inf", "Infinity", "+inf", "1e999"] {
        for (role, inherited) in [("talky", 180000), ("consult", 140000)] {
            assert_eq!(
                cold_aim(
                    curator_profiles::set_knobs(role, json!({"cold_to": v})),
                    inherited
                ),
                json!(inherited),
                "{role} with cold_to {v:?} keeps the inherited aim"
            );
        }
    }
}
