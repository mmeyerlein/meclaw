//! GH #1465 (D58, the bank) -- after a rebuild the window lies at its aim.
//!
//! WHY this file exists: a rebuild is ordered when the window reaches the
//! target of its role and names an aim under it. The rebuild aims at the
//! size of the blocks; the finished call lays its envelope on top (keys, id
//! prefixes, its frame), and the hard lines are the model cell's to guard.
//! On every wall of the bank, with every role the wall is made for, after
//! the rebuild with the trigger `target` the window of the turn after --
//! `window_tokens`, the figure the watch reads and the provider bills, less
//! the words that turn added -- lies at most 10 percent over the aim, or the
//! rebuild wrote the mark `over_aim` (OR-58-78). The statement catches a
//! rebuild that misses its aim by far, not the envelope. Per run the test
//! prints a line `DISTANCE | ...` with the distance in percent of the aim: a
//! figure for the report.
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

use curator_bank::*;
use curator_hive::shipped;

/// The statement on every run of the wall `name`; one test per wall
/// (`per_wall!`, OR-58-102).
fn the_window_lands_at_its_aim(name: &str) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    bank.named(name);
    let runs = run_walls(&bank, |w| w.name == name, &mut found);
    // Every run is judged; the violations are said together at the end.
    for (_, o) in &runs {
        found.judge("aim", || lands_at_aim(o));
        if let Some(line) = found.judge("distance", || distance_line(o)) {
            println!("{line}");
        }
    }
    found.finish();
}

per_wall!(the_window_lands_at_its_aim);
