//! GH #947 -- a file's topics ride its summary. The summarizer is asked for
//! one last line `TAGS: a, b, c`, and `parse_summary` of `file-space/derive`
//! takes every `TAGS:` line out, wherever it stands and in any case, and
//! hands back `(one line, short summary, tags)`; without such a line the tags
//! are `[]`. Every tag passes the directory twin `tags_norm` once: lower case,
//! inner whitespace folded, cut to 32 characters, no empty one, no repeat, at
//! most eight, the first ones winning.
//!
//! Why: the directories above a file count its topics (`dirs.tags`), so a
//! tag must have exactly one stored form, and a `TAGS:` line must never leak
//! into the one line or the short summary a reader sees.
//!
//! Tables over the shipped `script_inline`, loaded via `ast`
//! (`support/file_space_hive.rs` `pure`): no colony, no model, no endpoint.
//! The road through the hive -- the `tags` row beside the summary, the
//! counts in the directories -- is `gh947_aggregates_propagate_in_constant_work.rs`.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// R2b / GH #49: a tree without the cell skips.
fn derive_shipped() -> bool {
    repo("templates/file-space/derive/config.json").is_file()
}

#[test]
fn a_summary_answer_is_one_line_a_short_one_and_its_tags() {
    if !derive_shipped() {
        return;
    }
    let long = "w".repeat(300);
    let got = pure(
        "derive",
        "[parse_summary(t) for t in ARGS]",
        json!([
            "One line.\n\nA short one.",
            "One line.\n\nA short one.\n\nTAGS: Parser, tree , parser",
            "One.\n\nShort.\n\nTAGS: a, b, c, d, e, f, g, h, i, j",
            format!("One.\n\nShort.\n\nTAGS: {}, ok", "x".repeat(40)),
            "One.\nTAGS: mid, text\nShort part.",
            "TAGS: first\nOne.\n\nShort.",
            "One.\n\nShort.\n\ntags:  Two  Words , ,B",
            "One.\n\nShort.\n\nTags: `quoted`, \"q2\".\nTAGS: more",
            "TAGS: only",
            format!("{long}\n\n{}\n\nTAGS: a", "s".repeat(2000)),
        ]),
    );
    assert_eq!(
        got[0],
        json!(["One line.", "A short one.", []]),
        "no TAGS line: no tags"
    );
    assert_eq!(
        got[1],
        json!(["One line.", "A short one.", ["parser", "tree"]]),
        "lower case, trimmed, no repeat; the line is gone from the short one"
    );
    assert_eq!(
        got[2][2],
        json!(["a", "b", "c", "d", "e", "f", "g", "h"]),
        "at most eight, the first ones win"
    );
    assert_eq!(
        got[3][2],
        json!(["x".repeat(32), "ok"]),
        "a tag is cut to 32 characters"
    );
    assert_eq!(
        got[4],
        json!(["One.", "Short part.", ["mid", "text"]]),
        "a TAGS line in the middle is taken out of the short summary"
    );
    assert_eq!(
        got[5],
        json!(["One.", "Short.", ["first"]]),
        "a TAGS line before the one line is no one line"
    );
    assert_eq!(
        got[6][2],
        json!(["two words", "b"]),
        "any case; inner whitespace folded, an empty tag dropped"
    );
    assert_eq!(
        got[7][2],
        json!(["quoted", "q2", "more"]),
        "quotes around a tag go, two TAGS lines add up"
    );
    // A TAGS line alone leaves no line to summarize with: no summary. The
    // derive job then fails its summary (`fail_job`): the file keeps the
    // summary it had, and its directories count it without tags -- rather
    // than a one line made of topic words.
    assert_eq!(got[8], Value::Null, "a TAGS line alone is no summary");
    assert_eq!(got[9][0].as_str().unwrap().chars().count(), 120);
    assert_eq!(got[9][1].as_str().unwrap().chars().count(), 1200);
    assert_eq!(
        got[9][2],
        json!(["a"]),
        "the caps cut the text, not the tags"
    );
}

#[test]
fn a_tag_has_one_stored_form() {
    if !derive_shipped() {
        return;
    }
    // `tags_norm` reads a list or its JSON text (a `summaries` row of level
    // `tags`, a `contrib.tags`); anything else is no tag at all.
    let got = pure(
        "derive",
        "[tags_norm(x) for x in ARGS]",
        json!([
            "[\"A\", \"a\", 3, \"b.\"]",
            "not json",
            {"a": 1},
            ["  x  y ", ""],
            null
        ]),
    );
    assert_eq!(got, json!([["a", "b"], [], [], ["x y"], []]));
    assert_eq!(
        pure("derive", "(TAGS_MAX, TAG_CHARS)", json!(null)),
        json!([8, 32])
    );
}

#[test]
fn the_summarizer_is_asked_for_the_tags() {
    if !derive_shipped() {
        return;
    }
    let p = pure("derive", "SUMMARY_PROMPT", json!(null));
    let p = p.as_str().expect("the prompt");
    assert!(
        p.contains("`TAGS: `") && p.contains("at most 8"),
        "the last line of the answer is the tag line: {p}"
    );
    // The directory prompt asks for one sentence and no tags: a directory's
    // topics are its files' counted ones, never a model's guess.
    let d = pure("derive", "DIR_SUMMARY_PROMPT", json!(null));
    assert!(!d.as_str().unwrap().contains("TAGS"), "{d}");
}
