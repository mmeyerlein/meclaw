//! D58 note (GH #1477, contract point 7) -- withdrawing a note that was
//! already used up changes nothing.
//!
//! WHY this file exists: between a receipt and a sender's withdrawal lie
//! several message steps, and a sender that spread one note over several
//! curators withdraws it everywhere (plan review W7). A withdrawal that comes
//! after the note was shown is neither an error nor a second answer, and the
//! note does not come back.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "Ask whether the parcel came.";

#[test]
fn withdrawing_a_used_note_changes_nothing() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(shows(&c, TEXT), "{:?}", texts(&c));
    assert_eq!(receipts_of(&h, "n1").len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.tap(&c, "stop", json!({}), json!([said("d")]));

    // The sender withdraws it after the receipt: no error, no answer.
    h.out.clear();
    h.stderr.clear();
    post(&mut h, json!([once("n1", "", Some("r-1"))]));
    assert!(
        h.stderr.is_empty(),
        "a late withdrawal is no error: {:?}",
        h.stderr
    );
    assert!(receipts(&h).is_empty(), "{:?}", h.out);

    // And the note does not come back, nor does it answer again.
    for t in ["t3", "t4"] {
        let c = h.curate("s1", t, 0, json!([user("e")]), mode("Be brief."));
        assert!(!shows(&c, TEXT), "{t}: {:?}", texts(&c));
        assert!(receipts(&h).is_empty(), "{t}: {:?}", h.out);
        h.tap(&c, "stop", json!({}), json!([said("f")]));
    }
}
