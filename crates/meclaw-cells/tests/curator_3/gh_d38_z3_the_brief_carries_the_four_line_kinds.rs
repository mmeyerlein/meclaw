//! GH #1440 (D38 Z3, OR-38-42 T1) -- the brief is a handover brief, not a
//! chronicle: beside the statements it carries the fixed line kinds `topic`,
//! `promised`/`open`, `dead_end` and `intent`.
//!
//! WHY this file exists: the review of arXiv 2611.05306 (§ 4 T1) measured
//! that what carries a conversation on is what the person is busy with, what
//! is promised and open, what was tried and refused, and what the companion
//! meant to do next -- not the order in which things were said. So the brief
//! prompt asks for these kinds by name (English names in code; `topic` is the
//! person's concern in the person's own words, `dead_end` is a refused
//! suggestion, an unwanted topic or a failed tool way, dated and never a
//! judgement of the person, `intent` is the companion's next step), the
//! lines of a kind need no `subject/predicate` (`topic` and `intent` hold one
//! current line each), and the code keeps them in the order the model wrote.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

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
fn the_brief_carries_the_four_line_kinds() {
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
    let (q1, a1, q2, a2) = (
        short_id(&user("q1")),
        short_id(&said("a1")),
        short_id(&user("q2")),
        short_id(&said("a2")),
    );
    let brief = format!(
        "- topic (2026-11-05): whether the tomatoes survive the frost [#{q2}]\n\
         - promised (2026-11-05) call/hannah: The companion calls Hannah on Sunday. [#{a1}]\n\
         - open (2026-11-05) recipe/tomato: The tomato recipe is still to be found. [#{q2}]\n\
         - dead_end (2026-11-05) offer/crossword: A crossword was offered and not wanted. [#{a2}]\n\
         - intent (2026-11-05): ask how the tomatoes are doing [#{a2}]\n\
         - person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]"
    );
    let req = h.answer(&brief, "stop");
    let prompt = req.body["system"]["instructions"]["text"]
        .as_str()
        .expect("the summarizer's instructions")
        .to_string();
    for kind in ["topic", "promised", "open", "dead_end", "intent"] {
        assert!(
            prompt.contains(&format!("`{kind}`")),
            "the prompt names the kind {kind:?}: {prompt}"
        );
    }
    assert_eq!(
        stored_text(&h),
        brief,
        "every kind is kept, in the order written, nothing retold"
    );
}

/// Pure half: the parser reads every kind, keyless kind lines included, and
/// takes no line of another shape -- a kind outside the list is no line of
/// the form.
#[test]
fn the_parser_knows_the_kinds_and_nothing_else() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "[[l.get('who'), l.get('key')] for l in brief_lines(ARGS)[0]] + [brief_lines(ARGS)[1]]",
        json!(
            "- topic (2026-11-05): the frost [#0a1b2c3d4e5f]\n\
             - intent (2026-11-05): ask again [#0a1b2c3d4e5f]\n\
             - dead_end (2026-11-05) offer/crossword: Not wanted. [#0a1b2c3d4e5f]\n\
             - peer:anna (2026-11-05) anna/mood: Tired. [hedged] [#0a1b2c3d4e5f]\n\
             - mood (2026-11-05): the frost [#0a1b2c3d4e5f]\n\
             The person was happy."
        ),
    );
    assert_eq!(
        got,
        json!([
            ["topic", "topic"],
            ["intent", "intent"],
            ["dead_end", "offer/crossword"],
            ["peer:anna", "anna/mood"],
            2
        ]),
        "{err}"
    );
}
