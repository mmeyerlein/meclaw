//! GH #1539 (d58-3 nachlese) -- the transcript a brief is made from names
//! its speakers as the brief prompt does: the person's words as `person:`,
//! the model's own as `agent:self:`. The prose transcript stays as it was
//! (`user:`, `assistant:`).
//!
//! WHY this file exists: the brief prompt asks for `person` / `agent:self`
//! as the speaker of a line, but the transcript under it said `user:` and
//! `assistant:`. In the summary bank of d58-3 one answer (role consult) took
//! the transcript's word and wrote `- user (<day>) reading_circle/room: ...`
//! on every statement -- 22 of 26 lines dropped, 20 planted points gone.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn transcript_of_rebuild(form: &str) -> (Hive, String) {
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!(form)),
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
    let req = h.answer("They talked about the garden.", "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    (h, transcript)
}

/// Pin: a brief transcript says `person:` for the person's words and
/// `agent:self:` for the model's, never `user:` or `assistant:`.
#[test]
fn the_brief_transcript_names_the_speakers_of_the_prompt() {
    if !shipped() {
        return;
    }
    let (h, transcript) = transcript_of_rebuild("brief");
    let q1 = short_id(&user("q1"));
    let a1 = short_id(&said("a1"));
    let lines: Vec<&str> = transcript.lines().collect();
    let of = |id: &str| {
        lines
            .iter()
            .find(|l| l.starts_with(&format!("[#{id}] (")))
            .map(|l| l.split_once(") ").map(|(_, rest)| rest).unwrap_or(""))
            .unwrap_or_else(|| panic!("the row line of {id}: {transcript} {:?}", h.stderr))
    };
    assert_eq!(of(&q1), "person: q1", "{transcript}");
    assert_eq!(of(&a1), "agent:self: a1", "{transcript}");
    for line in &lines {
        let rest = line.split_once(") ").map(|(_, r)| r).unwrap_or(line);
        assert!(
            !rest.starts_with("user") && !rest.starts_with("assistant"),
            "{line:?} in {transcript}"
        );
    }
}

/// Lock: the prose transcript keeps `user:` and `assistant:`.
#[test]
fn the_prose_transcript_stays_as_it_was() {
    if !shipped() {
        return;
    }
    let (_h, transcript) = transcript_of_rebuild("prose");
    let lines: Vec<&str> = transcript.lines().collect();
    assert!(lines.contains(&"user: q1"), "{transcript}");
    assert!(lines.contains(&"assistant: a1"), "{transcript}");
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with("person:") || l.starts_with("agent:self:")),
        "{transcript}"
    );
}
