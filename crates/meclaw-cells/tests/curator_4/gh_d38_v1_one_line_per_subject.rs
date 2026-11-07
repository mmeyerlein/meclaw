//! GH #1451 (D38 V1, T-2) -- the state block names one line per subject.
//!
//! WHY this file exists: in the R039 acceptance the state block of every turn
//! from 09:01 on read `set today: ... will call his mother tonight | ... will
//! call his mother at 8 pm | ... wants a reminder at 6 pm | ...` -- one errand
//! four times, in variants, without a state. `set_today` lists every `fact`
//! mark of the day; nothing replaces an older line of the same subject. With
//! `state_one_per_subject` "1" a line of the same `(subject, predicate)` as a
//! younger one leaves the block; a line with an empty key stays as before.
//! Off ("0", shipped), the block of before, byte for byte.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); the marks are
//! sown into the ledger the way `./intake` writes them (`fact_marks`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const MOVE: &str = "Elias wants to move to Hamburg.";
const OLD: &str = "Ada will call his mother tonight.";
const NEW: &str = "Ada will call his mother at 8 pm.";

fn hive(knob: Option<&str>) -> Hive {
    let mut over = vec![
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
    ];
    if let Some(k) = knob {
        over.push(("policy", "state_one_per_subject", json!(k)));
    }
    Hive::with(&over)
}

fn fact(h: &mut Hive, back_s: i64, subject: &str, predicate: &str, claim: &str) {
    h.mark(
        backdated(back_s),
        "s1",
        "t1",
        "fact",
        &canonical(
            &json!({"subject": subject, "predicate": predicate, "claim": claim,
                           "source": "person", "stance": "asserted"}),
        ),
    );
}

fn state_line(c: &Msg) -> String {
    texts(c)
        .last()
        .cloned()
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string()
}

fn sown(knob: Option<&str>, subject: &str, predicate: &str) -> String {
    clear_of_midnight();
    let mut h = hive(knob);
    turn(
        &mut h,
        "s1",
        "t1",
        "Elias is moving.",
        "Oh, where to?",
        json!({}),
    );
    fact(&mut h, 30, "Elias", "moves_to", MOVE);
    fact(&mut h, 20, subject, predicate, OLD);
    fact(&mut h, 10, subject, predicate, NEW);
    let c = h.curate("s1", "t2", 0, json!([user("And?")]), mode("Be brief."));
    state_line(&c)
}

/// Pin: two lines of one `(subject, predicate)` -- only the younger stays.
#[test]
fn a_younger_line_of_the_same_subject_replaces_the_older() {
    if !shipped() {
        return;
    }
    let line = sown(Some("1"), "Ada", "calls");
    assert!(line.contains(NEW), "the younger line stays: {line}");
    assert!(
        !line.contains(OLD),
        "the older line of the subject goes: {line}"
    );
    assert!(
        line.contains(MOVE),
        "another subject keeps its line: {line}"
    );
}

/// Lock: an empty key never collapses -- lines without a subject stay.
#[test]
fn lines_without_a_key_stay_as_before() {
    if !shipped() {
        return;
    }
    let line = sown(Some("1"), "", "");
    assert!(line.contains(OLD) && line.contains(NEW), "{line}");
}

/// Lock: shipped off, the block of before -- every line of the day.
#[test]
fn off_the_block_lists_every_line_of_the_day() {
    if !shipped() {
        return;
    }
    let line = sown(None, "Ada", "calls");
    assert!(
        line.contains(OLD) && line.contains(NEW) && line.contains(MOVE),
        "{line}"
    );
}
