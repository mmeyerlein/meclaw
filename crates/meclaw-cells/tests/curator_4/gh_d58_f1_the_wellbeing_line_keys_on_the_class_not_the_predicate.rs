//! GH #1456 (D58 F1) -- the wellbeing line keys on the class the model set on
//! the fact, never on its predicate.
//!
//! WHY this file exists: `wellbeing_line` (D38 V2) took the day's newest
//! `fact` mark only with predicate `health` (or `needs` `unfulfilled`). The
//! sidecar writes predicates as open keys ("PREDICATES ARE KEYS", memory-hive
//! `inline-contract.md`); the payloads measured in the D58 analysis carried
//! `reports_symptom`, `health_status`, `has_symptom`, `feels` -- never
//! `health` -- so the line never opened. A family of predicates in code would
//! be a word list; the model classifies the fact itself instead, with the
//! closed field `wellbeing` (true/false, missing reads as false).
//!
//! The fixtures are the sidecar items of real runs of this wave, the claims
//! and quotes put into English for the export (the originals are German):
//! (a) the telephone call 14-D1 (`reports_symptom`), (b) the text window S8
//! "slight twinge in the back" (`has_symptom`), (c) the same window,
//! "the neighbour brought tomatoes" (`visited`), (d) fact (b) without the
//! field. (a) and (b) open the line, (c) and (d) do not; the predicate is
//! free. The marks are the intake's with `fact_mark_kind` "1".

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY: &str = "2026-11-08";

/// (a) Telephone 14-D1, talky sidecar: "I have been so dizzy since early
/// this morning, and I am alone at home."
fn phone_dizzy() -> Value {
    json!({"subject": "Rita", "predicate": "reports_symptom",
           "claim": "Rita has been dizzy since early this morning and is alone at home.",
           "source": "person", "stance": "asserted", "fact_kind": "world",
           "valid_from": "2026-11-08T12:20:00+01:00", "wellbeing": true})
}

/// (b) Text window S8: "a slight twinge in the back today".
fn back_twinge() -> Value {
    json!({"subject": "Rita", "predicate": "has_symptom",
           "claim": "Rita has a slight twinge in her back today.",
           "source": "person", "stance": "asserted", "fact_kind": "experience",
           "valid_from": "2026-11-08", "wellbeing": true})
}

/// (c) Text window S8: the tomatoes -- a fact of the day, not of wellbeing.
fn tomatoes() -> Value {
    json!({"subject": "Rita", "predicate": "visited",
           "claim": "Rita's upstairs neighbour brought her tomatoes from her garden today.",
           "source": "person", "stance": "asserted", "fact_kind": "experience",
           "valid_from": "2026-11-08", "wellbeing": false})
}

/// (d) Fact (b) as a sidecar of before wrote it: no field.
fn back_twinge_unclassed() -> Value {
    let mut v = back_twinge();
    v.as_object_mut().expect("an object").remove("wellbeing");
    v
}

fn mark(seq: i64, at: &str, value: Value) -> Value {
    json!({"seq": seq, "kind": "fact", "session_id": "phone-1", "at": at,
           "value": canonical(&value)})
}

fn line(cell: &str, marks: &Value) -> Value {
    let (got, err) = cell_scope(
        cell,
        json!({"wellbeing_line": "1"}),
        "wellbeing_line(ARGS['marks'], ARGS['day'])",
        json!({"marks": marks, "day": DAY}),
    );
    assert!(!err.contains("Traceback"), "{cell}: {err}");
    got
}

/// Pin: (a) and (b) each open the line with the predicate as delivered; the
/// same line in the seed (`./history`) and in the feed (`./policy`).
#[test]
fn a_classed_fact_opens_the_line_whatever_its_predicate() {
    if !shipped() {
        return;
    }
    for (fixture, predicate) in [
        (phone_dizzy(), "reports_symptom"),
        (back_twinge(), "has_symptom"),
    ] {
        let marks = json!([mark(2, "2026-11-08T11:20:00Z", fixture)]);
        let got = line("history", &marks);
        assert_eq!(got["state"], json!("open"), "{predicate}: {got}");
        assert_eq!(got["predicate"], json!(predicate), "{got}");
        assert_eq!(got["fact_seq"], json!(2), "{got}");
        assert_eq!(got, line("policy", &marks), "one rule, both twins");
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
        let marks = json!([mark(2, "2026-11-08T11:20:00Z", fixture)]);
        for cell in ["history", "policy"] {
            assert_eq!(line(cell, &marks), Value::Null, "{cell}: {name}");
        }
    }
}

/// Pin: the newest CLASSED fact of the day carries the line, not the newest
/// fact -- the tomatoes after the twinge leave the twinge open.
#[test]
fn the_newest_classed_fact_wins_over_a_newer_unclassed_one() {
    if !shipped() {
        return;
    }
    let marks = json!([
        mark(5, "2026-11-08T11:40:00Z", tomatoes()),
        mark(4, "2026-11-08T11:35:00Z", back_twinge_unclassed()),
        mark(3, "2026-11-08T11:30:00Z", back_twinge()),
    ]);
    for cell in ["history", "policy"] {
        let got = line(cell, &marks);
        assert_eq!(got["fact_seq"], json!(3), "{cell}: {got}");
        assert_eq!(got["predicate"], json!("has_symptom"), "{cell}: {got}");
    }
}
