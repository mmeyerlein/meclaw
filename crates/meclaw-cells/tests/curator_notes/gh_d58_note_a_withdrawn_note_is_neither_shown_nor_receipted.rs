//! D58 note (GH #1477, contract point 7) -- a withdrawn open note is neither
//! shown nor answered.
//!
//! WHY this file exists: a sender withdraws a note with the same id and text
//! "" (the row ends, `until` = now). A `once` note withdrawn before any window
//! showed it must not stand in the next window and must not answer; the
//! other note of the same message still does both.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

#[test]
fn a_withdrawn_note_is_neither_shown_nor_receipted() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    post(
        &mut h,
        json!([
            once("n1", "Withdrawn before it was shown.", Some("r-1")),
            once("n2", "Still standing.", Some("r-2"))
        ]),
    );
    h.stderr.clear();
    post(&mut h, json!([once("n1", "", Some("r-1"))]));
    assert!(
        !h.stderr.iter().any(|l| l.contains("refused")),
        "a withdrawal is no error: {:?}",
        h.stderr
    );

    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(!shows(&c, "Withdrawn before"), "{:?}", texts(&c));
    assert!(shows(&c, "Still standing."), "{:?}", texts(&c));
    assert!(receipts_of(&h, "n1").is_empty(), "{:?}", h.out);
    assert_eq!(receipts_of(&h, "n2").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(field(&receipts_of(&h, "n2")[0], "receipt"), "r-2");
}
