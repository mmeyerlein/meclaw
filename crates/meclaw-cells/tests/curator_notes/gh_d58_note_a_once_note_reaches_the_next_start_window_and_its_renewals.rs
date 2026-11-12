//! D58 note (GH #1477, contract point 4) -- an open `once` note stands in the
//! next start window of its round and in that session's renewals, and
//! answers once.
//!
//! WHY this file exists: `seed_notes` takes only `session` and `until` notes
//! and is pinned byte-equal to its twin in `./policy` (plan review W4).
//! `./history` reads `once` notes through a reader of its own beside it,
//! books the use in the same step as its answer `seed` (it writes there
//! already) and sends the receipt. A renewed start window of the same running
//! session carries the note on and does not answer again; the start window of
//! the next session, and every window after, does not see it. The question
//! for a start window names no target, so the receipt's `target` stays empty.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "The daughter called and will visit on Sunday.";

#[test]
fn a_once_note_reaches_the_next_start_window_and_its_renewals() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
    assert_eq!(used(&h, "n1"), "", "stored open: {:?}", h.stderr);

    // The start window of the call carries it and answers once.
    let s = seed(&mut h, "call-9", "start", 0);
    assert!(
        seed_texts(&s).iter().any(|t| t.contains(TEXT)),
        "{:?}",
        seed_texts(&s)
    );
    let r = receipts(&h);
    assert_eq!(r.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(field(&r[0], "note_id"), "n1");
    assert_eq!(field(&r[0], "receipt"), "r-1");
    assert_eq!(field(&r[0], "target"), "", "a seed names no target");
    assert!(!field(&r[0], "at").is_empty(), "{:?}", r[0].hop);
    assert!(!used(&h, "n1").is_empty(), "used up by the start window");
    assert_eq!(h.emit_violations, Vec::<String>::new());

    // Its renewals carry it on, without a second answer.
    for n in [1, 2] {
        let s = seed(&mut h, "call-9", "renew", n);
        assert!(
            seed_texts(&s).iter().any(|t| t.contains(TEXT)),
            "renewal {n}: {:?}",
            seed_texts(&s)
        );
        assert!(receipts(&h).is_empty(), "renewal {n}: {:?}", h.out);
    }

    // The next call's start window and a turn's window: never again.
    let s = seed(&mut h, "call-10", "start", 0);
    assert!(
        !seed_texts(&s).iter().any(|t| t.contains(TEXT)),
        "{:?}",
        seed_texts(&s)
    );
    assert!(receipts(&h).is_empty(), "{:?}", h.out);
    let c = h.curate("s1", "t1", 0, json!([user("hello")]), mode("Be brief."));
    assert!(!shows(&c, TEXT), "{:?}", texts(&c));
    assert!(receipts(&h).is_empty(), "{:?}", h.out);
}
