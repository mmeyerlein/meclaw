//! GH #1440 (D38 Z3, RK-38-10) -- under `summary_form` "brief" the next
//! summary is the old brief plus the new lines, and a newer line on the same
//! subject and predicate of the same speaker replaces the older one.
//!
//! WHY this file exists: a summary written anew at every rebuild loses what
//! the cheap model did not repeat, and one that keeps both states ("lives in
//! Lisbon", later "moved to Porto") leaves the reader to guess which holds
//! (ACE delta, `review-arxiv-2611.05306.md` § 4). The merge is code, not the
//! model: the old brief is parsed line by line (deterministic, its own line
//! form), kept unchanged, and a new line with the same `(who, subject/
//! predicate)` takes the old one's place, marked `[superseded <old date>]`;
//! never two states side by side. The speaker is part of the key (R-39-1 Q2):
//! the companion's line never corrects a fact the person stated.
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
fn a_newer_line_supersedes_the_older() {
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
    // GH #1496: each line is dated with a day its transcript shows, and the
    // days come from the rows. The rows of the first rebuild (t1, t2) are a
    // day older than the one of the second (t3), so `[superseded <date>]`
    // tells the older line's date from the newer one's.
    let yesterday = days_ago(1, 12).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    for t in ["t1", "t2"] {
        h.db.execute(
            "UPDATE wall SET turn_at = ?1 WHERE turn_id = ?2",
            [yesterday.clone(), t.to_string()],
        )
        .expect("the turn time of an earlier turn");
    }
    h.fire(&last_add(&h));
    let (q1, q2) = (short_id(&user("q1")), short_id(&user("q2")));
    let d1 = h.transcript_day();
    let lisbon =
        format!("- person ({d1}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]");
    let tomatoes = format!(
        "- person ({d1}) tomatoes/planted: The person planted tomatoes. [asserted] [#{q2}]"
    );
    h.answer(&format!("{lisbon}\n{tomatoes}"), "stop");

    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let (q3, a3) = (short_id(&user("q3")), short_id(&said("a3")));
    let d2 = h.transcript_day();
    assert_ne!(
        d1, d2,
        "the two rebuilds read rows of two days, so the marked date is the older line's"
    );
    let req = h.answer(
        &format!(
            "- person ({d2}) hannah/lives_in: Hannah moved to Porto. [asserted] [#{q3}]\n\
             - agent:self ({d2}) tomatoes/planted: The companion doubted the tomatoes were planted. [#{a3}]"
        ),
        "stop",
    );
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    assert!(
        transcript.contains("Hannah lives in Lisbon."),
        "the model sees the brief so far, so it can write the newer line: {transcript}"
    );
    assert!(
        !transcript.contains("Summary of the exchange before this part"),
        "the brief is not handed as a summary to retell: {transcript}"
    );

    let text = stored_text(&h);
    assert_eq!(
        text,
        format!(
            "- person ({d2}) hannah/lives_in: Hannah moved to Porto. [asserted] \
             [superseded {d1}] [#{q3}]\n\
             {tomatoes}\n\
             - agent:self ({d2}) tomatoes/planted: The companion doubted the tomatoes \
             were planted. [#{a3}]"
        ),
        "the newer line takes the older one's place, marked; the companion's line \
         corrects no fact of the person"
    );
    assert!(!text.contains("Lisbon"), "never two states side by side");
}
