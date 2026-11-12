//! GH #1469 (D58 errand) -- the outcome is written once.
//!
//! WHY this file exists: the check runs at every curate of the session, and
//! the parked row of the errand may stand for many of them. One mark per
//! `delegation_id` (`errand_outcome`, its value names the id and the
//! outcome) says the row is filed; the mark lies in the ledger, so a policy
//! that was restarted between two turns files nothing twice. A second row
//! would read as a second failure.

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
use meclaw_core::serde_json::json;

#[test]
fn the_outcome_is_written_once() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    fails(
        &mut h,
        start() + Duration::seconds(20),
        &e,
        "upstream_error",
    );
    let first = ask(&mut h, start() + Duration::minutes(1), "t2");
    assert_eq!(
        outcome_lines(&first, &e).len(),
        1,
        "the first curate after the failure files the outcome: {:?}",
        texts(&first)
    );
    close(&mut h, &first);
    let marks = outcome_marks(&h, &e);
    assert_eq!(marks.len(), 1, "one mark for the errand: {marks:?}");
    assert_eq!(marks[0]["outcome"], json!(FAILED), "{marks:?}");

    // The deadline passes, the policy forgets what it held in memory.
    h.restart("policy");
    for (i, turn_id) in ["t3", "t4"].into_iter().enumerate() {
        let call = ask(
            &mut h,
            e.deadline + Duration::minutes(1 + i as i64),
            turn_id,
        );
        assert_eq!(
            outcome_lines(&call, &e).len(),
            1,
            "{turn_id}: the window shows the one outcome: {:?}",
            texts(&call)
        );
        close(&mut h, &call);
    }
    assert_eq!(outcome_rows(&h, &e).len(), 1, "one outcome row in the wall");
    let marks = outcome_marks(&h, &e);
    assert_eq!(marks.len(), 1, "still one mark for the errand: {marks:?}");
}
