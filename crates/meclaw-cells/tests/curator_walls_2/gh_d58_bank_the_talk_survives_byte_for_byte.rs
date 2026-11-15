//! GH #1465 (D58, the bank) -- after a rebuild the talk is whole.
//!
//! WHY this file exists: a rebuild that takes the garbage out is worth
//! nothing if it touches what was said. On every wall of the bank, after a
//! rebuild with the trigger `target`, every word of the person and every
//! answer of the companion in the raw part of the plan stands in the window
//! of the next call byte for byte -- behind its id where the window names
//! ids, never shortened, never reworded -- and so do the words of the turn
//! that ordered the rebuild.
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

/// Whether a piece is talk: words of the person or an answer.
fn talk(w: &What) -> bool {
    matches!(w, What::Words | What::Answer)
}

/// The bank holds a wall with talk: read off the list of the walls, no
/// curator run.
#[test]
fn the_bank_holds_a_wall_with_talk() {
    let bank = bank();
    assert!(
        bank.walls.iter().any(|w| !w.counted(talk).is_empty()),
        "the bank holds a wall with talk"
    );
}

/// The statement on every run of the wall `name`, when it holds talk; one
/// test per wall (`per_wall!`, OR-58-102).
fn the_talk_survives_byte_for_byte(name: &str) {
    the_talk_survives_byte_for_byte_as(name, Run::clone);
}

/// The statement on the runs of the wall `name`, each made `as_run` of it.
fn the_talk_survives_byte_for_byte_as(name: &str, as_run: fn(&Run) -> Run) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    if bank.named(name).counted(talk).is_empty() {
        return;
    }
    let runs = run_walls_as(&bank, |w| w.name == name, &mut found, as_run);
    // Every run is judged; the violations are said together at the end.
    for (wall, o) in &runs {
        if found.judge("talk", || talk_is_whole(wall, o)) == Some(0) {
            found.note(format!(
                "talk: {}: the raw part holds no words of the wall",
                o.who()
            ));
        }
    }
    found.finish();
}

per_wall!(the_talk_survives_byte_for_byte);

/// The same statement over every run of the wall `name` as a rebuild for an
/// expired cache with the cold collection on (`Run::cold`, GH #1464): the
/// collection without a model leaves every word and every answer as it was.
fn the_talk_survives_byte_for_byte_through_the_cache(name: &str) {
    the_talk_survives_byte_for_byte_as(name, Run::cold);
}

/// The walls through the cache, one test each.
mod through_the_cache {
    use super::{per_wall, the_talk_survives_byte_for_byte_through_the_cache};

    per_wall!(the_talk_survives_byte_for_byte_through_the_cache);
}
