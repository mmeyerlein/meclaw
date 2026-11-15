//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: a rebuild that collects nothing
//!
//! WHY this file exists: the figure `gc_share` of the watch had no mutant.
//! The watch reads the figure off the answer of the curator's `stats` door
//! (`gc.active_pct`: the share of the rebuilds whose `gc` mark counts
//! anything). What the knob `gc` "0" switches off is the collection of a
//! rebuild that is not over its aim: with `gc` "1" every rebuild collects,
//! with `gc` "0" only the class `shrunk_tools` runs, and only in a rebuild
//! over its aim -- the behaviour of before GH #1430 the roles with `gc` "0"
//! keep (OR-58-54).
//! Pinned, in two parts. (a) At a rebuild that is not over its aim -- the
//! day boundary orders it, the test clock moved on by a day, the target set
//! far above the wall -- the sabotage moves the number the watch reads: with
//! `gc` "1" the figure is over 0, with `gc` "0" it is 0. (b) At the rebuild
//! of the wall over its aim (the trigger `target`) `gc` "0" leaves the
//! classes `dup_recall` and `scaffold` at 0 while `shrunk_tools` stays over
//! 0. No alert is part of the statement: a threshold of the watch is an
//! upper bound. The walls and the roles are parameters, the roles the runs'
//! own. Deterministic: no model anywhere; the knob is set beside the knobs
//! of the run, nothing in the tree is changed.
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

use curator_bank::{bank, figures_at_day, per_wall, rebuild};
use curator_hive::shipped;
use meclaw_core::serde_json::json;

/// The wall of part (a): it holds garbage a rebuild under its aim collects
/// (a recall result a younger one repeats).
const UNDER_AIM: &str = "long_chat";

/// A target no wall of the bank reaches: the rebuild of the day plans under
/// its aim.
const FAR_TARGET: i64 = 240_000;

/// (a) A rebuild that is not over its aim: gc "1" collects, gc "0" not.
/// Part (b) is one test per wall (`over_its_aim`, OR-58-102).
#[test]
fn under_its_aim() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let wall = bank.named(UNDER_AIM);
    let mut judged = 0;
    for run in &wall.runs {
        let far = run.clone().with("window_target", json!(FAR_TARGET));
        let who = format!("{UNDER_AIM} as {} at the day boundary", run.label());
        let Some(sound) = figures_at_day(wall, &far.clone().with("gc", json!("1"))) else {
            continue;
        };
        let broken = figures_at_day(wall, &far.with("gc", json!("0")))
            .unwrap_or_else(|| panic!("{who} with gc 0: the day orders the rebuild as with gc 1"));
        let share = sound["gc"]["active_pct"].as_i64();
        println!(
            "DAY | {who} | raw garbage {} | gc 1: {:?} | gc 0: {}",
            sound["raw_garbage"], share, broken["gc"]["active_pct"]
        );
        assert_eq!(
            broken["gc"]["active_pct"],
            json!(0),
            "{who} with gc 0: gc.active_pct is 0 (with gc 1 {share:?}): {broken}"
        );
        assert!(
            broken["rebuild"]["rate_pct"]
                .as_i64()
                .is_some_and(|r| r > 0),
            "{who} with gc 0: the run still rebuilt, the 0 is the collection's: {broken}"
        );
        if sound["raw_garbage"] == json!(0) {
            // The raw part of this run's rebuild holds none of the garbage
            // the wall counts: nothing for gc "1" to collect either.
            continue;
        }
        assert!(
            share.is_some_and(|s| s > 0),
            "{who} with gc 1: the rebuild under its aim collects, gc.active_pct is over 0: {sound}"
        );
        judged += 1;
    }
    assert!(
        judged > 0,
        "{UNDER_AIM}: a run of the wall rebuilds at the day boundary with garbage in the raw part"
    );
}

/// (b) The wall `name` over its aim: gc "0" still shrinks old tool results
/// and collects nothing else.
fn mutant_gc_off_trips_gc_share_over_its_aim(name: &str) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let wall = bank.named(name);
    for run in &wall.runs {
        let o = rebuild(wall, &run.clone().with("gc", json!("0")));
        let mark = o.gc_mark();
        let class = |c: &str| mark["classes"][c].as_i64();
        assert_eq!(
            (class("dup_recall"), class("scaffold")),
            (Some(0), Some(0)),
            "{} over its aim with gc 0: dup_recall and scaffold collect nothing: {mark}",
            o.who()
        );
        assert!(
            class("shrunk_tools").is_some_and(|n| n > 0),
            "{} over its aim with gc 0: shrunk_tools stays over 0: {mark}",
            o.who()
        );
    }
}

/// The walls of part (b), one test each: each stands over its aim at the
/// trigger `target` and holds old tool results.
mod over_its_aim {
    use super::{mutant_gc_off_trips_gc_share_over_its_aim, per_wall};

    per_wall!(mutant_gc_off_trips_gc_share_over_its_aim; long_chat coding);
}
