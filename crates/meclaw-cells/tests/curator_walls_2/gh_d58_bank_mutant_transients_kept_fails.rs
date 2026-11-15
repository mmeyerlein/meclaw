//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: an expired transient block is no longer taken out
//!
//! WHY this file exists: a bank that stays green whatever the curator does
//! proves nothing. The class has no knob, but it has a spot in the curator:
//! `expired`, which says a transient block is past its `until`. The
//! sabotage is made there (`Off::Patched`, d58-3 N13 GH #1524):
//! `./policy` is born with that one place replaced, and the shipped hive runs
//! the wall with the broken program. Until d58-3 the mutant rewrote the
//! outcome of the run instead (`Off::Kept`), which proved the bank reads an
//! outcome, not that it catches a broken curator; `Off::Kept` stays only for
//! a class without such a spot. The bank must turn red for exactly that
//! class: statement 1 names the garbage of the class that is still in the
//! window. Beside it the same run with the shipped program holds nothing
//! against the class, so the red is the patch's and not the wall's; a patch
//! that never reached the cell leaves the outcome as it was and turns this
//! test red. The role is the run's own: every run of the wall on which the
//! class collects is judged. Deterministic: no model anywhere.
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

#[test]
fn mutant_transients_kept_fails() {
    if !shipped() {
        return;
    }
    class_off_turns_the_bank_red(
        "phone_day",
        "transient",
        Off::Patched {
            cell: "policy",
            from: "return until is not None and as_of_ms is not None and until <= as_of_ms",
            to: "return False",
        },
        Red::Window,
    );
}
