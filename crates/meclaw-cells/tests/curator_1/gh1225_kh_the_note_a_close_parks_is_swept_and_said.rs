//! GH #1225 (A13, OR-KF-31) -- the note a close parks itself carries its time,
//! and `pending_swept` names only what the store took.
//!
//! WHY this file exists: the sweep pins park their note by hand, so a close
//! that parked its own note without `parked_at` left the suite green while
//! that note was never swept (re-check KH I-1). And no pin let a sweep delete
//! miss: a mark naming every key it asked for, gone or not, stayed green
//! (I-2). Here the close parks the note itself (the summarizer answered, the
//! store refused the note step), and a note parked anew between the sweep's
//! read and its delete stays -- and is not named.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

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

fn day_ago() -> String {
    (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

#[test]
fn the_note_a_close_parks_carries_its_time_and_is_swept() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh-i1 a: the parcel went to Ulm.",
        "kh-i1 noted.",
        json!({}),
    );
    close(&mut h, "s-a");
    // The note step never comes back whole: the store refuses its bundle.
    h.ledger_may_refuse = true;
    h.refuse_phase = Some("done".into());
    h.answer("kh-i1 note: the parcel went to Ulm.", "stop");
    assert!(
        h.refuse_phase.is_none(),
        "the note step was refused: {:?}",
        h.stderr
    );
    let value = parked(&h, "s-a").expect("the close parked its note");
    let mut note: Value = sj::from_str(&value).expect("a parked note is JSON");
    assert_eq!(
        note.get("text").and_then(Value::as_str),
        Some("kh-i1 note: the parcel went to Ulm.")
    );
    let at = note
        .get("parked_at")
        .and_then(Value::as_str)
        .expect("the note the close parks carries `parked_at`")
        .to_string();
    assert!(
        chrono::DateTime::parse_from_rfc3339(&at).is_ok(),
        "`parked_at` is a time: {at}"
    );
    // The same note, its time a day back: older than `pending_sweep_s`.
    note["parked_at"] = json!(day_ago());
    h.db.execute(
        "UPDATE state SET value = ?1 WHERE key = 'pending:handover:s-a'",
        rusqlite::params![canonical(&note)],
    )
    .expect("age the parked note");
    close(&mut h, "s-a");
    assert_eq!(parked(&h, "s-a"), None, "swept: {:?}", h.stderr);
    assert_eq!(
        swept(&h),
        vec![json!({"keys": ["pending:handover:s-a"], "older_than_s": 600})]
    );
}

#[test]
fn a_sweep_whose_delete_takes_no_row_names_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-old",
        "t1",
        "kh-i2: the gate is open.",
        "kh-i2 noted.",
        json!({}),
    );
    let old = canonical(&json!({"text": "kh-i2 old", "model": "m", "parked_at": day_ago()}));
    h.db.execute(
        "INSERT OR REPLACE INTO state (key, value) VALUES ('pending:handover:s-old', ?1)",
        rusqlite::params![old],
    )
    .expect("park an old note");
    // Between the sweep's read and its delete, the session parks a note
    // anew: the delete of the old value takes no row. (The new note's time is
    // never read here: the step has swept by then.)
    let young = canonical(&json!({"text": "kh-i2 young", "model": "m",
                                  "parked_at": "2026-10-16T06:00:00.000000Z"}));
    h.sql_at_phase = Some((
        "swept".into(),
        format!(
            "UPDATE state SET value = '{}' WHERE key = 'pending:handover:s-old'",
            young.replace('\'', "''")
        ),
    ));
    close(&mut h, "s-old");
    assert!(
        h.sql_at_phase.is_none(),
        "the sweep reached the store: {:?}",
        h.stderr
    );
    assert_eq!(
        parked(&h, "s-old"),
        Some(young),
        "the note parked anew stays"
    );
    assert_eq!(
        swept(&h),
        Vec::<Value>::new(),
        "a delete that took no row is not named: {:?}",
        h.stderr
    );
}
