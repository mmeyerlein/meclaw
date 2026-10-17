//! GH #1247 (T4-08) -- push candidates reach the quick round without the
//! memory leg.
//!
//! WHY this file exists: a push candidate (GH #949) came into the window only
//! on the ask path of `./push` -- the collector's `recall` -- and its pair was
//! ahead of the round only because the round came in after the memory had
//! answered. A `simple` turn does not wait for the memory (GH #1244,
//! `route_no_recall`), and a quick round may come with no ask at all. Here the
//! round comes alone, on `quick`, and must still carry the candidate pair
//! ahead of the person's words.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// The id of a candidate pair: this prefix and 16 hex digits.
const PAIR_ID: &str = "call_candidates_";

/// One candidate as the door files it (the form of gh949): its text a
/// `candidate` block, its row under the round `ROUND_E`, no trigger, no end.
fn sow(h: &mut Hive, id: &str, text: &str) {
    let el = json!({"type": "candidate", "source": "app-test", "text": text});
    let body = canonical(&el);
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
         VALUES (?1, 'candidate', ?2, ?3, '2026-10-01T10:00:00.000000Z')",
        rusqlite::params![hash, text.chars().count() as i64, body],
    )
    .unwrap();
    h.db.execute(
        "INSERT INTO candidates (source, cand_id, hash, triggers, until, once, used_at, \
         last_seq, priority, audience_set, at) \
         VALUES ('app-test', ?1, ?2, '[]', '', 0, '', 0, 5, ?3, \
         '2026-10-01T10:00:00.000000Z')",
        rusqlite::params![id, hash, ROUND_E],
    )
    .unwrap();
}

#[test]
fn a_quick_turn_carries_its_candidates_without_the_memory() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("push", "candidate_cooldown", json!(0)),
    ]);
    sow(&mut h, "fw", "ticket 7 is open");

    // The round alone: no `recall` of the collector before it, no memory leg.
    h.out.clear();
    let words = "how is my ticket doing?";
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "simple", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(words)], "system": {"instructions": {"mode": {"text": "Be brief."}}}}),
    );

    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "one call on quick: {:?} {:?}",
        h.out,
        h.stderr
    );
    let msgs = quick[0].messages();
    let at = msgs
        .iter()
        .position(|m| {
            m["type"] == "tool_result" && m["id"].as_str().is_some_and(|i| i.starts_with(PAIR_ID))
        })
        .unwrap_or_else(|| {
            panic!(
                "no candidate pair in the quick round: {msgs:?} {:?}",
                h.stderr
            )
        });
    assert!(
        msgs[at]["text"]
            .as_str()
            .is_some_and(|t| t.ends_with("[ticket 7 is open]")),
        "the result carries the candidate: {:?}",
        msgs[at]
    );
    let own = msgs
        .iter()
        .position(|m| {
            m["origin"] == "user" && m["text"].as_str().is_some_and(|t| t.ends_with(words))
        })
        .unwrap_or_else(|| panic!("the round's own words: {msgs:?}"));
    assert!(
        at < own,
        "the pair opens the round, ahead of its words: {msgs:?}"
    );
    assert!(
        h.routed("recall").is_empty(),
        "no question leaves for the memory on the round's own pick: {:?}",
        h.out
    );
}
