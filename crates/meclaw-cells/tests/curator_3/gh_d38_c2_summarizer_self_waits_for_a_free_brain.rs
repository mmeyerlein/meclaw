//! GH #1448 (D38 C2, rule (c)) -- `summarizer_self` condenses on the role's
//! own model only while `brain` is free.
//!
//! WHY this file exists: with `summarizer_self` "1" (GH #1440) a rebuild asks
//! the role's own model to condense, on the way `brain`, after its answer.
//! C2 builds the plan ahead of the turn; a condensing call on `brain` while a
//! turn of the same session waits for its call would stand in front of the
//! person's answer. So the self call runs only when no turn of the session is
//! parked; else the summarizer condenses, as with the knob off (M2 measures
//! the delay, P3).
//!
//! Stub summarizer and stub model (`support/curator_hive.rs`); the parked
//! turn is a `pending:` row as the curator parks one.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// A turn of `session` parked in the ledger, waiting for its call.
fn park(h: &Hive, key: &str, session: &str) {
    let value = json!({"session": session, "turn": "t4", "iter": "0",
                       "hop": {"session_id": session, "turn_id": "t4"},
                       "parked_at": "2099-01-01T00:00:00Z"});
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value.to_string()],
    )
    .expect("a parked turn");
}

/// A compress rebuild of round E with `summarizer_self` "1" after three
/// answered turns of `s1`; `parked` runs before the order fires.
fn rebuild_with(parked: impl FnOnce(&Hive)) -> Hive {
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summarizer_self", json!("1")),
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
    parked(&h);
    h.out.clear();
    let mut order = last_add(&h);
    order.body["emit_body"]["reason"] = json!("compress");
    h.fire(&order);
    h
}

fn self_calls(h: &Hive) -> Vec<Msg> {
    h.routed("brain")
        .into_iter()
        .filter(|m| m.hop.get("summarize") == Some(&Value::from("1")))
        .collect()
}

#[test]
fn a_free_brain_condenses_itself() {
    if !shipped() {
        return;
    }
    let h = rebuild_with(|_| {});
    assert_eq!(
        self_calls(&h).len(),
        1,
        "the role condenses: {:?}",
        h.stderr
    );
    assert!(h.summ.is_empty(), "no request to ./summarizer");
}

#[test]
fn a_turn_waiting_in_the_session_hands_the_summary_to_the_summarizer() {
    if !shipped() {
        return;
    }
    let h = rebuild_with(|h| park(h, "pending:c-waiting", "s1"));
    assert!(
        self_calls(&h).is_empty(),
        "nothing condenses on the busy brain: {:?}",
        h.out
    );
    assert_eq!(h.summ.len(), 1, "the summarizer condenses: {:?}", h.stderr);
}

#[test]
fn a_turn_waiting_in_another_session_leaves_the_brain_free() {
    if !shipped() {
        return;
    }
    let h = rebuild_with(|h| park(h, "pending:c-elsewhere", "s2"));
    assert_eq!(
        self_calls(&h).len(),
        1,
        "the role condenses: {:?}",
        h.stderr
    );
    assert!(h.summ.is_empty(), "no request to ./summarizer");
}
