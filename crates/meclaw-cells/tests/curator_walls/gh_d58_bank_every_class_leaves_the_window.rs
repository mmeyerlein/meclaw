//! GH #1465 (D58, the bank) -- after a rebuild the garbage is out.
//!
//! WHY this file exists: the garbage collection of a rebuild (R-38-2) was
//! shown on single rows and measured on live runs, never on a window whose
//! garbage somebody had counted beforehand. Every wall of the bank counts
//! its garbage per class. After a rebuild with the trigger `target` each
//! counted piece of a class that collects under the knobs of the run is
//! gone from the window of the next call or stands there in a one-line
//! form, and the keyword only that piece carried is gone with it. Every
//! class a wall counts is judged on at least one of its runs.
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

/// Whether a wall holds garbage: in its rows or beside them.
fn garbage(w: &Wall) -> bool {
    !w.classes().is_empty() || !w.beside_classes().is_empty()
}

/// The bank holds a wall with garbage: read off the list of the walls, no
/// curator run.
#[test]
fn the_bank_holds_a_wall_with_garbage() {
    let bank = bank();
    assert!(
        bank.walls.iter().any(garbage),
        "the bank holds a wall with garbage"
    );
}

/// The statement on every run of the wall `name`, when it holds garbage,
/// and every class of the wall judged on one of its runs; one test per wall
/// (`per_wall!`, OR-58-102).
fn every_class_leaves_the_window(name: &str) {
    every_class_leaves_the_window_as(name, Run::clone);
}

/// The statement on the runs of the wall `name`, each made `as_run` of it.
fn every_class_leaves_the_window_as(name: &str, as_run: fn(&Run) -> Run) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let mut seen: Vec<(&str, &str)> = Vec::new();
    if !garbage(bank.named(name)) {
        return;
    }
    let runs = run_walls_as(&bank, |w| w.name == name, &mut found, as_run);
    // Every run is judged; the violations are said together at the end.
    for (wall, o) in &runs {
        found.judge("garbage", || garbage_is_out(&bank, wall, o));
        for class in wall.classes() {
            let stands = wall
                .counted(|w| matches!(w, What::Junk(c) if *c == class))
                .into_iter()
                .any(|i| o.raw(i));
            if stands && bank.collects(class, &o.knobs) == Collects::Always {
                seen.push((wall.name, class));
            }
        }
        // What a wall sows beside its rows has no place under a cover: it
        // is judged on every run its class collects on.
        for class in wall.beside_classes() {
            if bank.collects(class, &o.knobs) == Collects::Always {
                seen.push((wall.name, class));
            }
        }
    }
    for wall in bank.walls.iter().filter(|w| w.name == name) {
        for class in wall.classes().into_iter().chain(wall.beside_classes()) {
            if !seen.contains(&(wall.name, class)) {
                found.note(format!(
                    "classes: {}: no run judged the class {class} on a piece of the raw part",
                    wall.name
                ));
            }
        }
    }
    found.finish();
}

per_wall!(every_class_leaves_the_window);

/// The same statement over every run of the wall `name` as a rebuild for an
/// expired cache with the cold collection on (`Run::cold`, GH #1464): every
/// class leaves the window under the aim too.
fn every_class_leaves_the_window_through_the_cache(name: &str) {
    every_class_leaves_the_window_as(name, Run::cold);
}

/// The walls through the cache, one test each.
mod through_the_cache {
    use super::{every_class_leaves_the_window_through_the_cache, per_wall};

    per_wall!(every_class_leaves_the_window_through_the_cache);
}
