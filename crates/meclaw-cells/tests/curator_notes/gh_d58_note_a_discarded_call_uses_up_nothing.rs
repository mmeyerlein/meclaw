//! D58 note (GH #1477, contract point 3) -- a call that is thrown away uses
//! nothing up and answers nobody.
//!
//! WHY this file exists: the spec call's window went to the model at once and
//! showed the note, but the turn is decided without it: the stamp sends the
//! turn away as an errand (a departure shows no notes), the spec row is
//! dropped, and the spec answer that comes later is discarded (`discard`).
//! Nobody heard the note, so nothing is booked; it stays open and stands in
//! the next turn, which answers for it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "Remind them of the appointment at three.";

#[test]
fn a_discarded_call_uses_up_nothing() {
    if !shipped() {
        return;
    }
    let mut h = once_spec_hive();
    post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
    let spec = spec_call(&mut h, "t1", "plan my week");
    assert!(
        shows(&spec, TEXT),
        "the spec window shows it: {:?}",
        texts(&spec)
    );

    // The stamp sends the turn away as an errand: no window shows the note.
    seam(&mut h, "t1", "complex", "plan my week");
    let errands = h.routed("consult");
    assert_eq!(errands.len(), 1, "an errand: {:?} {:?}", h.out, h.stderr);
    assert!(!shows(&errands[0], TEXT), "{:?}", texts(&errands[0]));
    let mut seen = receipts(&h);

    // The spec answer comes late and is discarded.
    spec_end(
        &mut h,
        &spec,
        "stop",
        json!({}),
        json!([said("Here is your week.")]),
    );
    assert!(
        h.routed("spec_release").is_empty(),
        "discarded: {:?}",
        h.out
    );
    seen.extend(receipts(&h));
    assert!(seen.is_empty(), "nothing answered: {seen:?}");
    assert_eq!(used(&h, "n1"), "", "nothing used up");

    // The next turn shows it, uses it up and answers.
    seam(&mut h, "t2", "talk", "and tomorrow?");
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(shows(&calls[0], TEXT), "{:?}", texts(&calls[0]));
    assert_eq!(receipts_of(&h, "n1").len(), 1, "{:?}", h.out);
    assert_eq!(used(&h, "n1"), "s1#t2");
}
