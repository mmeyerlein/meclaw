//! GH #1440 (D38 Z3, RK-38-9) -- under `summary_form` "brief" an answer
//! without a line of the form is no summary: it is dropped, said, and the old
//! summary stays.
//!
//! WHY this file exists: a cheap summarizer that falls back to prose would
//! replace a brief of kept lines with a retelling, and the next rebuild would
//! merge into nothing (the delta reads the old summary line by line). The
//! form is checked in code: no line of the form -> the answer is dropped, the
//! window stays what it was (`old_window_kept`, OR-KX-K6) and a `marks` row
//! `summary_rejected` says how many lines were kept and dropped. An answer
//! with some lines of the form keeps those and says the rest
//! (R-IG-1: nothing leaves unsaid). The prose form ("prose", the default)
//! takes prose as before.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn hive(form: &str) -> Hive {
    Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!(form)),
    ])
}

fn first_rebuild(h: &mut Hive) {
    turn(h, "s1", "t1", "q1", "a1", json!({}));
    turn(h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(h));
}

fn second_rebuild(h: &mut Hive) {
    turn(
        h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(h));
}

fn summary_slot_hash(h: &Hive) -> Value {
    h.rows(&format!(
        "SELECT hash FROM slots WHERE path = '{}'",
        summary_slot(ROUND_E)
    ))
    .first()
    .map(|r| r[0].clone())
    .unwrap_or(Value::Null)
}

fn rejected_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'summary_rejected' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).expect("json"))
        .collect()
}

fn stored_text(h: &Hive) -> String {
    let rows = h.rows(
        "SELECT b.body FROM summaries s JOIN blocks b ON b.hash = s.hash \
         ORDER BY s.rowid DESC LIMIT 1",
    );
    let body: Value = meclaw_core::serde_json::from_str(rows[0][0].as_str().expect("a block body"))
        .expect("json");
    body["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn prose_is_rejected_and_the_old_summary_stays() {
    if !shipped() {
        return;
    }
    let mut h = hive("brief");
    first_rebuild(&mut h);
    let q1 = short_id(&user("q1"));
    h.answer(
        &format!(
            "- person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]"
        ),
        "stop",
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "{:?}",
        h.stderr
    );
    let slot = summary_slot_hash(&h);
    assert!(!slot.is_null(), "the round shows the first summary");

    second_rebuild(&mut h);
    h.answer("They talked about the garden and the weather.", "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "prose is no brief: no second summary"
    );
    assert_eq!(
        summary_slot_hash(&h),
        slot,
        "the window keeps the old summary"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 0, "dropped": 1})],
        "the rejection is said once, with its count"
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "and the rebuild is over"
    );
}

#[test]
fn a_mixed_answer_keeps_its_lines_and_says_the_rest() {
    if !shipped() {
        return;
    }
    let mut h = hive("brief");
    first_rebuild(&mut h);
    let q1 = short_id(&user("q1"));
    let line = format!(
        "- person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]"
    );
    h.answer(&format!("Here is the brief:\n{line}"), "stop");
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(1));
    assert_eq!(stored_text(&h), line, "only the line of the form is kept");
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 1})],
        "the dropped line is said (R-IG-1)"
    );
}

#[test]
fn the_prose_form_takes_prose_as_before() {
    if !shipped() {
        return;
    }
    let mut h = hive("prose");
    first_rebuild(&mut h);
    h.answer("They talked about the garden.", "stop");
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(1));
    assert_eq!(stored_text(&h), "They talked about the garden.");
    assert!(rejected_marks(&h).is_empty(), "nothing is checked");
}
