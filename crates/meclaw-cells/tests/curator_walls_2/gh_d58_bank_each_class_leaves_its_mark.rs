//! GH #1465 (D58, the bank) -- every class leaves its mark, with its count.
//!
//! WHY this file exists: a watcher sees the garbage collection only through
//! the `gc` mark of a rebuild (GH #1430). On every wall of the bank the
//! rebuild with the trigger `target` writes exactly one such mark, and for
//! every class the rebuild collects its figures are the wall's own count: a
//! class that collects under the knobs of the run names the pieces of it
//! the plan shows, a class that does not collect names none, and the
//! classes of each one-line form add up to the blocks the window of the
//! next call shows in that form.
//!
//! A class the rebuild does not collect has no figure in the mark: spent
//! notes are cleared by every call in the notes block it builds, and a mark
//! per call would be noise. For such a class the statement is that the
//! piece is gone (`gh_d58_bank_every_class_leaves_the_window`), not a mark.
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
fn each_class_leaves_its_mark(name: &str) {
    each_class_leaves_its_mark_as(name, Run::clone);
}

/// The statement on the runs of the wall `name`, each made `as_run` of it.
fn each_class_leaves_its_mark_as(name: &str, as_run: fn(&Run) -> Run) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    bank.named(name);
    let runs = run_walls_as(&bank, |w| w.name == name, &mut found, as_run);
    // Every run is judged; the violations are said together at the end.
    for (wall, o) in &runs {
        found.judge("marks", || marks_count(&bank, wall, o));
    }
    found.finish();
}

per_wall!(each_class_leaves_its_mark);

/// The same statement over every run of the wall `name` as a rebuild for an
/// expired cache with the cold collection on (`Run::cold`, GH #1464): the
/// collection under the aim writes the one `gc` mark with the count of the
/// wall, as the rebuild of the target does.
fn each_class_leaves_its_mark_through_the_cache(name: &str) {
    each_class_leaves_its_mark_as(name, Run::cold);
}

/// The walls through the cache, one test each.
mod through_the_cache {
    use super::{each_class_leaves_its_mark_through_the_cache, per_wall};

    per_wall!(each_class_leaves_its_mark_through_the_cache);
}
