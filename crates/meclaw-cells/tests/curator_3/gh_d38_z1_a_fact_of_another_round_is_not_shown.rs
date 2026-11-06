//! GH #1439 (D38 Z1) -- the state block reads through the audience gate.
//!
//! WHY this file exists: the state block is a new read of the ledger, and
//! every read of the curator passes the one audience gate (GH #925/#932): a
//! fact, a topic or a promise of another round -- another member's
//! conversation -- never stands in this round's block, even when it was set
//! today in the same session. The marks of the other round are sown with its
//! own `audience_set`, the way `./intake` writes them for that round.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const OTHER: &str = "[\"member:x\"]";

fn mark_under(h: &Hive, n: i64, aud: &str, kind: &str, value: &str) {
    let at = backdated(5) + chrono::Duration::microseconds(n);
    h.db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            at.timestamp_micros(),
            "s1",
            "t1",
            kind,
            value,
            at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
            aud
        ],
    )
    .expect("a mark");
}

#[test]
fn a_fact_of_another_round_is_not_shown() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
    ]);
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    let fact = |claim: &str| {
        canonical(
            &json!({"subject": "Hannah", "predicate": "lives_in", "claim": claim,
                          "source": "person", "stance": "asserted"}),
        )
    };
    mark_under(&h, 1, ROUND_E, "fact", &fact("Hannah lives in Porto."));
    mark_under(&h, 2, OTHER, "fact", &fact("Hannah lives in Oslo."));
    mark_under(
        &h,
        3,
        OTHER,
        "topic",
        &canonical(&json!({"movement": "start", "name": "the other's secret"})),
    );
    h.lane(
        "in_pin",
        json!({"audience_set": OTHER}),
        json!({}),
        json!({"pins": [{"text": "Keep the other's promise.", "source": "commitment"}]}),
    );
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let text = texts(&c).last().cloned().unwrap_or_default();
    assert!(
        text.contains("- [state -- set today: Hannah lives in Porto.]"),
        "this round's fact, and only it: {text} {:?}",
        h.stderr
    );
    for leak in ["Oslo", "the other's secret", "the other's promise"] {
        assert!(!text.contains(leak), "`{leak}` of another round: {text}");
    }
}
