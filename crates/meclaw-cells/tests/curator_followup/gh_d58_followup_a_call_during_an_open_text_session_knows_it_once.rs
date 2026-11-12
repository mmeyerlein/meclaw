//! D58 followup (GH #1479, contract point 4) -- a call that comes in while the
//! text session that heard the concern is still open knows it in its start
//! window, once; the text session never sees it, not even after it ended.
//!
//! WHY this file exists: the re-check of fix round 1 (I1) found the concern
//! held for EVERY child until the hearing session ended. A text session on
//! `./talky-chat` does not end with the exchange but only
//! after the keeper's idle threshold (`templates/session-keeper/close`,
//! `idle_ms` 7200000 = 2 h as shipped) or in the night, so a call half an
//! hour after "I feel dizzy" opened with nothing -- the old rule ("not for
//! the call that booked it") showed it in the start window of every other
//! call. Orchestrator ruling (d58_o22): the cell sends the note at once to
//! the surface that did NOT hear it and holds it for the hearing one -- and,
//! since builder 4, for the reasoning core `./cogny`, which the hearing
//! session consults -- until its session ends; the first receipt closes the
//! concern and withdraws every other copy, the held ones included (they are
//! then never sent).
//!
//! The hearing child is the surface the section came from, by the key the
//! hive already splits its surfaces by: `context.channel_node` `chat` is
//! `./talky-chat`, anything else `./talky` (edges 0 and 1 of `assistant`).
//!
//! The hold is per surface, not per session (ruling OR-d58-2.followup.29):
//! channels that share a surface -- today Telegram and a call, both on
//! `./talky` -- see a concern heard there only after the hearing session has
//! ended; only the other surface, `./talky-chat`, sees it at once. Holding
//! per session would need the curator to exclude one session from a note,
//! and the curator stays as it is in this wave. So this pin measures the case
//! the hold covers: the text on `./talky-chat`, the call on `./talky`.
//! Clock: the cell and the start windows run on the lab day (`NOON`).

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
fn gh_d58_followup_a_call_during_an_open_text_session_knows_it_once() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    let mut chat = curator(&[]);
    let mut voice = curator(&[]);

    // The text session `s1` of `./talky-chat` hears it, and stays open.
    let section = memory_from_curator_on(
        &mut chat,
        "./talky-chat",
        Some("chat"),
        "s1",
        "t1",
        json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)]),
    );
    a.leave("./talky-chat", section);
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    assert!(
        live_notes(&a.notes_into("./talky-chat")).is_empty(),
        "the hearing text session is not sent its concern: {:?}",
        a.into
    );
    // The reasoning core is held like the hearing surface (ruling d58_o22,
    // builder 4): the hearing session consults it, so it gets the note only
    // when that session ends.
    assert!(
        live_notes(&a.notes_into("./cogny")).is_empty(),
        "the core is held while the hearing session runs: {:?}",
        a.into
    );
    let notes = live_notes(&a.notes_into("./talky"));
    assert_eq!(
        notes.len(),
        1,
        "the surface that did not hear it gets the note at once: {:?} {:?}",
        a.into,
        a.stderr
    );
    assert_eq!(notes[0]["scope"], json!("once"), "{}", notes[0]);
    assert!(
        notes[0]["text"].as_str().unwrap_or("").contains(CLAIM),
        "{}",
        notes[0]
    );
    let id = notes[0]["id"].clone();

    // A call on the voice surface while `s1` is still open: its start
    // window knows it, once, and the receipt closes it.
    assert_eq!(deliver_notes(&a, "./talky", &mut voice), 1);
    let call = seed(&mut voice, "call-b", "start", 0);
    assert_eq!(
        count_in(&call, CLAIM),
        1,
        "the call during the open text session knows it, once: {:?}",
        call.messages()
    );
    a.clear();
    assert_eq!(
        receipts_home(&mut a, "./talky", &voice),
        1,
        "the call's receipt reaches the cell"
    );
    for child in ["./talky-chat", "./cogny"] {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "the receipt sends no note into {child}: {:?}",
            a.into
        );
        let w = withdrawals(&a.notes_into(child));
        assert_eq!(w.len(), 1, "the copy for {child} goes: {:?}", a.into);
        assert_eq!(w[0]["id"], id, "{:?}", w[0]);
    }
    let again = seed(&mut voice, "call-c", "start", 0);
    assert_eq!(
        count_in(&again, CLAIM),
        0,
        "asked after: the next call does not know it again: {:?}",
        again.messages()
    );

    // The text session runs on: whatever reached its curator, a turn of it
    // shows no note and sends no receipt.
    deliver_notes(&a, "./talky-chat", &mut chat);
    let turn = turn_in_on(&mut chat, Some("chat"), "s1", "t2", "and then?");
    assert_eq!(
        count_in(&turn, CLAIM),
        0,
        "the hearing text session is not fed its concern: {:?}",
        texts(&turn)
    );
    assert!(
        chat.routed("note_receipt").is_empty(),
        "the hearing text session sends no receipt: {:?}",
        chat.out
    );

    // The text session ends: the held copy was withdrawn with the receipt
    // and is never sent.
    a.clear();
    session_ends(&mut a, "./talky-chat", "s1");
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    for child in COMPOSITES {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "the end of the text session sends nothing into {child}: {:?}",
            a.into
        );
    }
    assert_eq!(deliver_notes(&a, "./talky-chat", &mut chat), 0);
    let later = seed(&mut chat, "s2", "start", 0);
    assert_eq!(
        count_in(&later, CLAIM),
        0,
        "the text surface never sees it, not after its session ended: {:?}",
        later.messages()
    );
}
