//! GH #1465 (D58, the bank) -- an open errand stays, closed ones shrink.
//!
//! WHY this file exists: the scaffold of an errand (its `(delegation)` and
//! `(advice)` rows) is garbage only once the errand is over. On every wall
//! of the bank that holds an open errand, after a rebuild with the trigger
//! `target`, the rows of the open errand stand whole on every run; on the
//! runs whose knobs collect the class, the rows of the closed errands stand
//! in their one-line form in the same window. Each such wall has a run that
//! collects.
//!
//! GH #1469: an errand that ended WITHOUT an answer -- its call failed, or
//! nobody answered before the deadline -- is closed like the others, and its
//! `(delegation)` row shrinks like theirs. What became of it must not go
//! with that row: on the run that sets `errand_outcome` "1" the window after
//! the rebuild says the outcome of each such errand, once and whole.
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

#[test]
fn an_open_errand_stays_while_closed_ones_shrink() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let open = |w: &What| matches!(w, What::Open(_));
    let runs = run_walls(&bank, |w| !w.counted(open).is_empty(), &mut found);
    assert!(
        !runs.is_empty(),
        "the bank holds a wall with an open errand"
    );
    let mut collected: Vec<&str> = Vec::new();
    // Every run is judged; the violations are said together at the end.
    for (wall, o) in &runs {
        if found.judge("open errands", || open_errands_stand(wall, o)) == Some(0) {
            found.note(format!("open errands: {}: none judged", o.who()));
        }
        for i in wall.counted(open) {
            let What::Open(class) = wall.pieces[i].what else {
                continue;
            };
            if bank.collects(class, &o.knobs) != Collects::Always {
                continue;
            }
            let closed: Vec<usize> = wall
                .counted(|w| matches!(w, What::Junk(c) if *c == class))
                .into_iter()
                .filter(|j| o.raw(*j))
                .collect();
            if closed.is_empty() {
                found.note(format!(
                    "closed errands: {}: none stands beside the open one",
                    o.who()
                ));
            }
            for j in closed {
                let shown = o.shown(wall, j);
                if shown != Shown::Form(Form::Shortened) {
                    found.note(format!(
                        "closed errands: {}: the row {} of a closed errand does not shrink: \
                         {shown:?}",
                        o.who(),
                        wall.pieces[j].key
                    ));
                }
            }
            collected.push(wall.name);
        }
    }
    for (wall, _) in &runs {
        if !collected.contains(&wall.name) {
            found.note(format!(
                "closed errands: {}: no run collects the scaffold of its closed errands",
                wall.name
            ));
        }
    }
    found.finish();
}

#[test]
fn an_errand_without_an_answer_leaves_its_outcome_after_the_rebuild() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let ended = |w: &Wall| w.pieces.iter().any(|p| p.errand.is_some());
    let runs = run_walls(&bank, ended, &mut found);
    assert!(
        !runs.is_empty(),
        "the bank holds a wall with an errand that ends without an answer"
    );
    if !declared("policy", "errand_outcome") {
        found.note("outcome: the policy declares no errand_outcome, no run sets it".to_string());
    }
    let mut walls: Vec<&str> = Vec::new();
    for (wall, o) in &runs {
        if !walls.contains(&wall.name) {
            walls.push(wall.name);
        }
        // Judged on the run that sets the knob: the last of the wall.
        let last = wall.runs.last().map(Run::label).unwrap_or_default();
        if o.run != last {
            continue;
        }
        if found.judge("outcome", || outcomes_stand(wall, o)) == Some(0) {
            found.note(format!("outcome: {}: none judged", o.who()));
        }
    }
    assert!(!walls.is_empty(), "a wall ran");
    found.finish();
}
