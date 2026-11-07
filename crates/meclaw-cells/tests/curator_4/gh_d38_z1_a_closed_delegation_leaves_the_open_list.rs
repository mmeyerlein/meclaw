//! GH #1439 (D38 Z1) -- an open errand stands in the state block until it closes.
//!
//! WHY this file exists: the `open` part of the state block lists what the
//! companion still waits for -- a delegation of the session
//! (`pending:delegation:<session>#<turn>`, GH #1407) and a consult of the
//! session (`consult_pending:<session>`, GH #1280) -- each while its row
//! stands and its `until` lies ahead. A row the closing path took down, a row
//! whose `until` passed and a row of another session are never listed: a
//! state block that keeps a done errand open makes the model wait for an
//! answer that came long ago.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); the
//! rows are sown into the ledger the way the policy writes them.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn hive() -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
    ])
}

fn put_state(h: &Hive, key: &str, value: &str) {
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )
    .expect("a state row");
}

/// GH #1447: the consult mark is the lead's row (`lead_state`), the window
/// hears of it through the lead's verdict, never from the ledger.
fn put_lead_state(h: &Hive, key: &str, value: &str) {
    h.lead_db
        .as_ref()
        .expect("the tree ships the lead")
        .execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .expect("a lead state row");
}

fn drop_lead_state(h: &Hive, key: &str) {
    h.lead_db
        .as_ref()
        .expect("the tree ships the lead")
        .execute("DELETE FROM state WHERE key = ?1", [key])
        .expect("the row goes");
}

fn drop_state(h: &Hive, key: &str) {
    h.db.execute("DELETE FROM state WHERE key = ?1", [key])
        .expect("the row goes");
}

fn ms(at: chrono::DateTime<chrono::Utc>) -> i64 {
    at.timestamp_millis()
}

fn iso(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// A delegation row of `session`, opened by turn `label`, `until` as given.
fn delegation(h: &Hive, session: &str, label: &str, until_ms: i64) {
    let round = format!("{label}~{until_ms}~0000abcd");
    put_state(
        h,
        &format!("pending:delegation:{session}#{label}"),
        &canonical(
            &json!({"delegation_id": format!("d-{label}"), "round": round,
                           "until": until_ms, "parked_at": iso(backdated(5))}),
        ),
    );
}

/// The state line of the newest message of the person, "" without one.
fn state_line(c: &Msg) -> String {
    texts(c)
        .last()
        .cloned()
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string()
}

#[test]
fn a_closed_delegation_leaves_the_open_list() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s1",
        "t1",
        "Find me a plumber.",
        "I asked around.",
        json!({}),
    );
    let ahead = ms(chrono::Utc::now() + chrono::Duration::hours(1));
    delegation(&h, "s1", "t1", ahead);
    // Another session's errand is never this session's.
    delegation(&h, "s2", "t9", ahead);
    let c = h.curate("s1", "t2", 0, json!([user("Any news?")]), mode("Be brief."));
    assert_eq!(
        state_line(&c),
        "- [state -- open: a delegation]",
        "one open delegation, the one of this session: {:?} {:?}",
        texts(&c),
        h.stderr
    );
    h.tap(&c, "stop", json!({}), json!([said("Not yet.")]));
    // The closing path takes the row down: the errand leaves the list.
    drop_state(&h, "pending:delegation:s1#t1");
    let c = h.curate("s1", "t3", 0, json!([user("And now?")]), mode("Be brief."));
    assert!(
        !state_line(&c).contains("open:"),
        "a closed delegation is not open: {:?}",
        texts(&c)
    );
}

#[test]
fn a_delegation_past_its_until_is_not_open() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s1",
        "t1",
        "Find me a plumber.",
        "I asked around.",
        json!({}),
    );
    delegation(&h, "s1", "t1", ms(backdated(30)));
    let c = h.curate("s1", "t2", 0, json!([user("Any news?")]), mode("Be brief."));
    assert!(
        !state_line(&c).contains("open:"),
        "an `until` that passed closes the errand for the block: {:?}",
        texts(&c)
    );
}

#[test]
fn an_answered_consult_leaves_the_open_list() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s1",
        "t1",
        "Is this rash serious?",
        "Let me think.",
        json!({}),
    );
    let now = chrono::Utc::now();
    put_lead_state(
        &h,
        "consult_pending:s1",
        &canonical(
            &json!({"consult_id": "c-1", "at": iso(backdated(5)), "wait_ms": 120000,
                           "until": iso(now + chrono::Duration::hours(1))}),
        ),
    );
    let c = h.curate("s1", "t2", 0, json!([user("Well?")]), mode("Be brief."));
    assert_eq!(
        state_line(&c),
        "- [state -- open: a consult]",
        "{:?} {:?}",
        texts(&c),
        h.stderr
    );
    h.tap(&c, "stop", json!({}), json!([said("One moment.")]));
    drop_lead_state(&h, "consult_pending:s1");
    let c = h.curate("s1", "t3", 0, json!([user("Well?")]), mode("Be brief."));
    assert!(!state_line(&c).contains("open:"), "{:?}", texts(&c));
}
