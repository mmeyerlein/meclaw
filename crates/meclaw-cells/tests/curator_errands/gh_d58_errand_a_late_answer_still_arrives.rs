//! GH #1469 (D58 errand) -- a late answer still arrives.
//!
//! WHY this file exists: "no answer" is what the curator knew at the
//! deadline, not a verdict on the errand. An answer that comes after the
//! outcome row runs in as it does today; nothing is taken back and nothing
//! is dropped. Both stand in the wall in the order they happened: first the
//! outcome, then the answer -- the model reads that the answer came late.

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
fn a_late_answer_still_arrives() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let first = ask(&mut h, e.deadline + Duration::minutes(1), "t2");
    let lines = outcome_lines(&first, &e);
    assert_eq!(
        lines.len(),
        1,
        "past the deadline the outcome is filed: {:?}",
        texts(&first)
    );
    assert!(lines[0].contains(NO_ANSWER), "{lines:?}");
    close(&mut h, &first);

    let answer = "The plumber comes on Friday at nine.";
    answers(&mut h, e.deadline + Duration::minutes(3), &e, answer);
    let call = ask(&mut h, e.deadline + Duration::minutes(4), "t3");
    let lines = outcome_lines(&call, &e);
    assert_eq!(lines.len(), 1, "the outcome stays: {:?}", texts(&call));
    let (outcome, late) = (place(&call, &lines[0]), place(&call, answer));
    assert!(
        late.is_some() && outcome < late,
        "the late answer stands behind the outcome: {outcome:?} {late:?} {:?}",
        texts(&call)
    );
    assert_eq!(outcome_rows(&h, &e).len(), 1, "one outcome row");
}
