//! GH #1439 (D38 Z1) -- `state_block` "0" is the window of before.
//!
//! WHY this file exists: the knob ships "0" (the set switches it on), so a
//! ledger full of state -- a named topic, an open promise, a fact of today, an
//! open delegation -- must leave the window of a talky call exactly as it was
//! without that state: the same messages byte for byte, no state line, no
//! `curator` note in the block. A mutant that builds the block without the
//! knob turns this red.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn sow(h: &mut Hive) {
    let at = backdated(5);
    h.mark(
        at,
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "the garden"})),
    );
    h.mark(
        at,
        "s1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Hannah", "predicate": "lives_in",
                           "claim": "Hannah lives in Porto.", "source": "person",
                           "stance": "asserted"})),
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
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [{"id": "n1", "source": "probe", "text": "Speak slowly.",
                          "scope": "session"}]}),
    );
}

fn window(h: &mut Hive) -> Vec<Value> {
    turn(h, "s1", "t1", "We planted tomatoes.", "Lovely.", json!({}));
    h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."))
        .messages()
}

#[test]
fn off_is_byte_identical() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let policy = cell_config("policy");
    assert_eq!(policy["params"]["state_block"], json!("0"), "ships off");
    assert_eq!(
        policy["contract"]["settings"]["state_block"]["default"],
        json!("0"),
        "the setting says the same"
    );
    for knob in [None, Some(json!("0")), Some(json!(false))] {
        let over = |extra: &Option<Value>| -> Hive {
            let mut o = Vec::<(&str, &str, Value)>::new();
            if let Some(v) = extra {
                o.push(("policy", "state_block", v.clone()));
            }
            Hive::with(&o).reporting_composite("talky")
        };
        // The same conversation and the same note, once with the state sown
        // into the ledger and once without.
        let mut seeded = over(&knob);
        let mut bare = over(&knob);
        bare.lane(
            "in_note",
            json!({"session_id": "s1"}),
            json!({}),
            json!({"notes": [{"id": "n1", "source": "probe", "text": "Speak slowly.",
                              "scope": "session"}]}),
        );
        sow(&mut seeded);
        let (a, b) = (window(&mut seeded), window(&mut bare));
        assert_eq!(a, b, "{knob:?}: the window does not see the state");
        let all = meclaw_core::serde_json::to_string(&a).expect("messages");
        assert!(!all.contains("[state --"), "{knob:?}: {all}");
        assert!(!all.contains("from curator"), "{knob:?}: {all}");
    }
}
