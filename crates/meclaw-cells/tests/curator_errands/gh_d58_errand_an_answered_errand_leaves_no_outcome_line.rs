//! GH #1469 (D58 errand) -- an answered errand leaves no outcome line.
//!
//! WHY this file exists: the outcome row is for an errand that ended without
//! an answer. An errand whose call came back with its answer is told by that
//! answer; a row "no answer" beside it, filed because the deadline passed
//! afterwards, would tell the model the opposite of what happened. The
//! answer has reached the wall when a final assistant row stands under the
//! turn of the errand: that is the reading the deadline is checked against.

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
fn an_answered_errand_leaves_no_outcome_line() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let answer = "The plumber comes on Friday at nine.";
    answers(&mut h, start() + Duration::minutes(1), &e, answer);
    let finals = h.rows(&format!(
        "SELECT seq FROM wall WHERE turn_id = '{}' AND kind = 'assistant' AND final = 1",
        e.inner
    ));
    assert_eq!(
        finals.len(),
        1,
        "the answer stands in the wall under the turn of the errand: {:?}",
        h.stderr
    );

    // Past the deadline: the answer is in the window, no outcome beside it.
    let call = ask(&mut h, e.deadline + Duration::minutes(1), "t2");
    assert!(
        place(&call, answer).is_some(),
        "the answer of the errand stands in the window: {:?}",
        texts(&call)
    );
    assert!(
        outcome_lines(&call, &e).is_empty(),
        "an answered errand has no outcome line: {:?}",
        texts(&call)
    );
    assert_eq!(all_outcome_rows(&h), 0, "no outcome row in the wall");
    assert!(outcome_marks(&h, &e).is_empty(), "no mark of an outcome");
}
