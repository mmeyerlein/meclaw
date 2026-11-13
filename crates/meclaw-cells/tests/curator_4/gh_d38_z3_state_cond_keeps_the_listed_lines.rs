//! GH #1440 (D38 Z3, OR-38-42 "state as a condition") -- with
//! `summary_state_cond` "1" the summarizer is handed the state of the round
//! as a list that must survive the summary, and the brief keeps the lines
//! it lists.
//!
//! WHY this file exists: a cheap summarizer drops what the transcript part
//! it condenses no longer mentions -- an open promise from three days ago, a
//! delegation still running, a fact the person set today. Z1 renders that
//! state deterministically (`state_block`, GH #1439); Z3 hands the same line
//! to the summarizer behind the transcript, without the running topic (the
//! topic moves, it is no "must stay"), parts `promised`, `open`,
//! `set_today` only. And the delta merge never drops an old line the model
//! did not repeat, so a listed promise already in the brief stays.
//!
//! Depends on Z1's `state_block`/`state_of_rows` (curator/policy). Stub
//! summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const TOPIC: &str = "the orchard";
const PROMISE: &str = "Call Hannah on Sunday.";
const FACT: &str = "Hannah lives in Porto.";

fn sow(h: &mut Hive) {
    let at = backdated(5);
    h.mark(
        at,
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": TOPIC})),
    );
    h.mark(
        at + chrono::Duration::microseconds(1),
        "s1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Hannah", "predicate": "lives_in",
                           "claim": FACT, "source": "person", "stance": "asserted"})),
    );
    let until = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp_millis();
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![
            "pending:delegation:s1#t1",
            canonical(&json!({"delegation_id": "d-1",
                              "round": format!("t1~{until}~0000abcd"),
                              "until": until,
                              "parked_at": at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()}))
        ],
    )
    .expect("a state row");
    h.lane(
        "in_pin",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"pins": [{"text": PROMISE, "source": "commitment"}]}),
    );
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
fn state_cond_keeps_the_listed_lines() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        // Z1's cap (OR-d38-2.z3.4); the default role's preset is 0.
        ("policy", "state_lines", json!(8)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    sow(&mut h);
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
    let a1 = short_id(&said("a1"));
    // GH #1496: each line is dated with a day its transcript shows.
    let day = h.transcript_day();
    let kept = format!("- promised ({day}) call/hannah: {PROMISE} [#{a1}]");
    let req = h.answer(&kept, "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    let line = transcript
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or_else(|| panic!("the state line rides behind the transcript: {transcript}"))
        .to_string();
    assert!(
        transcript.trim_end().ends_with(&line),
        "after the transcript, last: {transcript}"
    );
    for part in [
        format!("promised: {PROMISE}"),
        "open: a delegation".to_string(),
        format!("set today: {FACT}"),
    ] {
        assert!(line.contains(&part), "{part:?} in {line:?}");
    }
    assert!(
        !transcript.contains(TOPIC),
        "never the running topic: {transcript}"
    );

    // The next rebuild: the model does not repeat the promise; the brief
    // keeps it.
    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let q3 = short_id(&user("q3"));
    // GH #1496: each line is dated with a day its transcript shows.
    let day = h.transcript_day();
    h.answer(
        &format!("- person ({day}) garden/state: The garden is in bloom. [asserted] [#{q3}]"),
        "stop",
    );
    let text = stored_text(&h);
    assert!(
        text.lines().any(|l| l == kept),
        "the listed promise stays in the brief: {text}"
    );
}
