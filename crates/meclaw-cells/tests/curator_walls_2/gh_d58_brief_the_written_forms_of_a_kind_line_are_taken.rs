//! GH #1517 (d58rest, OR-54g-39) -- the parser of a brief takes the two
//! shapes of a kind line the models write that have exactly one reading, and
//! renders them in the canonical form; a statement without a day stays
//! dropped.
//!
//! WHY this file exists: in the summary bank of the battery on 0.86.1 every
//! line the parser rejected (talky 18 of 36, consult 36 of 72) failed at its
//! shape, none at its day -- and these lines carried the planted open points
//! and promises (consult open 2/12, talky promise 4/12, open 6/12):
//! - a kind line without the colon after its day:
//!   `- open (<day>) the letter ... [#<id>]`, `- promised (<day>) agent:self will ...`;
//! - an `open`/`promised` line with a key without `/`:
//!   `- open (<day>) upper_market_list: ... [#<id>]` (`BRIEF_LINE` asks for
//!   `<a>/<b>`). A kind line's key is its kind (`key = who`, as for a line
//!   without key).
//!
//! A statement without its day (`- person office_visit/room: ...`) has no
//! reading: no day is guessed, it stays dropped.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`); the pure
//! halves run in the shipped policy's own scope (`policy_scope`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// `[who, key, text, id, canonical text]` of `brief_line(x)` for each `x`
/// of the args, `None` where it is no line.
const PARSE: &str = "[None if l is None else [l.get('who'), l.get('key'), l.get('text'), \
                     l.get('id'), brief_render([l])] for l in [brief_line(x) for x in ARGS]]";

/// Pin: a kind line without the colon after its day is a line of its kind,
/// the rest is its text, and it is rendered with the colon.
#[test]
fn a_kind_line_without_a_colon_is_taken_and_rendered_canonical() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        PARSE,
        json!([
            "- open (2026-02-27) the letter to the archive is not settled [#8b27f45cb8e6]",
            "- promised (2026-02-28) agent:self will bring up the recipe [#a4db670bc04d]"
        ]),
    );
    assert_eq!(
        got,
        json!([
            [
                "open",
                "open",
                "the letter to the archive is not settled",
                "8b27f45cb8e6",
                "- open (2026-02-27): the letter to the archive is not settled [#8b27f45cb8e6]"
            ],
            [
                "promised",
                "promised",
                "agent:self will bring up the recipe",
                "a4db670bc04d",
                "- promised (2026-02-28): agent:self will bring up the recipe [#a4db670bc04d]"
            ]
        ]),
        "{err}"
    );
}

/// Pin: an `open` or `promised` line whose key has no `/` is a line of its
/// kind (`key = who`); its words after the key stay in its text, it keeps its
/// id, and its canonical text starts `- <kind> (<day>): ` and reads back as
/// the same line.
#[test]
fn an_open_or_promised_key_without_a_slash_is_the_kind() {
    if !shipped() {
        return;
    }
    let lines = json!([
        "- open (2026-03-01) market_list: the list for the market is not done [#4b1b1ddb7e09]",
        "- promised (2026-03-01) call_back: agent:self calls back on Friday [#5c2c2eec8f1a]"
    ]);
    let (got, err) = policy_scope(json!({"summary_form": "brief"}), PARSE, lines.clone());
    let rows = got
        .as_array()
        .unwrap_or_else(|| panic!("a list: {got} {err}"));
    let want = [
        (
            "open",
            "the list for the market is not done",
            "4b1b1ddb7e09",
        ),
        (
            "promised",
            "agent:self calls back on Friday",
            "5c2c2eec8f1a",
        ),
    ];
    assert_eq!(rows.len(), want.len(), "{got} {err}");
    for (row, (kind, words, id)) in rows.iter().zip(want) {
        assert!(row.is_array(), "the line is taken: {row} {err}");
        assert_eq!(row[0], json!(kind), "who: {row}");
        assert_eq!(row[1], json!(kind), "a kind line's key is its kind: {row}");
        let text = row[2].as_str().unwrap_or_default();
        assert!(text.ends_with(words), "its words stay its text: {row}");
        assert_eq!(row[3], json!(id), "it keeps its id: {row}");
        let canon = row[4].as_str().unwrap_or_default();
        assert!(
            canon.starts_with(&format!("- {kind} (2026-03-01): "))
                && canon.ends_with(&format!(" [#{id}]")),
            "rendered in the canonical form: {row}"
        );
        // The canonical text reads back as the same line.
        let (again, err2) = policy_scope(json!({"summary_form": "brief"}), PARSE, json!([canon]));
        assert_eq!(again, json!([row]), "{err2}");
    }
}

/// Lock: no day is guessed -- a statement without its day stays dropped, and
/// so does a statement whose key has no `/` (a statement names who and what,
/// only a kind line's key is its kind).
#[test]
fn a_statement_without_a_day_or_without_its_key_stays_dropped() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        PARSE,
        json!([
            "- person office_visit/room: The meeting is in room 4. [asserted] [#0ee52a995e35]",
            "- person (2026-03-01) market_list: The list is long. [asserted] [#0ee52a995e35]"
        ]),
    );
    assert_eq!(got, json!([null, null]), "{err}");
}

/// Pin, end to end: the answer of the summarizer with the written shapes --
/// two kind lines without the colon, one `open` line with a key without `/`,
/// one statement without its day -- goes into the brief with three lines in
/// the canonical form; the statement is dropped and counted.
#[test]
fn the_brief_keeps_the_written_kind_lines_in_the_canonical_form() {
    if !shipped() {
        return;
    }
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
    let q1 = short_id(&user("q1"));
    let day = h
        .rows("SELECT hash, coalesce(nullif(turn_at, ''), at) FROM wall")
        .into_iter()
        .find(|r| r[0].as_str().is_some_and(|x| x.starts_with(&q1)))
        .and_then(|r| r[1].as_str().map(|at| at[..10].to_string()))
        .expect("check of the setup: the row of q1");
    let answer = [
        format!("- open ({day}) the letter to the archive is not settled [#{q1}]"),
        format!("- promised ({day}) agent:self will bring up the recipe [#{q1}]"),
        format!("- open ({day}) market_list: the list for the market is not done [#{q1}]"),
        format!("- person office_visit/room: The meeting is in room 4. [asserted] [#{q1}]"),
    ]
    .join("\n");
    h.answer(&answer, "stop");
    let rows = h.rows(
        "SELECT b.body FROM summaries s JOIN blocks b ON b.hash = s.hash \
         ORDER BY s.rowid DESC LIMIT 1",
    );
    assert_eq!(rows.len(), 1, "a summary: {:?}", h.stderr);
    let body: Value = meclaw_core::serde_json::from_str(rows[0][0].as_str().expect("a block body"))
        .expect("json");
    let stored = body["text"].as_str().unwrap_or_default().to_string();
    let lines: Vec<&str> = stored.lines().collect();
    assert_eq!(lines.len(), 3, "three lines in the brief: {stored}");
    assert!(
        lines.contains(
            &format!("- open ({day}): the letter to the archive is not settled [#{q1}]").as_str()
        ),
        "{stored}"
    );
    assert!(
        lines.contains(
            &format!("- promised ({day}): agent:self will bring up the recipe [#{q1}]").as_str()
        ),
        "{stored}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with(&format!("- open ({day}): "))
                && l.ends_with(&format!("the list for the market is not done [#{q1}]"))),
        "{stored}"
    );
    let marks: Vec<Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'summary_rejected' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).expect("json"))
        .collect();
    assert_eq!(
        marks,
        vec![json!({"kept": 3, "dropped": 1})],
        "the statement without its day is dropped and counted"
    );
}
