//! GH #1469 (D58 errand) -- an errand past its deadline leaves its outcome.
//!
//! WHY this file exists: an errand nobody answers ends at its deadline, the
//! `until` of its parked row. Nothing said so: the row fell to the sweep, the
//! `(delegation)` row became the scaffold of a closed errand, and the model
//! went on as if the answer were still to come or had never been promised
//! (bank find F6). With `errand_outcome` "1" the first curate of the session
//! after the deadline files one row that says `no answer`. No timer: the
//! curate that comes anyway reads the clock. Before the deadline the errand
//! is open and nothing is filed.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;

#[test]
fn an_errand_past_its_deadline_leaves_its_outcome() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);

    // Two minutes on the errand is open: the window shows it, nothing more.
    let early = ask(&mut h, start() + Duration::minutes(2), "t2");
    assert!(
        place(&early, &e.text).is_some(),
        "the errand stands in the window: {:?}",
        texts(&early)
    );
    assert!(
        outcome_lines(&early, &e).is_empty() && outcome_rows(&h, &e).is_empty(),
        "an open errand has no outcome: {:?}",
        texts(&early)
    );
    close(&mut h, &early);

    // A minute past the deadline.
    let late = ask(&mut h, e.deadline + Duration::minutes(1), "t3");
    let lines = outcome_lines(&late, &e);
    assert_eq!(
        lines.len(),
        1,
        "one message says the outcome of the errand: {:?} {:?}",
        texts(&late),
        h.stderr
    );
    assert!(
        lines[0].contains(NO_ANSWER) && !lines[0].contains(FAILED),
        "nobody answered: {lines:?}"
    );
    let rows = outcome_rows(&h, &e);
    assert_eq!(rows.len(), 1, "one outcome row in the wall: {rows:?}");
    assert!(lines[0].ends_with(&rows[0]), "{lines:?} {rows:?}");
}
