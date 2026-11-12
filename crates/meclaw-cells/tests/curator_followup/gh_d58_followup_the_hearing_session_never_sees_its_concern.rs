//! D58 followup (GH #1479, contract point 4) -- the session that heard the
//! concern never sees it as a note; the next conversation knows it once.
//!
//! WHY this file exists: the review of the strand (C1) found the cell sending
//! its `once` note right behind the `sidecar` of the turn that carried the
//! fact. `./policy` shows an open `once` note in the next turn of ANY session
//! of the round, so the running session that had just heard it asked again
//! and its receipt closed the concern -- the next conversation knew nothing,
//! which is the opposite of the goal (R039: "the next conversation knows it
//! once") and of the old rule "not for the call that booked it". The curator
//! stays as it is (orchestrator ruling); the rule lives in the domain: the
//! cell holds the concern for the hearing surface while the hearing session
//! runs and sends the note there when that session ends -- on the `write`
//! its curator hands out at every close (OR-d58-2.followup.19); the other
//! surface gets it at once (re-check I1, OR-d58-2.followup.23), the reasoning
//! core `./cogny`, which the hearing session consults, with the hearing
//! surface (OR-d58-2.followup.27).
//!
//! The road: session A of talky hears the fact; while A runs nothing goes to
//! talky's curator or to cogny's (talky-chat gets the note), and a further
//! turn of A shows no note and sends no receipt; A ends (`write` at talky's
//! rim); now one note goes into talky and one into cogny and nothing more
//! into talky-chat; the start
//! window of session B shows it once, its receipt closes the concern and
//! withdraws the other copies; the window after that shows nothing.
//! Clock: the cell and the start windows run on the lab day (`NOON`). The
//! turn of A asserts no note against the curator's own store as well --
//! the curator holds none -- so its verdict never rests on the wall clock
//! the turn reads for `until` (OR-d58-2.followup.20).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const CLAIM: &str = "Ada feels dizzy since the morning.";

#[test]
fn gh_d58_followup_the_hearing_session_never_sees_its_concern() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    let mut talky = curator(&[]);

    // Session A (`s1`) hears it.
    let section = memory_from_curator(
        &mut talky,
        "./talky",
        "t1",
        json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)]),
    );
    a.leave("./talky", section);
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    assert!(
        a.ops.iter().any(|o| o.is_object()),
        "check of the setup: the cell heard the concern and booked it: {:?} {:?}",
        a.ops,
        a.stderr
    );
    for child in ["./talky", "./cogny"] {
        assert!(
            a.notes_into(child).is_empty(),
            "while the hearing session runs nothing goes into {child}: {:?}",
            a.into
        );
    }
    assert_eq!(
        live_notes(&a.notes_into("./talky-chat")).len(),
        1,
        "the surface that did not hear it gets it at once: {:?}",
        a.into
    );
    assert_eq!(deliver_notes(&a, "./talky", &mut talky), 0);
    let held = talky.rows("SELECT text FROM notes");
    assert!(
        held.iter()
            .all(|r| !r[0].as_str().unwrap_or("").contains(CLAIM)),
        "the curator of the hearing session holds no such note: {held:?}"
    );

    // A further turn of A: no note, no receipt.
    let call = turn_in(&mut talky, "s1", "t2", "and then?");
    assert_eq!(
        count_in(&call, CLAIM),
        0,
        "the hearing session is not fed its concern: {:?}",
        texts(&call)
    );
    assert!(
        talky.routed("note_receipt").is_empty(),
        "the hearing session sends no receipt: {:?}",
        talky.out
    );
    talky.out.clear();
    talky.tap(&call, "stop", json!({}), json!([said("I see.")]));
    assert!(
        talky.routed("note_receipt").is_empty(),
        "nor after its answer: {:?}",
        talky.out
    );

    // A ends: now the note goes into talky and cogny; every curator holds
    // it once.
    let early = a.notes_into("./talky-chat").len();
    session_ends(&mut a, "./talky", "s1");
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    assert_eq!(
        early,
        a.notes_into("./talky-chat").len(),
        "the end of A sends nothing more into ./talky-chat: {:?}",
        a.into
    );
    let mut ids = Vec::new();
    for child in COMPOSITES {
        let notes = live_notes(&a.notes_into(child));
        assert_eq!(
            notes.len(),
            1,
            "one note into {child}, all told: {:?} {:?}",
            a.into,
            a.stderr
        );
        assert_eq!(notes[0]["scope"], json!("once"), "{}", notes[0]);
        assert!(
            notes[0]["text"].as_str().unwrap_or("").contains(CLAIM),
            "{}",
            notes[0]
        );
        ids.push(notes[0]["id"].clone());
    }
    assert!(ids.windows(2).all(|w| w[0] == w[1]), "{ids:?}");

    // Session B: its start window knows it once, the receipt closes it.
    assert_eq!(deliver_notes(&a, "./talky", &mut talky), 1);
    let b = seed(&mut talky, "call-b", "start", 0);
    assert_eq!(
        count_in(&b, CLAIM),
        1,
        "the next conversation knows it, once: {:?}",
        b.messages()
    );
    a.clear();
    assert_eq!(
        receipts_home(&mut a, "./talky", &talky),
        1,
        "B's receipt reaches the cell"
    );
    for child in ["./talky-chat", "./cogny"] {
        let w = withdrawals(&a.notes_into(child));
        assert_eq!(w.len(), 1, "the copy in {child} goes: {:?}", a.into);
        assert_eq!(w[0]["id"], ids[0], "{:?}", w[0]);
    }
    let c = seed(&mut talky, "call-c", "start", 0);
    assert_eq!(count_in(&c, CLAIM), 0, "{:?}", c.messages());

    // A second end of A changes nothing: the concern is asked after.
    a.clear();
    session_ends(&mut a, "./talky", "s1");
    for child in COMPOSITES {
        assert!(live_notes(&a.notes_into(child)).is_empty(), "{:?}", a.into);
    }
}
