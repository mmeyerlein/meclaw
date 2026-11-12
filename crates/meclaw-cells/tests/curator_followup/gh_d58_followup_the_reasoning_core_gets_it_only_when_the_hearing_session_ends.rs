//! D58 followup (GH #1479, contract point 4) -- the reasoning core `./cogny`
//! gets the concern the way the hearing surface does: only when the session
//! that heard it ends. A withdrawal still reaches it, before and after.
//!
//! WHY this file exists: fix round 2 sent the note at once to every child
//! that did not hear it, `./cogny` included. But `./cogny` is consulted BY
//! the hearing session: when that session asks it before it ends, the
//! core's window shows the open `once` note, its receipt closes the concern,
//! and the call after the session knows nothing -- the gap fix round 2
//! left open in the strand report of the wave.
//! Orchestrator ruling OR-d58-2.followup.27: `./cogny` is held like the
//! hearing surface -- the edge `./followup -> ./cogny` reads `in_note_held`
//! -- while a withdrawal (every `in_note` without `hop.channel_node`) keeps
//! reaching it, so a copy the core holds can always be taken back.
//! Clock: the cell and the start windows run on the lab day (`NOON`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CLAIM: &str = "Ada has a headache since the morning.";

/// The voice session `s1` of `./talky` hears the concern and stays open;
/// returns the cell's state and the note's id as `./talky-chat` got it.
fn heard_on_the_voice(a: &mut Assistant, voice: &mut Hive) -> Value {
    let section = memory_from_curator_on(
        voice,
        "./talky",
        None,
        "s1",
        "t1",
        json!([fact("Ada", "has_symptom", CLAIM, Some(0.9), true)]),
    );
    a.leave("./talky", section);
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    for child in ["./talky", "./cogny"] {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "while the hearing session runs, {child} is not sent the concern: {:?}",
            a.into
        );
    }
    let chat = live_notes(&a.notes_into("./talky-chat"));
    assert_eq!(
        chat.len(),
        1,
        "the surface that did not hear it gets the note at once: {:?} {:?}",
        a.into,
        a.stderr
    );
    chat[0]["id"].clone()
}

/// The start window of a call on `./talky-chat` shows the note and its
/// receipt reaches the cell; `a` holds only what that receipt sent.
fn receipt_from_the_chat(a: &mut Assistant, chat: &mut Hive) {
    let call = seed(chat, "call-b", "start", 0);
    assert_eq!(count_in(&call, CLAIM), 1, "{:?}", call.messages());
    a.clear();
    assert_eq!(receipts_home(a, "./talky-chat", chat), 1);
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
}

#[test]
fn gh_d58_followup_the_reasoning_core_gets_it_only_when_the_hearing_session_ends() {
    if !shipped() {
        return;
    }

    // 1. The hearing session ends first: the core gets the note with the
    //    hearing surface, then the first receipt withdraws its copy.
    let mut a = Assistant::on_at(NOON);
    let mut voice = curator(&[]);
    let mut chat = curator(&[]);
    let id = heard_on_the_voice(&mut a, &mut voice);
    assert_eq!(deliver_notes(&a, "./talky-chat", &mut chat), 1);
    a.clear();
    session_ends(&mut a, "./talky", "s1");
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    for child in ["./talky", "./cogny"] {
        let notes = live_notes(&a.notes_into(child));
        assert_eq!(
            notes.len(),
            1,
            "the end of the hearing session sends the held note into {child}: {:?} {:?}",
            a.into,
            a.stderr
        );
        assert_eq!(notes[0]["id"], id, "one note, one id: {}", notes[0]);
        assert!(
            notes[0]["text"].as_str().unwrap_or("").contains(CLAIM),
            "{}",
            notes[0]
        );
    }
    assert!(
        a.notes_into("./talky-chat").is_empty(),
        "the end sends nothing into the surface that had it: {:?}",
        a.into
    );
    receipt_from_the_chat(&mut a, &mut chat);
    for child in ["./talky", "./cogny"] {
        let w = withdrawals(&a.notes_into(child));
        assert_eq!(w.len(), 1, "the copy in {child} goes: {:?}", a.into);
        assert_eq!(w[0]["id"], id, "{:?}", w[0]);
    }

    // 2. The first receipt comes while the hearing session still runs: the
    //    withdrawal reaches the core, and the end sends the core nothing.
    let mut a = Assistant::on_at(NOON);
    let mut voice = curator(&[]);
    let mut chat = curator(&[]);
    let id = heard_on_the_voice(&mut a, &mut voice);
    assert_eq!(deliver_notes(&a, "./talky-chat", &mut chat), 1);
    receipt_from_the_chat(&mut a, &mut chat);
    for child in ["./talky", "./cogny"] {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "the receipt sends no note into {child}: {:?}",
            a.into
        );
        let w = withdrawals(&a.notes_into(child));
        assert_eq!(
            w.len(),
            1,
            "the withdrawal reaches {child} while the hearing session runs: {:?}",
            a.into
        );
        assert_eq!(w[0]["id"], id, "{:?}", w[0]);
    }
    a.clear();
    session_ends(&mut a, "./talky", "s1");
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    for child in COMPOSITES {
        assert!(
            a.notes_into(child).is_empty(),
            "the end after the receipt sends nothing into {child}: {:?}",
            a.into
        );
    }
}
