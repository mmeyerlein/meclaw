//! D58 note (GH #1477, contract point 3) -- a `once` note left out for want
//! of room stays open.
//!
//! WHY this file exists: today the policy sets `used_turn` for every `turn`
//! note of the pool, shown or left out (plan review "Fenster": the loop runs
//! over `pool`, not over what was shown). A `once` note is used up only when
//! a window showed it; left out, it stays open and is named in the mark
//! `notes` as dropped, turn after turn, and nobody is answered.

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
fn a_once_note_left_out_for_space_stays_open() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    // The first answer reports the model's window: without a known window
    // the producer delivers every note whole (`budget_chars`), nothing is
    // ever left out for room (pattern `gh1303_notes_share_drops_…`).
    turn(&mut h, "s1", "t1", "a", "b", json!({"input_soft": 100000}));
    // A note too long for any share of the window, the last of the order
    // (priority 9), beside a short one that fits (priority 0).
    let long = "Keep this in mind. ".repeat(40_000);
    let mut big = once("big", &long, Some("r-big"));
    big["priority"] = json!(9);
    let mut small = note("small", "until", "Speak slowly.");
    small["priority"] = json!(0);
    post(&mut h, json!([big, small]));
    assert_eq!(used(&h, "big"), "", "stored open: {:?}", h.stderr);

    for t in ["t2", "t3"] {
        h.out.clear();
        let c = h.curate("s1", t, 0, json!([user("c")]), mode("Be brief."));
        h.tap(&c, "stop", json!({}), json!([said("d")]));
        assert!(shows(&c, "Speak slowly."), "{t}: {:?}", texts(&c));
        assert!(!shows(&c, "Keep this in mind."), "{t}: left out");
        let mark = note_marks(&h).last().cloned().expect("a notes mark");
        assert_eq!(
            mark["notes_dropped"],
            json!([{"id": "big", "source": "probe", "scope": "once"}]),
            "{t}: named as dropped: {mark}"
        );
        assert_eq!(used(&h, "big"), "", "{t}: left out is not used up");
        assert!(receipts(&h).is_empty(), "{t}: no receipt: {:?}", h.out);
    }
}
