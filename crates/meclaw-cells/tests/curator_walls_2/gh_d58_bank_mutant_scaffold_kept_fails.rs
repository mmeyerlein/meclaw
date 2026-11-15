//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: the scaffold of a closed errand is no longer collected
//!
//! WHY this file exists: a bank that stays green whatever the curator does
//! proves nothing. The class is switched off by the knob of the policy that carries it
//! (`gc_scaffold` "0"), set beside the knobs of the run -- the shipped curator runs
//! the wall again, nothing in the tree is changed.
//! The bank must turn red for exactly that class: statement 1
//! names the garbage of the class that is still in the window.
//! Beside it the same run with the class collecting holds nothing against
//! the class, so the red is the sabotage's and not the wall's. The role is
//! the run's own: every run of the wall on which the class collects is
//! judged. Deterministic: no model anywhere.
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

use curator_bank::{Off, Red, class_off_turns_the_bank_red};
use curator_hive::shipped;
use meclaw_core::serde_json::json;

#[test]
fn mutant_scaffold_kept_fails() {
    if !shipped() {
        return;
    }
    class_off_turns_the_bank_red(
        "delegations",
        "scaffold",
        Off::Knob("gc_scaffold", json!("0")),
        Red::Window,
    );
}
