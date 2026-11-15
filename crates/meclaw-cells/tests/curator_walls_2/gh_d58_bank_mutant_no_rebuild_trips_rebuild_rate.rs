//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: a curator that never rebuilds
//!
//! WHY this file exists: the figure `rebuild_rate` of the watch had no mutant.
//! The watch reads the figure off the answer of the curator's `stats` door
//! (`rebuild.rate_pct`: the share of the calls whose turn ordered a rebuild).
//! A sound curator orders a rebuild when its window reaches the target; the
//! broken one has no target to reach (`window_target` 0, set beside the knobs
//! of the run -- the shipped curator runs the wall again, nothing in the tree
//! is changed) and never orders one.
//! Pinned: the sabotage moves the number the watch reads. Every run of the
//! walls is run twice through the shipped hive and its `stats` door is asked
//! each time: as it is the figure at the wall is over 0, with the sabotage
//! it is 0. No alert is part of the statement: a threshold of the watch is
//! an upper bound, and a rate of 0 is the normal case of a short talk. The
//! walls and the roles are parameters, the roles the runs' own.
//! Deterministic: no model anywhere.
//!
//! Runs the shipped curator hive over the bank (`support/curator_bank.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::{bank, figures, per_wall};
use curator_hive::shipped;
use meclaw_core::serde_json::json;

/// The sabotage on every run of the wall `name`; one test per wall the
/// figure is read at (`per_wall!` below, OR-58-102).
fn mutant_no_rebuild_trips_rebuild_rate(name: &str) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let wall = bank.named(name);
    assert!(!wall.runs.is_empty(), "{name}: the wall has a run");
    for run in &wall.runs {
        let who = format!("{name} as {}", run.label());
        let sound = figures(wall, run);
        let rate = sound["rebuild"]["rate_pct"].as_i64();
        assert!(
            rate.is_some_and(|r| r > 0),
            "{who}: the sound curator rebuilds at the wall, rebuild.rate_pct is over 0: \
             {sound}"
        );
        let broken = figures(wall, &run.clone().with("window_target", json!(0)));
        assert_eq!(
            broken["rebuild"]["rate_pct"],
            json!(0),
            "{who} without a target: rebuild.rate_pct is 0 (sound {rate:?}): {broken}"
        );
        assert!(
            broken["rebuild"]["n"].as_i64().is_some_and(|n| n > 0),
            "{who} without a target: the 0 is counted over calls, not over nothing: {broken}"
        );
    }
}

// The walls the figure is read at.
per_wall!(mutant_no_rebuild_trips_rebuild_rate; long_chat coding);
