//! GH #1517 (d58rest, OR-54g-39) -- with a transcript, a line of a brief
//! names a row of it: a line without `[#<id>]`, or with an id the transcript
//! never showed, is dropped and counted like a line dated with a day the
//! transcript never showed (GH #1496).
//!
//! WHY this file exists: the summary bank of the battery on 0.86.1 found
//! 2 of 24 talky lines without a source pointer (rule: 0). `BRIEF_LINE`
//! ends on an optional `[#<id>]`, so a line whose rest after the stance
//! swallowed the pointer -- `... [asserted] (replaces 4507, ...)` -- went
//! into the brief with `id` "". The dates of a line are checked against the
//! transcript since GH #1496; its id was not checked at all.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`); the pure
//! halves run in the shipped policy's own scope (`policy_scope`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn brief_hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    h
}

/// The day (`YYYY-MM-DD`) of the wall row with the short id `id`.
fn day_of(h: &Hive, id: &str) -> String {
    h.rows("SELECT hash, coalesce(nullif(turn_at, ''), at) FROM wall")
        .into_iter()
        .find(|r| r[0].as_str().is_some_and(|x| x.starts_with(id)))
        .and_then(|r| r[1].as_str().map(|at| at[..10].to_string()))
        .unwrap_or_else(|| panic!("check of the setup: a wall row {id}"))
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

/// Pin: with the transcript, a line without an id and a line whose id is no
/// row of the transcript are dropped and counted; the line naming a row of
/// the transcript stays. The line without an id is the shape the battery
/// found: the rest after the stance (`(replaces ...)`) stands where the
/// pointer belongs.
#[test]
fn a_line_without_a_row_of_the_transcript_is_dropped() {
    if !shipped() {
        return;
    }
    let mut h = brief_hive();
    let q1 = short_id(&user("q1"));
    let day = day_of(&h, &q1);
    let kept =
        format!("- person ({day}) office_form/number: The form number is 12. [asserted] [#{q1}]");
    let no_id = format!(
        "- person ({day}) office_room/number: The office is room 4. [asserted] (replaces 4507)"
    );
    // Twelve hex digits no row of the transcript has.
    let foreign = format!("- open ({day}): the letter to the archive [#0123456789ab]");
    h.answer(&format!("{kept}\n{no_id}\n{foreign}"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "{:?}",
        h.stderr
    );
    assert_eq!(
        stored_text(&h),
        kept,
        "only the line that names a row of the transcript stays"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 2})],
        "both are dropped and counted like any dropped line"
    );
}

/// Pin, pure half: the ids a brief transcript shows are the id of each row
/// line and the id of each line of the brief so far at its top (the model
/// saw both), as `transcript_days` reads their days.
#[test]
fn the_ids_of_a_transcript_are_its_rows_and_the_brief_so_far() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "sorted(transcript_ids(ARGS[0]))",
        json!([
            "The brief so far (keep it; write only new or replacing lines):\n\
             - open (2026-11-01): the recipe [#aaaaaaaaaaaa]\n\
             - person (2026-11-03) office_room/number: The office is room 4. [asserted] \
             [superseded 2026-10-20] [#bbbbbbbbbbbb]\n\
             \n\
             [#0a1b2c3d4e5f] (2026-11-05) user: The form number is 12.\n\
             [#1a2b3c4d5e6f] (2026-11-05) assistant: Noted."
        ]),
    );
    assert_eq!(
        got,
        json!([
            "0a1b2c3d4e5f",
            "1a2b3c4d5e6f",
            "aaaaaaaaaaaa",
            "bbbbbbbbbbbb"
        ]),
        "{err}"
    );
}

/// Lock: without a transcript (a brief read back from the store, as the
/// open lines are read) no id is checked -- a stored line without one stays.
#[test]
fn without_a_transcript_a_line_without_an_id_stays() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "[[len(x[0]), x[1], [l.get('id') for l in x[0]]] for x in [brief_lines(ARGS[0])]]",
        json!(["- open (2026-11-01): the recipe\n\
                - person (2026-11-05) office_form/number: The form number is 12. [asserted]"]),
    );
    assert_eq!(got, json!([[2, 0, ["", ""]]]), "{err}");
}
