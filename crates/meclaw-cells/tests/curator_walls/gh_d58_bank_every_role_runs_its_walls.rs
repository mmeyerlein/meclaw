//! GH #1465 (D58, the bank; R-58-7) -- every role runs its walls.
//!
//! WHY this file exists: the curator keeps the window of every model that
//! has one, so the bank is no proof while it runs as one role only. Every
//! wall names the roles it is made for, and a role is a parameter of the
//! run: the same statements are read after the rebuild of every wall with
//! every one of its runs. Since the roles left the curator (D58 target,
//! R-58-15) a run's role is the example value set its target reports as
//! its profile (`support/curator_profiles.rs`): every set of a run is one
//! of the five, and once every seat of the bank holds its wall, every set
//! runs at least one wall.
//!
//! The statements on the runs are one test per wall (`per_wall!`); what
//! holds over all walls is read off the list of the runs and the knobs the
//! policy reports for them, not off the rebuilds (OR-58-102).
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

/// Every statement on every run of the wall `name`; one test per wall
/// (`per_wall!`, OR-58-102). What holds over all walls is read off the list
/// of the runs below (`every_role_runs_its_walls_over_the_bank`).
fn every_role_runs_its_walls(name: &str) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    bank.named(name);
    let runs = run_walls(&bank, |w| w.name == name, &mut found);
    // Every statement on every run; the violations are said together at
    // the end.
    for (wall, o) in &runs {
        if !o.knobs.roles.contains(&o.knobs.role) {
            found.note(format!("role: {}: no example value set", o.who()));
        }
        found.judge("garbage", || garbage_is_out(&bank, wall, o));
        found.judge("beside", || beside_stands(wall, o));
        found.judge("open errands", || open_errands_stand(wall, o));
        found.judge("talk", || talk_is_whole(wall, o));
        found.judge("errors", || errors_stand(wall, o));
        found.judge("aim", || lands_at_aim(o));
        found.judge("marks", || marks_count(&bank, wall, o));
        found.judge("snapshot", || snapshot_shows(wall, o));
    }
    found.finish();
}

per_wall!(every_role_runs_its_walls);

/// What holds over all walls, read off the list of the runs and the knobs
/// the policy reports for each of them -- no wall goes through the curator:
/// every value set of a run is one of the example sets, a wall runs as more
/// than one set or says why not, and once every seat holds its wall every
/// example set runs at least one wall.
#[test]
fn every_role_runs_its_walls_over_the_bank() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    assert!(!bank.walls.is_empty(), "the bank holds a wall");
    let mut ran: Vec<String> = Vec::new();
    let mut known: Vec<String> = Vec::new();
    for wall in bank.walls.iter() {
        for run in &wall.runs {
            let Some(knobs) = found.judge("knobs", || knobs_of(wall, run)) else {
                continue;
            };
            if !knobs.roles.contains(&knobs.role) {
                found.note(format!(
                    "role: {} as {}: no example value set",
                    wall.name,
                    run.label()
                ));
            }
            ran.push(knobs.role.clone());
            known = knobs.roles;
        }
    }
    for wall in bank.walls.iter() {
        let mut roles: Vec<&str> = wall.runs.iter().map(|r| r.role).collect();
        roles.sort_unstable();
        roles.dedup();
        if roles.len() < 2 && wall.one_role.is_empty() {
            found.note(format!(
                "roles: {}: a wall runs as more than one role, or says why not: {roles:?}",
                wall.name
            ));
        }
    }
    if bank.pending.is_empty() {
        for role in &known {
            if !ran.contains(role) {
                found.note(format!(
                    "roles: the example value set {role} runs no wall of the bank"
                ));
            }
        }
    }
    found.finish();
}

/// The walls the tests per wall are made from (`bank_walls!`) are exactly
/// the walls `bank()` registers, in its order: a wall added to the bank
/// and missing from the list, or a name in the list no seat registers,
/// turns this red. No curator run.
#[test]
fn the_wall_list_is_the_bank() {
    let bank = bank();
    let registered: Vec<&str> = bank.walls.iter().map(|w| w.name).collect();
    assert_eq!(
        WALLS.to_vec(),
        registered,
        "the list of `bank_walls!` names every wall of the bank, in its order"
    );
}
