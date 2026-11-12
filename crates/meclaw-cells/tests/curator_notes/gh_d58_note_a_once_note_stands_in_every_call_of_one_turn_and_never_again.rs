//! D58 note (GH #1477, contract points 1 and 2) -- a `once` note stands in
//! every call of one turn and never again.
//!
//! WHY this file exists: "once" counts per turn, not per call (plan review
//! W3). A turn has several calls; the brain's later calls take the frozen
//! block of the first (GH #1298: a signed thinking binds every block before
//! it, the person's turn included), so a note that left the window between
//! two calls of one turn would move the prefix mid-turn. The note is used up
//! with the turn's key (`<session>#<turn>`) in `used_turn`, like a `turn`
//! note; the next turn does not see it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "Ask how the doctor's visit went.";

#[test]
fn a_once_note_stands_in_every_call_of_one_turn_and_never_again() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    post(&mut h, json!([once("n1", TEXT, None)]));
    assert_eq!(used(&h, "n1"), "", "stored open: {:?}", h.stderr);

    // The first call of the turn shows it and uses it up with the turn's key.
    let c0 = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    assert!(shows(&c0, TEXT), "{:?}", texts(&c0));
    assert_eq!(used(&h, "n1"), "s1#t2");

    // A tool round of the same turn: the same block, byte for byte.
    h.tap(
        &c0,
        "tool_calls",
        json!({}),
        json!([tool_call("x1", "look")]),
    );
    let c1 = h.curate("s1", "t2", 1, json!([]), mode("Be brief."));
    assert!(shows(&c1, TEXT), "{:?}", texts(&c1));
    let block = |c: &Msg| {
        texts(c)
            .into_iter()
            .find(|t| t.contains(TEXT))
            .and_then(|t| t.split("\n\n").next().map(str::to_string))
    };
    assert_eq!(block(&c0), block(&c1), "the turn's block does not move");
    h.tap(&c1, "stop", json!({}), json!([said("done")]));

    // The next turn, and the one after: never again.
    for t in ["t3", "t4"] {
        let c = h.curate("s1", t, 0, json!([user("e")]), mode("Be brief."));
        assert!(!shows(&c, TEXT), "{t}: {:?}", texts(&c));
        h.tap(&c, "stop", json!({}), json!([said("f")]));
    }
    assert_eq!(used(&h, "n1"), "s1#t2", "used by the turn that showed it");
}
