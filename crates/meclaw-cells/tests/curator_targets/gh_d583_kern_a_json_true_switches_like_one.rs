//! D58-3 kern (GH #1524, contract point 4, N19) -- a switch reads the same
//! in every curator cell: JSON true is on, like "1".
//!
//! WHY this file exists: a set writes its switches as JSON true (soul2me
//! 0.45), and a script that reads a switch as `str(v) == "1"` sees "True" and
//! keeps it off without a word (#1370). `./policy` has the one reader for
//! that (`switch_on`: true, "1", "true", "on"), but a handful of its own
//! switches, and those of `./history` and `./intake`, still compared with
//! "1". Every switch of the three cells named here reads through
//! `switch_on` (history and intake hold a copy: scripts share no code). The
//! shipped value of each stays off, so the cells are the cells of before,
//! byte for byte. One switch per cell is followed to its effect as well: the
//! intake's `fact_mark_kind`.
//!
//! Reads the pure half of the shipped cells (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// `(cell, param, the name the script reads it under)`.
const SWITCHES: [(&str, &str, &str); 11] = [
    ("policy", "topic_aging", "TOPIC_AGING"),
    ("policy", "note_once", "NOTE_ONCE"),
    ("policy", "notes_latest_per_item", "NOTES_PER_ITEM"),
    ("policy", "state_one_per_subject", "STATE_ONE_PER_SUBJECT"),
    (
        "policy",
        "state_scheduled_on_need",
        "STATE_SCHEDULED_ON_NEED",
    ),
    ("history", "topic_aging", "TOPIC_AGING"),
    ("history", "notes_latest_per_item", "NOTES_PER_ITEM"),
    ("history", "note_once", "NOTE_ONCE"),
    ("history", "seed_anchor_first", "SEED_ANCHOR_FIRST"),
    ("intake", "fact_mark_kind", "FACT_MARK_KIND"),
    ("intake", "note_once", "NOTE_ONCE"),
];

/// What the cell's script holds for `name` with `param` set to `value`
/// (`None`: the shipped value).
fn read(cell: &str, param: &str, name: &str, value: Option<Value>) -> Value {
    let over = match value {
        Some(v) => json!({ param: v }),
        None => json!({}),
    };
    let (got, err) = cell_scope(cell, over, name, Value::Null);
    assert!(!err.contains("Traceback"), "{cell}.{param}: {err}");
    got
}

/// The intake's `fact` mark of a classed item with `fact_mark_kind` at `knob`.
fn fact_mark(knob: Value) -> Value {
    let item = json!({"subject": "Rita", "predicate": "has_symptom",
                      "claim": "Rita has a slight twinge in her back today.",
                      "source": "person", "stance": "asserted",
                      "fact_kind": "experience", "valid_from": "2026-11-08",
                      "verified": ["claim"], "wellbeing": true});
    let (got, err) = cell_scope(
        "intake",
        json!({"fact_mark_kind": knob,
               "sections": {"memory": {"lane": "sidecar", "mark": "topic",
                                       "classes": ["wellbeing"]}}}),
        "fact_marks('memory', {'items': [ARGS]}, 'items')",
        item,
    );
    assert!(!err.contains("Traceback"), "{err}");
    let raw = got[0][1]
        .as_str()
        .unwrap_or_else(|| panic!("a mark: {got} {err}"))
        .to_string();
    meclaw_core::serde_json::from_str(&raw).expect("the mark value is JSON")
}

#[test]
fn gh_d583_kern_a_json_true_switches_like_one() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    for (cell, param, name) in SWITCHES {
        for on in [json!(true), json!("1"), json!("true"), json!("on")] {
            let got = read(cell, param, name, Some(on.clone()));
            if got != json!(true) {
                failures.push(format!("{cell}.{param} = {on} reads {got}, not on"));
            }
        }
        for off in [json!(false), json!("0"), json!("")] {
            let got = read(cell, param, name, Some(off.clone()));
            if got != json!(false) {
                failures.push(format!("{cell}.{param} = {off} reads {got}, not off"));
            }
        }
        let shipped = read(cell, param, name, None);
        if shipped != json!(false) {
            failures.push(format!("{cell}.{param} shipped reads {shipped}, not off"));
        }
    }
    // The effect of one of them: JSON true keeps the class on the mark, as "1".
    let one = fact_mark(json!("1"));
    let truth = fact_mark(json!(true));
    if truth != one || truth["wellbeing"] != json!(true) {
        failures.push(format!(
            "intake.fact_mark_kind = true marks {truth}, \"1\" marks {one}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
