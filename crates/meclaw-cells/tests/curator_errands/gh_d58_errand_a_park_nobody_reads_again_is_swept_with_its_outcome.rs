//! GH #1469 (D58 errand, review I1) -- a park nobody reads again is swept
//! with its outcome.
//!
//! WHY this file exists: the outcome of an errand without an answer is filed
//! by a curate that reads the parked row -- a curate of the same session, or
//! of the same running session (its handle `live`, D58 live). A session that
//! never takes another turn, and a running session that has ended, read the row never
//! again. Had the sweep left such a row standing until a curate filed its
//! outcome, it would stand for good: no outcome, and every parked row behind
//! it in key order out of the sweep's read (`PENDING_SWEEP_ROWS`). So the
//! sweep files the outcome itself, in the same go, and takes the row: the
//! outcome stands once in the wall, and the row has left the sweep's read.
//! A row whose errand is still within its deadline gets no outcome; it is
//! swept once the deadline has passed.

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

/// The parked rows the sweep reads: every `pending:` row in key order.
fn sweep_read(h: &Hive) -> Vec<String> {
    h.rows("SELECT key FROM state WHERE key > 'pending:' ORDER BY key")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect()
}

/// After the sweep at `at`: the outcome of `e` stands once in the wall and
/// says `outcome`, one mark names it, and its parked row `key` is gone from
/// the sweep's read and named by a `pending_swept` mark.
fn swept_with_outcome(h: &Hive, e: &Errand, key: &str, outcome: &str) {
    let rows = outcome_rows(h, e);
    assert_eq!(
        rows.len(),
        1,
        "the sweep files the outcome of the errand once: {rows:?} {:?}",
        h.stderr
    );
    assert!(
        rows[0].contains(&e.text) && rows[0].contains(outcome),
        "the outcome row names the errand and says {outcome:?}: {rows:?}"
    );
    assert_eq!(outcome_marks(h, e).len(), 1, "one mark errand_outcome");
    assert!(
        !sweep_read(h).contains(&key.to_string()) && swept_keys(h).contains(&key.to_string()),
        "the parked row has left the sweep's read: read {:?}, swept {:?}",
        sweep_read(h),
        swept_keys(h)
    );
}

#[test]
fn a_session_without_another_turn_has_its_errand_swept_with_its_outcome() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let key = park_key(&e.label);

    // The session never takes another turn; twenty minutes on, a sweep runs
    // beside the rebuild another session's turn ordered.
    strike(&mut h, start() + Duration::minutes(20), "b1");
    swept_with_outcome(&h, &e, &key, NO_ANSWER);

    // A later sweep files nothing more.
    strike(&mut h, start() + Duration::minutes(40), "b2");
    assert_eq!(outcome_rows(&h, &e).len(), 1, "one outcome row in the wall");
    assert_eq!(outcome_marks(&h, &e).len(), 1, "one mark errand_outcome");
}

#[test]
fn a_park_under_a_call_nobody_curates_again_is_swept_with_its_outcome() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on_in(&mut h, start(), "p01", "bbbb2222", 5, Some("live-ended"));
    let key = park_key_in("live-ended", &e.label);
    // The call of the errand fails; the session it was parked under has
    // ended, and no curate carries its handle again.
    fails(&mut h, start() + Duration::minutes(1), &e, "upstream_error");

    strike(&mut h, start() + Duration::minutes(20), "b1");
    swept_with_outcome(&h, &e, &key, FAILED);
    assert!(
        outcome_rows(&h, &e)[0].contains("upstream_error"),
        "the code stands beside the outcome: {:?}",
        outcome_rows(&h, &e)
    );
    // Under a call the outcome stands in the session of its `(delegation)`
    // row, never under the handle of the ended call (`errand_session`).
    assert_eq!(
        outcome_sessions(&h, &e),
        vec![S.to_string()],
        "the outcome stands in the session of the errand"
    );

    strike(&mut h, start() + Duration::minutes(40), "b2");
    assert_eq!(outcome_rows(&h, &e).len(), 1, "one outcome row in the wall");
}

#[test]
fn an_errand_within_its_deadline_is_not_swept_and_files_no_outcome() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "cccc3333", 60);
    let key = park_key(&e.label);

    // Past the age of the sweep, within the deadline: the errand runs on.
    strike(&mut h, start() + Duration::minutes(20), "b1");
    assert!(
        sweep_read(&h).contains(&key) && outcome_rows(&h, &e).is_empty(),
        "an errand within its deadline keeps its row and has no outcome: read {:?}, rows {:?}",
        sweep_read(&h),
        outcome_rows(&h, &e)
    );

    // The deadline has passed: the sweep files the outcome and takes the row.
    strike(&mut h, start() + Duration::minutes(70), "b2");
    swept_with_outcome(&h, &e, &key, NO_ANSWER);
}
