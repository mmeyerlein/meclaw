//! GH #1464 (D58 coldgc, contract point 5) -- each trigger names its aim.
//!
//! WHY this file exists: six triggers start a rebuild, and each cuts the
//! window to an aim of its own. The aims were written down in three places
//! with three numbers; this file is the table read off the code, per preset
//! and trigger, in tokens, for a model whose package names `input_soft`
//! 200000 and for a call without a package. A change of any aim turns it
//! red -- the strand that added the cold collection changed none.
//!
//! How the table is read: `curate_mark` answers `{mark, rebuild_to}` for a
//! turn; the tap writes `rebuild_to` as the aim of the round, and the
//! rebuild the clock starts reads it. An order of the cache is written on a
//! turn whose mark is `none`, so its aim is the `rebuild_to` of that answer
//! (the aim of a `soft` rebuild); 0 = no aim of the package, the rebuild
//! then cuts to `rebuild_to` of the usable window.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// Per trigger the aim the policy names for it, `None` where the trigger
/// cannot fire under the preset.
const PROBE: &str = "(lambda lim, aim: {\
    'hard': aim(curate_mark(lim['input_hard'] + 1, lim), 'hard') if lim else None, \
    'soft': aim(curate_mark(lim['input_soft'], lim), 'soft') if lim else None, \
    'day': aim(curate_mark(1000, lim, day_due=True), 'day') if HORIZON == 'day' else None, \
    'target': aim(curate_mark(WINDOW_TARGET, lim), 'target') if WINDOW_TARGET > 0 else None, \
    'cost': cost_aim_of(lim) if lim else None, \
    'cache': aim(curate_mark(1000, lim), 'none')})\
    (ARGS, lambda cm, mark: cm['rebuild_to'] if cm['mark'] == mark else 'mark ' + cm['mark'])";

fn package() -> Value {
    json!({"input_soft": 200000, "input_hard": 1000000, "cost_in": 100, "cost_cached_in": 10})
}

fn aims(role: &str, limits: Value) -> Value {
    let (got, err) = policy_scope(curator_profiles::set_knobs(role, json!({})), PROBE, limits);
    assert!(!err.contains("Traceback"), "{err}");
    got
}

#[test]
fn gh_d58_cold_each_trigger_names_its_aim() {
    if !shipped() {
        return;
    }
    // input_soft 200000: soft and hard aim at input_soft x rebuild_to /
    // compress_at, cost at input_soft x rebuild_to, day and target at
    // window_target x rebuild_to / compress_at (never above the soft aim).
    let with_package = [
        (
            "talky",
            json!({"hard": 180000, "soft": 180000, "day": 90000, "target": 90000,
                   "cost": 90000, "cache": 180000}),
        ),
        (
            "consult",
            json!({"hard": 140000, "soft": 140000, "day": null, "target": null,
                   "cost": 70000, "cache": 140000}),
        ),
        (
            "coding",
            json!({"hard": 125000, "soft": 125000, "day": null, "target": null,
                   "cost": 50000, "cache": 125000}),
        ),
        (
            "research",
            json!({"hard": 125000, "soft": 125000, "day": null, "target": null,
                   "cost": 50000, "cache": 125000}),
        ),
        (
            "quick",
            json!({"hard": 140000, "soft": 140000, "day": 8400, "target": 8400,
                   "cost": 70000, "cache": 140000}),
        ),
    ];
    for (role, want) in with_package {
        assert_eq!(aims(role, package()), want, "{role} with a package");
    }
    // Without a package only the day and the target order a rebuild; the
    // order of the cache carries no aim.
    let without = [
        ("talky", json!(90000), json!(90000)),
        ("consult", Value::Null, Value::Null),
        ("coding", Value::Null, Value::Null),
        ("research", Value::Null, Value::Null),
        ("quick", json!(8400), json!(8400)),
    ];
    for (role, day, target) in without {
        let got = aims(role, json!({}));
        assert_eq!(got["day"], day, "{role} without a package: {got}");
        assert_eq!(got["target"], target, "{role} without a package: {got}");
        assert_eq!(got["cache"], json!(0), "{role} without a package: {got}");
    }
}
