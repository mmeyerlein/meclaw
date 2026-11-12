//! D58 followup (GH #1479) -- the same words in a new session replace a hold
//! whose session never reported its end.
//!
//! WHY this file exists: the re-check of fix round 1 (M6) found a hold that
//! no `write` releases -- the curator's close sends none when the store
//! refused the read twice -- muting the concern for the rest of the day when
//! the person says it again in a later session: the cell compared the claim
//! only and did nothing, while the README promised that the next concern of
//! the day replaces the held one. Now the same claim, held for ANOTHER
//! session, is replaced like a different one (OR-d58-2.followup.24): the
//! copies already out are withdrawn everywhere, the successor goes at once
//! to the surface that did not hear it and to the new hearing surface and
//! the reasoning core when the new session ends (OR-d58-2.followup.27:
//! the core is held like the hearing surface). The same claim in the same
//! session still does nothing.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const CLAIM: &str = "Ada has a headache.";

#[test]
fn gh_d58_followup_the_same_words_in_a_new_session_replace_a_hanging_hold() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    let facts = || json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)]);

    // Session s1 of talky hears it; its end never reaches the cell.
    a.leave("./talky", memory_out("s1", "t1", facts()));
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    let first = live_notes(&a.notes_into("./talky-chat"));
    assert_eq!(first.len(), 1, "check of the setup: {:?}", a.into);
    let old_id = first[0]["id"].clone();

    // The same words again in s1: nothing.
    a.clear();
    a.leave("./talky", memory_out("s1", "t2", facts()));
    assert!(
        a.into.is_empty(),
        "the same words in the same session change nothing: {:?}",
        a.into
    );

    // The same words in the new session s2: the hold is replaced.
    a.clear();
    a.leave("./talky", memory_out("s2", "t1", facts()));
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    for child in COMPOSITES {
        let w = withdrawals(&a.notes_into(child));
        assert_eq!(
            w.len(),
            1,
            "the old copy is withdrawn in {child}: {:?} {:?}",
            a.into,
            a.stderr
        );
        assert_eq!(w[0]["id"], old_id, "{:?}", w[0]);
    }
    for child in ["./talky", "./cogny"] {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "{child} gets no note while s2 runs: {:?}",
            a.into
        );
    }
    let n = live_notes(&a.notes_into("./talky-chat"));
    assert_eq!(
        n.len(),
        1,
        "the successor goes into ./talky-chat: {:?}",
        a.into
    );
    assert!(
        n[0]["text"].as_str().unwrap_or("").contains(CLAIM),
        "{}",
        n[0]
    );
    let new_id = n[0]["id"].clone();
    assert_ne!(new_id, old_id, "a new id: {new_id:?}");

    // s1's late end releases nothing; s2's end releases the successor.
    a.clear();
    session_ends(&mut a, "./talky", "s1");
    assert!(a.into.is_empty(), "s1 holds nothing now: {:?}", a.into);
    session_ends(&mut a, "./talky", "s2");
    for child in ["./talky", "./cogny"] {
        let n = live_notes(&a.notes_into(child));
        assert_eq!(
            n.len(),
            1,
            "the end of s2 sends it into {child}: {:?}",
            a.into
        );
        assert_eq!(n[0]["id"], new_id, "{}", n[0]);
    }
    assert!(a.notes_into("./talky-chat").is_empty(), "{:?}", a.into);
}
