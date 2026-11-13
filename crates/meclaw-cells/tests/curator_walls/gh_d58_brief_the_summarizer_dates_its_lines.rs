//! GH #1496 (OR-58-179) -- the separate summarizer of the `brief` form reads
//! the day of each transcript line, sees the line kinds `promised` and `open`
//! in the exact form the parser takes, and a line dated with a day the
//! transcript never showed is dropped.
//!
//! WHY this file exists: the summary bank of wave d58-2 dropped every line of
//! one role's brief (0 of 48 points): the prompt asks for `(<YYYY-MM-DD>)`,
//! but the transcript the curator sent carried no day, so the model wrote
//! `(undated)` and the parser dropped all 53 lines. Another model invented a
//! day for every line (`(2024-01-01)`), which the parser let through into the
//! brief and into `[superseded <date>]`. And every `promised` line of one role
//! fell at the parser because the prompt named the kinds only in words
//! (promise 0/54, 23/54 with the form alone).
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`); the pure
//! halves run in the shipped policy's own scope (`policy_scope`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeMap;

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

/// The transcript the summarizer holds, as the text of its request.
fn pending_transcript(h: &Hive) -> String {
    let req = h.summ.front().expect("a summarizer request");
    req.messages()
        .last()
        .and_then(|m| m["text"].as_str())
        .expect("the transcript")
        .to_string()
}

/// The day (`YYYY-MM-DD`, UTC) of every wall row, by its short id: the
/// turn's own time where set, else when it was written (`row_time`).
fn wall_days(h: &Hive) -> BTreeMap<String, String> {
    h.rows("SELECT hash, coalesce(nullif(turn_at, ''), at) FROM wall")
        .into_iter()
        .map(|r| {
            let hash = r[0].as_str().expect("a hash").to_string();
            let at = r[1].as_str().expect("a time").to_string();
            (hash[..12].to_string(), at[..10].to_string())
        })
        .collect()
}

/// Every row line of `transcript` reads `[#<id>] (<day>) `, the day being
/// the UTC day of its wall row; there is at least one.
fn assert_rows_dated(h: &Hive, transcript: &str) {
    let days = wall_days(h);
    let rows: Vec<&str> = transcript.lines().filter(|l| l.starts_with("[#")).collect();
    assert!(
        !rows.is_empty(),
        "the transcript holds row lines: {transcript}"
    );
    for line in rows {
        let id = &line[2..14];
        let day = days
            .get(id)
            .unwrap_or_else(|| panic!("the line names a wall row: {line}"));
        let head = format!("[#{id}] ({day}) ");
        assert!(
            line.starts_with(&head),
            "the line carries the day of its row, {head:?}: {line}"
        );
    }
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

/// Pin 1: every row line of the brief transcript reads `[#<id>] (<day>) `,
/// the day being the UTC day of its wall row.
#[test]
fn a_transcript_line_carries_the_day_of_its_row() {
    if !shipped() {
        return;
    }
    let h = brief_hive();
    assert_rows_dated(&h, &pending_transcript(&h));
}

/// Pin 2: the prompt carries one literal line each for `promised` and
/// `open`, and the parser takes both as lines of their kind.
#[test]
fn the_prompt_examples_for_promised_and_open_pass_the_parser() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "[[[l.get('who'), l.get('key')] for l in brief_lines(x)[0]] + [brief_lines(x)[1]] \
         for x in [[s for s in SUMMARY_PROMPT_BRIEF.splitlines() \
         if s.startswith('- %s (' % k)][0] for k in ARGS]]",
        json!(["promised", "open"]),
    );
    assert_eq!(
        got,
        json!([[["promised", "promised"], 0], [["open", "open"], 0]]),
        "each example line is one line of its kind, none dropped: {err}"
    );
}

/// Pin 3: a line dated with a day the transcript never showed is dropped and
/// counted; the line dated with the transcript's day stays.
#[test]
fn a_line_with_an_invented_date_is_dropped() {
    if !shipped() {
        return;
    }
    let mut h = brief_hive();
    let days = wall_days(&h);
    let q1 = short_id(&user("q1"));
    let day = days.get(&q1).expect("the row of q1").clone();
    let kept =
        format!("- person ({day}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]");
    // A well-formed day no row of the transcript has: 1999-01-01.
    let invented =
        format!("- person (1999-01-01) anna/lives_in: Anna lives in Porto. [asserted] [#{q1}]");
    h.answer(&format!("{kept}\n{invented}"), "stop");
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "{:?}",
        h.stderr
    );
    assert_eq!(
        stored_text(&h),
        kept,
        "only the line with the transcript's day stays"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 1})],
        "the invented date is dropped and counted like any dropped line"
    );
}

/// Pin 3, pure half: the parser checks the date only against the days it is
/// given; without them (a brief read back from the store) it checks none. The
/// days of a transcript are the day of each row line and, of each line of the
/// brief so far, its date and its `[superseded <date>]` (the model saw both).
#[test]
fn the_parser_drops_a_date_outside_the_days_it_is_given() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "[[len(x[0]), x[1]] for x in [brief_lines(ARGS[0], transcript_days(ARGS[1])), \
         brief_lines(ARGS[0])]] + [sorted(transcript_days(ARGS[1]))]",
        json!([
            "- person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [#0a1b2c3d4e5f]\n\
             - open (2026-11-01): the recipe [#0a1b2c3d4e5f]\n\
             - person (2024-01-01) anna/lives_in: Anna lives in Porto. [#0a1b2c3d4e5f]\n\
             - person (2026-10-20) bob/lives_in: Bob lives in Faro. [#0a1b2c3d4e5f]",
            "The brief so far (keep it; write only new or replacing lines):\n\
             - open (2026-11-01): the recipe [#0a1b2c3d4e5f]\n\
             - person (2026-11-03) bob/lives_in: Bob lives in Braga. [asserted] \
             [superseded 2026-10-20] [#0a1b2c3d4e5f]\n\
             \n\
             [#0a1b2c3d4e5f] (2026-11-05) user: Hannah lives in Lisbon."
        ]),
    );
    assert_eq!(
        got,
        json!([
            [3, 1],
            [4, 0],
            ["2026-10-20", "2026-11-01", "2026-11-03", "2026-11-05"]
        ]),
        "{err}"
    );
}

/// Pin 4 (OR-d58-2.brief.2): with `summarizer_self` "1" the role condenses
/// on its own model, with the same brief transcript and the same check of
/// its answer -- the transcript carries the day of each row, and a line
/// dated with a day it never showed is dropped and counted.
#[test]
fn the_role_condensing_itself_reads_the_days_and_drops_an_invented_one() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summarizer_self", json!("1")),
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
    let mut order = last_add(&h);
    order.body["emit_body"]["reason"] = json!("compress");
    h.fire(&order);
    assert!(h.summ.is_empty(), "no request to ./summarizer");
    let calls: Vec<Msg> = h
        .routed("brain")
        .into_iter()
        .filter(|m| m.hop.get("summarize") == Some(&Value::from("1")))
        .collect();
    assert_eq!(calls.len(), 1, "the role condenses: {:?}", h.stderr);
    let call = &calls[0];
    let transcript = call
        .messages()
        .last()
        .and_then(|m| m["text"].as_str())
        .expect("the transcript")
        .to_string();
    assert_rows_dated(&h, &transcript);

    let q1 = short_id(&user("q1"));
    let day = wall_days(&h).get(&q1).expect("the row of q1").clone();
    let kept =
        format!("- person ({day}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]");
    // A well-formed day no row of the transcript has: 1999-01-01.
    let invented =
        format!("- person (1999-01-01) anna/lives_in: Anna lives in Porto. [asserted] [#{q1}]");
    h.lane(
        "in_summary_self",
        json!({}),
        json!({"finish_reason": "stop", "model": "role-model", "summarize": "1",
               "summary_call": call.hop.get("summary_call").cloned().unwrap_or_default(),
               "summary_token": call.hop.get("summary_token").cloned().unwrap_or_default()}),
        json!({"messages": [said(&format!("{kept}\n{invented}"))]}),
    );
    assert_eq!(
        h.rows("SELECT model FROM summaries"),
        vec![vec![json!("role-model")]],
        "the role's answer is the summary: {:?}",
        h.stderr
    );
    assert_eq!(
        stored_text(&h),
        kept,
        "only the line with the transcript's day stays"
    );
    assert_eq!(
        rejected_marks(&h),
        vec![json!({"kept": 1, "dropped": 1})],
        "the invented date is dropped and counted like any dropped line"
    );
}
