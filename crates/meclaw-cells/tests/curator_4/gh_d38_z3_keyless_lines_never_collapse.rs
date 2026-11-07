//! GH #1440 (D38 Z3, review d38_o3-r1 I1/M5) -- the merge of a brief keeps
//! every line that carries the exchange on.
//!
//! WHY this file exists: the review r1 found that a `promised`, `open` or
//! `dead_end` line without key took the kind as its key, so a second promise
//! replaced the first -- inside one answer as well: two keyless promises came
//! out as one line `[superseded ...]`, and open promises are what carries the
//! exchange on (R-38-2). Only `topic` and `intent` hold one current line. The
//! same review found the key ASCII only: a statement about a name with an
//! umlaut was dropped whole. And a line the model repeated unchanged counted
//! as a replacement (M5) -- noise, no new state. Pinned:
//!
//! 1. two keyless `promised`, `open` and `dead_end` lines stay two each, in
//!    one answer and across two; two keyless `topic` lines still make one;
//! 2. a key in lower case of any script stays the line's key;
//! 3. a line repeated word for word replaces nothing and gets no mark.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn two_keyless_promises_stay_two() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "(lambda L: [len(brief_merge([], L[:6])[0]), brief_merge([], L[:6])[1], \
         len(brief_merge(L[:3], L[3:6])[0]), brief_merge(L[:3], L[3:6])[1], \
         len(brief_merge([], L[6:])[0]), brief_merge([], L[6:])[1]])(brief_lines(ARGS)[0])",
        json!(
            "- promised (2026-11-05): The companion calls Hannah on Sunday. [#0a1b2c3d4e5f]\n\
             - open (2026-11-05): The tomato recipe is still to be found. [#0a1b2c3d4e5f]\n\
             - dead_end (2026-11-05): A crossword was offered and not wanted. [#0a1b2c3d4e5f]\n\
             - promised (2026-11-05): The companion sends the photo. [#0a1b2c3d4e5f]\n\
             - open (2026-11-05): Whether the bus runs on Monday. [#0a1b2c3d4e5f]\n\
             - dead_end (2026-11-05): The weather tool failed. [#0a1b2c3d4e5f]\n\
             - topic (2026-11-05): the frost [#0a1b2c3d4e5f]\n\
             - topic (2026-11-06): the garden [#0a1b2c3d4e5f]"
        ),
    );
    assert_eq!(
        got,
        json!([6, 0, 6, 0, 1, 1]),
        "keyless promises, open points and dead ends are appended, never replaced; \
         a topic holds one line: {err}"
    );
}

#[test]
fn a_key_in_any_script_stays_the_key() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "[[l.get('who'), l.get('key')] for l in brief_lines(ARGS)[0]] + [brief_lines(ARGS)[1]]",
        json!(
            "- person (2026-11-05) j\u{fc}rgen/lives_in: J\u{fc}rgen lives in K\u{f6}ln. [asserted] [#0a1b2c3d4e5f]\n\
             - person (2026-11-05) b\u{e4}rbel/age: B\u{e4}rbel is 81. [#0a1b2c3d4e5f]\n\
             - person (2026-11-05) Hannah/lives_in: Hannah lives in Lisbon. [#0a1b2c3d4e5f]"
        ),
    );
    assert_eq!(
        got,
        json!([
            ["person", "j\u{fc}rgen/lives_in"],
            ["person", "b\u{e4}rbel/age"],
            1
        ]),
        "lower case of any script is a key; upper case is no key of the form: {err}"
    );
}

#[test]
fn a_repeated_line_replaces_nothing() {
    if !shipped() {
        return;
    }
    let line =
        "- person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#0a1b2c3d4e5f]";
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "(lambda L: [brief_merge(L[:2], L[2:])[1], brief_render(brief_merge(L[:2], L[2:])[0])])\
         (brief_lines(ARGS)[0])",
        json!(format!(
            "{line}\n- promised (2026-11-05): The companion calls Hannah on Sunday. [#0a1b2c3d4e5f]\n\
             {line}\n- promised (2026-11-05): The companion calls Hannah on Sunday. [#0a1b2c3d4e5f]"
        )),
    );
    assert_eq!(
        got,
        json!([
            0,
            format!(
                "{line}\n- promised (2026-11-05): The companion calls Hannah on Sunday. [#0a1b2c3d4e5f]"
            )
        ]),
        "a line said again word for word is no replacement: {err}"
    );
}
