//! D58 followup (GH #1479, contract point 4) -- a concern heard without a
//! session goes into every child at once, the reasoning core `./cogny`
//! included.
//!
//! WHY this file exists: since `./cogny` is held like the hearing surface
//! (ruling OR-d58-2.followup.27), the edge `./followup -> ./cogny` on
//! `in_note_held` brings it a held note, and a second edge (143 of
//! `assistant`, `in_note` without `hop.channel_node`) brings it every note
//! sent to all -- a withdrawal, and a concern heard without a session, which
//! nothing holds because no session can end for it. The re-check of builder
//! 4 (M10) found the withdrawal pinned and the concern without a session
//! not: without edge 143 such a concern would reach both surfaces and never
//! the core. This pin is red with that edge gone.
//! Clock: the cell runs on the lab day (`NOON`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;

const CLAIM: &str = "Ada feels weak since the morning.";

#[test]
fn gh_d58_followup_a_concern_heard_without_a_session_reaches_the_core_at_once() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    // The section `memory` of a turn that names no session: no
    // `context.session_id`, no `hop.session_id`.
    a.leave(
        "./talky",
        memory_out(
            "",
            "t1",
            meclaw_core::serde_json::json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)]),
        ),
    );
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    let mut ids = Vec::new();
    for child in ["./talky", "./talky-chat", "./cogny"] {
        let notes = live_notes(&a.notes_into(child));
        assert_eq!(
            notes.len(),
            1,
            "a concern heard without a session reaches {child} at once: {:?} {:?}",
            a.into,
            a.stderr
        );
        assert!(
            notes[0]["text"].as_str().unwrap_or("").contains(CLAIM),
            "{child}: {}",
            notes[0]
        );
        ids.push(notes[0]["id"].clone());
    }
    assert!(
        ids.iter().all(|i| *i == ids[0]),
        "one note, the same id in every child: {ids:?}"
    );
    assert!(
        a.into.iter().all(|(_, m)| m.route() != "in_note_held"),
        "nothing is held without a session: {:?}",
        a.into
    );
}
