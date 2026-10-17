//! GH #1225 (A13, OR-KF-29, OR-KF-31) -- a parked handover note whose flow
//! never came back is swept, once, and said.
//!
//! WHY this file exists: `./handover` parks the summarizer's note as
//! `pending:handover:<session>` between two steps. A note step that never
//! came back (a refused write, a lost answer) left the row for ever. Now the
//! note carries `parked_at`, and the next event that reads it -- a close of
//! its session, the first call of the next session -- deletes one older than
//! `pending_sweep_s` (seconds, at least 60, default 600, the knob `./intake`
//! and `./policy` carry), never by a timer of its own (R-AG-1). One
//! `pending_swept` mark `{keys, older_than_s}` follows in the next step and
//! names only what the store took. A young note stays; a row without
//! `parked_at` is never swept (the form of `./intake`, OR-KF-31).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

fn note(parked_at: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let mut v = json!({"text": "kh13 a parked note", "model": "m"});
    if let Some(at) = parked_at {
        v["parked_at"] = json!(stamp(at));
    }
    canonical(&v)
}

fn park(h: &Hive, session: &str, value: &str) {
    h.db.execute(
        "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![format!("pending:handover:{session}"), value],
    )
    .expect("park a note");
}

fn parked(h: &Hive, session: &str) -> Option<String> {
    h.rows(&format!(
        "SELECT value FROM state WHERE key = 'pending:handover:{session}'"
    ))
    .first()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
}

fn swept(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'pending_swept' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("")).unwrap_or(Value::Null))
        .collect()
}

fn close(h: &mut Hive, session: &str) {
    h.lane(
        "in_close",
        json!({"session_id": session, "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": []}),
    );
}

#[test]
fn the_sweep_age_is_a_knob_in_seconds() {
    if !shipped() {
        return;
    }
    let cfg = cell_config("handover");
    assert_eq!(cfg["params"]["pending_sweep_s"], json!("600"));
    let s = &cfg["contract"]["settings"]["pending_sweep_s"];
    assert_eq!(s["default"], json!("600"));
    assert_eq!(s["type"], json!("string"));
    assert!(cfg["params"].get("pending_sweep_ms").is_none());
}

#[test]
fn a_close_sweeps_only_its_own_old_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    for s in ["s-old", "s-young", "s-bare"] {
        turn(
            &mut h,
            s,
            "t1",
            &format!("kh13 {s}: the gate is open."),
            "kh13 noted.",
            json!({}),
        );
    }
    let young = aged(chrono::Duration::zero());
    let bare = note(None);
    park(&h, "s-old", &aged(chrono::Duration::days(1)));
    park(&h, "s-young", &young);
    park(&h, "s-bare", &bare);
    for s in ["s-old", "s-young", "s-bare"] {
        close(&mut h, s);
    }
    assert_eq!(parked(&h, "s-old"), None, "the old note is swept");
    assert_eq!(parked(&h, "s-young"), Some(young), "a young note stays");
    assert_eq!(
        parked(&h, "s-bare"),
        Some(bare),
        "a note without `parked_at` is never swept"
    );
    assert_eq!(
        swept(&h),
        vec![json!({"keys": ["pending:handover:s-old"], "older_than_s": 600})],
        "one mark, naming what went: {:?}",
        h.stderr
    );
}

/// A parked note whose `parked_at` lies `age` in the past.
fn aged(age: chrono::Duration) -> String {
    note(Some(chrono::Utc::now() - age))
}

/// `s-a` served with an old note parked, then the first call of `s-b`.
fn next_session_after_an_old_note(over: &[(&str, &str, Value)], age: chrono::Duration) -> Hive {
    let mut h = Hive::with(over);
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh13 a: the gate is open.",
        "kh13 noted.",
        json!({}),
    );
    park(&h, "s-a", &note(Some(chrono::Utc::now() - age)));
    turn(
        &mut h,
        "s-b",
        "b1",
        "kh13 b: hello.",
        "kh13 noted.",
        json!({}),
    );
    h
}

#[test]
fn the_next_session_sweeps_the_old_note_of_the_last() {
    if !shipped() {
        return;
    }
    let h = next_session_after_an_old_note(&[], chrono::Duration::days(1));
    assert_eq!(parked(&h, "s-a"), None, "swept: {:?}", h.stderr);
    assert_eq!(
        swept(&h),
        vec![json!({"keys": ["pending:handover:s-a"], "older_than_s": 600})]
    );
}

#[test]
fn the_knob_decides_the_age_and_never_goes_below_a_minute() {
    if !shipped() {
        return;
    }
    // A year: a day-old note stays.
    let h = next_session_after_an_old_note(
        &[("handover", "pending_sweep_s", json!("31536000"))],
        chrono::Duration::days(1),
    );
    assert!(parked(&h, "s-a").is_some(), "younger than the knob");
    assert!(swept(&h).is_empty());
    // One second is read as a minute: a note half a minute old stays.
    let h = next_session_after_an_old_note(
        &[("handover", "pending_sweep_s", json!("1"))],
        chrono::Duration::seconds(30),
    );
    assert!(parked(&h, "s-a").is_some(), "never below a minute");
    // A knob that is no number is the default.
    let h = next_session_after_an_old_note(
        &[("handover", "pending_sweep_s", json!("soon"))],
        chrono::Duration::days(1),
    );
    assert_eq!(parked(&h, "s-a"), None);
    assert_eq!(
        swept(&h),
        vec![json!({"keys": ["pending:handover:s-a"], "older_than_s": 600})]
    );
}
