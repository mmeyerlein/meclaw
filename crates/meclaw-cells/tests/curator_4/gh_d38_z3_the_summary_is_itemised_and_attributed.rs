//! GH #1440 (D38 Z3, RK-38-9) -- with `summary_form` "brief" a rebuild's
//! summary is a list of lines, one per statement, each said by whom.
//!
//! WHY this file exists: the prose summary of before told the conversation
//! again in the order it was said and mixed the speakers into one report; a
//! model reading it later took the companion's guess for the person's word
//! (review `meclaw-next/38-kurator/review-arxiv-2611.05306.md` § 4). Under the
//! knob the summarizer is asked for lines of one form,
//! `- <who> (<YYYY-MM-DD>) [<subject>/<predicate>]: <statement> [<stance>] [#<id>]`,
//! the speakers kept apart (`person`, `agent:self`, `peer:<ref>`), and every
//! transcript line it reads carries the short id of its block, so it can say
//! where a line came from. A well-formed answer is kept as it was written.
//!
//! Stub summarizer: the test answers in place of the model
//! (`support/curator_hive.rs` `Hive::answer`), no provider is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn brief_hive() -> Hive {
    Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
    ])
}

/// Three turns; the stamp on the third orders the rebuild that condenses
/// t1 and t2 (t3 stays raw, `keep_recent` 1).
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
fn the_summary_is_itemised_and_attributed() {
    if !shipped() {
        return;
    }
    let mut h = brief_hive();
    first_rebuild(&mut h);
    let (q1, a1, q2) = (
        short_id(&user("q1")),
        short_id(&said("a1")),
        short_id(&user("q2")),
    );
    let brief = format!(
        "- person (2026-11-05) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]\n\
         - agent:self (2026-11-05) offer/visit: The companion offered to plan a visit. [#{a1}]\n\
         - peer:anna (2026-11-05) anna/mood: Anna said she was tired. [hedged] [#{q2}]"
    );
    let req = h.answer(&brief, "stop");

    let prompt = req.body["system"]["instructions"]["text"]
        .as_str()
        .expect("the summarizer's instructions")
        .to_string();
    for kind in ["person", "agent:self", "peer:"] {
        assert!(prompt.contains(kind), "the prompt names {kind:?}: {prompt}");
    }
    assert!(
        !prompt.contains("in the order it was said"),
        "the brief asks for no retelling of the order: {prompt}"
    );
    let lower = prompt.to_lowercase();
    for word in ["conversation", "assistant"] {
        // R-KX-2: any `llm` cell may sit behind the hive.
        assert!(!lower.contains(word), "{word:?} in {prompt:?}");
    }

    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    assert_eq!(
        transcript.lines().next(),
        Some(format!("[#{q1}] user: q1").as_str()),
        "every transcript line carries the short id of its block: {transcript}"
    );
    for line in transcript.lines().filter(|l| !l.is_empty()) {
        assert!(line.starts_with("[#"), "{line:?} in {transcript}");
    }

    assert_eq!(
        h.rows("SELECT COUNT(*) FROM summaries")[0][0],
        json!(1),
        "{:?}",
        h.stderr
    );
    assert_eq!(
        stored_text(&h),
        brief,
        "a well-formed brief is kept line for line, speakers apart"
    );
}
