//! GH #1463 (D58 form) -- a quote over several lines is compared whole, with
//! or without a note behind its closing mark.
//!
//! WHY this file exists: `recall_in_wall` leaves out a recall block whose
//! every quote stands raw in the wall. The memory hive renders the quote of
//! an answer with paragraphs as a head row with the opening mark only, the
//! further lines without indent, the closing mark on the last one
//! (`render_payload_lines`), and it may hang notes behind that closing mark
//! (`candidate_annotations`: ` (seen: 2)`, ` [2026-11-01 -> open]`, ...).
//! The pin `gh_d58_v5_a_recall_of_a_multiline_answer_is_sent_once` turns
//! green as well when a fix merely skips the head row, or reads no further
//! than the first lines: the one-line quote beside it matches the wall, and
//! the block would go without the answer ever being compared. So this file
//! locks the other side -- a quote of several lines that differs from the
//! wall in a later line stays -- and pins the rendered form with a note: the
//! last line of such a quote does not end on a quote mark.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_000000000formmlq";
const ASKED: &str = "Where could we go this summer?";
const FIRST: &str = "A week at the Baltic Sea would suit you well.";
const ITEM: &str = "- **Places:** Usedom or Ruegen, both easy by train.";
const WHEN: &str = "- **When:** early June, before the school holidays.";
const LAST: &str = "Shall I look up the train times for Friday?";
const OTHER_ITEM: &str = "- **Places:** Sylt or Amrum, both by ferry.";
const OTHER_LAST: &str = "Shall I book the ferry for Saturday?";
/// What the memory hive hangs behind the closing mark of a row it collapsed.
const NOTE: &str = " (seen: 2)";

/// An answer of a paragraph, a list and a closing question.
fn answer_of(item: &str, last: &str) -> String {
    format!("{FIRST}\n\n{item}\n{WHEN}\n\n{last}")
}

/// The answer as the wall books it.
fn answer() -> String {
    answer_of(ITEM, LAST)
}

/// A recall result in the rendered form: the question on one row, `reply`
/// behind a head row that carries the opening mark only, `note` behind the
/// closing mark.
fn recalled(reply: &str, note: &str) -> String {
    format!(
        "WHAT WAS SAID\n  user on 2026-11-08: \"{ASKED}\"\n  assistant on 2026-11-08: \"{reply}\"{note}"
    )
}

/// The request of `t3` as text: the answer of several lines in `t1`, the
/// recall result `result` in the earlier round `t2`.
fn sent(knob: &str, result: &str) -> String {
    let mut h = Hive::with(&[("policy", "recall_in_wall", json!(knob))]);
    turn(&mut h, "s1", "t1", ASKED, &answer(), json!({}));
    let pair = json!([
        user("What did you suggest for the holidays?"),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, result)
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("The Baltic Sea.")]));
    let c = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    sj::to_string(&c.messages()).expect("messages")
}

/// The block stayed: the call and the result, the lines of the wall's answer
/// beside the quote.
fn stays(text: &str) {
    assert!(text.contains(RECALL_ID), "the pair stays: {text}");
    assert_eq!(
        text.matches(FIRST).count(),
        2,
        "the first line stands in the turn and in the quote: {text}"
    );
    assert_eq!(text.matches(ASKED).count(), 2, "{text}");
}

/// Lock: a quote of several lines whose list item is not the wall's stays --
/// the head row and the first line match, a later line does not.
#[test]
fn a_multiline_quote_that_differs_from_the_wall_stays() {
    if !shipped() {
        return;
    }
    let text = sent("1", &recalled(&answer_of(OTHER_ITEM, LAST), ""));
    stays(&text);
    assert_eq!(
        text.matches(OTHER_ITEM).count(),
        1,
        "what only memory holds is still there: {text}"
    );
}

/// Lock: the same when only the last line differs, the one that carries the
/// closing mark.
#[test]
fn a_multiline_quote_whose_last_line_differs_stays() {
    if !shipped() {
        return;
    }
    let text = sent("1", &recalled(&answer_of(ITEM, OTHER_LAST), ""));
    stays(&text);
    assert_eq!(text.matches(OTHER_LAST).count(), 1, "{text}");
}

/// Pin: the rendered form with a note behind the closing mark. The quote is
/// the wall's answer byte for byte, so the pair goes whole.
#[test]
fn a_multiline_quote_with_a_note_behind_the_closing_mark_is_sent_once() {
    if !shipped() {
        return;
    }
    let text = sent("1", &recalled(&answer(), NOTE));
    for line in [FIRST, ITEM, WHEN, LAST, ASKED] {
        assert_eq!(
            text.matches(line).count(),
            1,
            "{line:?} once, as the raw turn: {text}"
        );
    }
    assert!(
        !text.contains(RECALL_ID),
        "the pair goes whole, no orphaned call: {text}"
    );
}

/// Lock: with a note too, a quote that differs from the wall stays.
#[test]
fn a_multiline_quote_with_a_note_that_differs_from_the_wall_stays() {
    if !shipped() {
        return;
    }
    let text = sent("1", &recalled(&answer_of(OTHER_ITEM, LAST), NOTE));
    stays(&text);
    assert_eq!(text.matches(OTHER_ITEM).count(), 1, "{text}");
}

/// Lock: a quote on one row with a note behind it goes, as it did before the
/// rule for several lines (the last quote mark of the row closes it).
#[test]
fn a_one_line_quote_with_a_note_is_sent_once() {
    if !shipped() {
        return;
    }
    let result = format!("WHAT WAS SAID\n  user on 2026-11-08: \"{ASKED}\"{NOTE}");
    let text = sent("1", &result);
    assert_eq!(text.matches(ASKED).count(), 1, "{text}");
    assert!(!text.contains(RECALL_ID), "{text}");
}

/// Check of the setup: off, the quote with its note stands beside the turn.
#[test]
fn off_the_quote_with_a_note_stays() {
    if !shipped() {
        return;
    }
    let text = sent("0", &recalled(&answer(), NOTE));
    for line in [FIRST, ITEM, WHEN, LAST, ASKED] {
        assert_eq!(text.matches(line).count(), 2, "{line:?}: {text}");
    }
    assert!(text.contains(RECALL_ID), "{text}");
}
