//! GH #1469 (D58 errand) -- a swept park has its outcome written.
//!
//! WHY this file exists: the parked row of an errand is the only place that
//! knows an errand was handed on and when it was due. The sweep of old
//! parked rows (A13, `pending_sweep_s`) took it ten minutes after it was
//! parked, whether or not anybody had answered -- and with it went the last
//! chance to say "no answer" (bank find F6: the next turn swept the row,
//! mark `pending_swept`). No state goes silently: the sweep takes the row of
//! an errand without an answer only once its outcome is filed, or files it
//! in the same go. Either way the session's next turn shows the outcome, and
//! the row is swept in the end.

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
fn a_swept_park_has_its_outcome_written() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let key = park_key(&e.label);

    // Twenty minutes on, no turn of the session came: a sweep runs beside
    // the rebuild another session's turn ordered.
    strike(&mut h, start() + Duration::minutes(20), "b1");
    let parked = !h.state(&key).is_empty();
    let filed = outcome_rows(&h, &e).len();
    assert!(
        parked || filed == 1,
        "the sweep leaves the parked row of an errand without an answer, or files its outcome \
         in the same go: parked {parked}, outcome rows {filed}, swept {:?}",
        swept_keys(&h)
    );

    let call = ask(&mut h, start() + Duration::minutes(21), "t2");
    let lines = outcome_lines(&call, &e);
    assert_eq!(
        lines.len(),
        1,
        "the next turn of the session shows the outcome: {:?} {:?}",
        texts(&call),
        h.stderr
    );
    assert!(lines[0].contains(NO_ANSWER), "{lines:?}");
    close(&mut h, &call);

    // The outcome is filed: the next sweep takes the row.
    strike(&mut h, start() + Duration::minutes(40), "b2");
    assert!(
        h.state(&key).is_empty() && swept_keys(&h).contains(&key),
        "the parked row is swept once its outcome is filed: {:?} {:?}",
        swept_keys(&h),
        h.stderr
    );
    assert_eq!(outcome_rows(&h, &e).len(), 1, "one outcome row in the wall");
}
