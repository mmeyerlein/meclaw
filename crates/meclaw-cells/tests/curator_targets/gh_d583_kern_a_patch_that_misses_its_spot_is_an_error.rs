//! D58-3 kern (GH #1524, contract point 5, N13) -- a patch that misses its
//! spot is an error.
//!
//! WHY this file exists: `Off::Patched` replaces one place of a curator
//! script before the birth of the cell. A place that does not stand in the
//! program (the script moved on) would patch nothing, and the mutant would
//! judge the shipped curator; a place that stands twice would patch more than
//! the mutant names. Both are an error of the bank, said with the count,
//! never a silent run. A place that stands once is replaced once.
//!
//! Reads the shipped templates (`support/curator_bank.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_bank::{Run, patch_script};
use curator_hive::{script_of, shipped};

const SPOT: &str = "return until is not None and as_of_ms is not None and until <= as_of_ms";

#[test]
fn gh_d583_kern_a_patch_that_misses_its_spot_is_an_error() {
    if !shipped() {
        return;
    }
    let missing = patch_script("policy", "this text stands in no program", "x");
    assert!(
        missing.as_ref().is_err_and(|e| e.contains("0 times")),
        "a spot the program does not hold is an error: {missing:?}"
    );
    let twice = patch_script("policy", "def ", "x");
    assert!(
        twice.is_err(),
        "a spot the program holds more than once is an error"
    );
    let shipped_script = script_of("policy");
    let once = patch_script("policy", SPOT, "return False").expect("the spot stands once");
    assert!(
        !once.contains(SPOT) && once.contains("return False"),
        "the one spot is replaced"
    );
    assert_eq!(
        once.len() + SPOT.len(),
        shipped_script.len() + "return False".len(),
        "and nothing else moved"
    );
    let run = std::panic::catch_unwind(|| Run::of("x").patched("policy", "no such spot", "x"));
    assert!(
        run.is_err(),
        "a run with a patch that misses its spot does not start"
    );
}
