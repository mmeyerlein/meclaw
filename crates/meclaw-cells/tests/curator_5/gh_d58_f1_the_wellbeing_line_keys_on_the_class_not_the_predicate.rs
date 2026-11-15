//! GH #1456 (D58 F1) -- the open wellbeing keys on the class the model set on
//! the fact, never on its predicate.
//! Since D58 followup (GH #1479) against the hive `assistant`.
//!
//! WHY this file exists: the first rule took the day's newest fact only with
//! predicate `health` (or `needs` `unfulfilled`). The sidecar writes
//! predicates as open keys ("PREDICATES ARE KEYS", memory-hive
//! `inline-contract.md`); the payloads measured in the D58 analysis carried
//! `reports_symptom`, `health_status`, `has_symptom`, `feels` -- never
//! `health` -- so the line never opened. A family of predicates in code would
//! be a word list; the model classifies the fact itself instead, with the
//! closed field `wellbeing` (true/false, missing reads as false). The rule now
//! lives in the cell `./followup` of `assistant`, which hears the section
//! `memory`; the statements stay.
//!
//! The fixtures are the sidecar items of real runs of the wave d58-1, the
//! claims and quotes put into English for the export (the originals are
//! German): (a) the telephone call 14-D1 (`reports_symptom`), (b) the text
//! window S8 "slight twinge in the back" (`has_symptom`), (c) the same window,
//! "the neighbour brought tomatoes" (`visited`), (d) fact (b) without the
//! field. (a) and (b) open a concern, (c) and (d) do not; the predicate is
//! free. Of two classed facts of the day the newer one is the open concern.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DIZZY: &str = "Rita has been dizzy since early this morning and is alone at home.";
const TWINGE: &str = "Rita has a slight twinge in her back today.";
const TOMATOES: &str = "Rita's upstairs neighbour brought her tomatoes from her garden today.";

/// (a) Telephone 14-D1, talky sidecar.
fn phone_dizzy() -> Value {
    json!({"subject": "Rita", "predicate": "reports_symptom", "claim": DIZZY,
           "quote": DIZZY, "source": "person", "stance": "asserted",
           "fact_kind": "world", "valid_from": "2026-11-08T12:20:00+01:00",
           "wellbeing": true})
}

/// (b) Text window S8: "a slight twinge in the back today".
fn back_twinge() -> Value {
    json!({"subject": "Rita", "predicate": "has_symptom", "claim": TWINGE,
           "quote": TWINGE, "source": "person", "stance": "asserted",
           "fact_kind": "experience", "valid_from": "2026-11-08", "wellbeing": true})
}

/// (c) Text window S8: the tomatoes -- a fact of the day, not of wellbeing.
fn tomatoes() -> Value {
    json!({"subject": "Rita", "predicate": "visited", "claim": TOMATOES,
           "quote": TOMATOES, "source": "person", "stance": "asserted",
           "fact_kind": "experience", "valid_from": "2026-11-08", "wellbeing": false})
}

/// (d) Fact (b) as a sidecar of before wrote it: no field.
fn back_twinge_unclassed() -> Value {
    let mut v = back_twinge();
    v.as_object_mut().expect("an object").remove("wellbeing");
    v
}

/// The live notes the cell sends into talky after the sections `turns`.
fn notes_after(turns: &[Value]) -> Vec<Value> {
    let mut a = Assistant::on_at(NOON);
    for (i, facts) in turns.iter().enumerate() {
        a.leave(
            "./talky",
            memory_out("phone-1", &format!("t{i}"), json!([facts])),
        );
    }
    // The session ends; a held concern goes now (OR-d58-2.followup.19).
    session_ends(&mut a, "./talky", "phone-1");
    live_notes(&a.notes_into("./talky"))
}

/// Pin: (a) and (b) each open a concern, whatever the predicate.
#[test]
fn a_classed_fact_opens_the_line_whatever_its_predicate() {
    if !shipped() {
        return;
    }
    for (fixture, claim) in [(phone_dizzy(), DIZZY), (back_twinge(), TWINGE)] {
        let notes = notes_after(&[fixture]);
        assert_eq!(notes.len(), 1, "{claim}: {notes:?}");
        assert!(
            notes[0]["text"].as_str().unwrap_or("").contains(claim),
            "{}",
            notes[0]
        );
    }
}

/// Pin: (c) classed false and (d) without the field open nothing.
#[test]
fn an_unclassed_fact_opens_no_line() {
    if !shipped() {
        return;
    }
    for (name, fixture) in [
        ("tomatoes", tomatoes()),
        ("unclassed", back_twinge_unclassed()),
    ] {
        assert!(notes_after(&[fixture]).is_empty(), "{name}");
    }
}

/// Pin: the newest CLASSED fact of the day is the open concern, not the
/// newest fact -- the tomatoes after the twinge leave the twinge open; a
/// newer classed fact takes the place of an older one.
#[test]
fn the_newest_classed_fact_wins_over_a_newer_unclassed_one() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    for (i, f) in [
        phone_dizzy(),
        back_twinge(),
        back_twinge_unclassed(),
        tomatoes(),
    ]
    .into_iter()
    .enumerate()
    {
        a.leave(
            "./talky",
            memory_out("phone-1", &format!("t{i}"), json!([f])),
        );
    }
    session_ends(&mut a, "./talky", "phone-1");
    let mut h = curator(&[]);
    deliver_notes(&a, "./talky", &mut h);
    let s = seed(&mut h, "call-2", "start", 0);
    assert_eq!(count_in(&s, TWINGE), 1, "{:?}", s.messages());
    assert_eq!(
        count_in(&s, DIZZY),
        0,
        "the older concern gave way: {:?}",
        s.messages()
    );
    assert_eq!(count_in(&s, TOMATOES), 0, "{:?}", s.messages());
}
