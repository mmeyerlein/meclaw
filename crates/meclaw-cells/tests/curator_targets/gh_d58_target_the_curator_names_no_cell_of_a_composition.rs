//! D58 target (GH #1483, contract point 14, R-58-15) -- the curator names no
//! cell of a composition.
//!
//! WHY this file exists: the curator is omni -- it keeps the window of every
//! model, and the cells, ways and roles of the compositions it serves are
//! their business, not its. A name of a composition in a curator script, in
//! an edge or in the contract of the curator hive is a table in disguise:
//! the next composition would need a change under `templates/curator/`.
//! The lock reads the script of every cell under `templates/curator/` and
//! the edges and the contract of `templates/curator/config.json` and refuses
//! every name of a fixed list as a whole string literal of the code (a
//! comment, a docstring and a part of a longer literal such as `consult_id`
//! do not count) or as a value in an edge or the contract. The list begins
//! with the names of this strand; every later strand of the stage adds its
//! own. There are no exceptions.
//!
//! Reads the shipped tree (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;

#[test]
fn gh_d58_target_the_curator_names_no_cell_of_a_composition() {
    if !shipped() {
        return;
    }
    let sources = curator_sources();
    assert!(
        sources
            .iter()
            .any(|(w, _)| w.ends_with("curator/policy/config.json")),
        "check of the setup: the policy's script is read: {:?}",
        sources.iter().map(|(w, _)| w).collect::<Vec<_>>()
    );
    let hive = read_json(&repo("templates/curator/config.json"));
    let found = lock_findings(
        &LOCKED_NAMES,
        &sources,
        &hive["params"]["graph"]["edges"],
        &hive["params"]["contract"],
    );
    assert!(
        found.is_empty(),
        "{} name(s) of a composition in the curator:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}
