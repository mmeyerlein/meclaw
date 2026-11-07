//! GH #1225 (A2, re-check I-1) -- a refused write never lets the next turn
//! mint an episode id that was handed on already.
//!
//! WHY this file exists: `./intake` hands a turn's episode to `./writer` in
//! the same output as the B that writes its wall row, and its RAM forgot the
//! index as soon as a later step A had left. Measured: the B of turn 1 refused
//! by the store -- the writer already holds `<session>#<tag>-0`, the wall has
//! no row -- and turn 2 read no index, forgot the RAM and minted 0 again: two
//! turns of the person under one `turn_write` id. The same in the claim
//! bundle (phase `e`): its episodes left before the store took the claims.
//! Now the RAM keeps the highest index this child handed on per round as a
//! floor (a gap is harmless, a second id never), a re-sent row keeps its
//! index, and a claim hands its episode on only once the store took it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-ri1";

fn curate(h: &mut Hive, turn: &str, words: &str) {
    h.lane(
        "in_curate",
        json!({"session_id": S, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": ROUND_E}),
        json!({"session_id": S, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(words)], "system": mode("Be brief.")}),
    );
}

fn ids(h: &Hive) -> Vec<String> {
    h.routed("turn_write")
        .iter()
        .map(|m| m.hop["turn_id"].as_str().unwrap_or("").to_string())
        .collect()
}

fn once_each(h: &Hive, what: &str) {
    let written: Vec<String> = ids(h);
    let mut seen = std::collections::BTreeSet::new();
    for id in &written {
        assert!(
            seen.insert(id.clone()),
            "{what}: one id, two turns: {written:?} {:?}",
            h.stderr
        );
    }
}

#[test]
fn a_refused_b_never_lets_the_next_turn_reissue_its_episode_id() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    h.refuse_phase = Some("win-hashes".into());
    curate(&mut h, "t1", "My dog is called Bello.");
    assert!(h.refuse_phase.is_none(), "the refusal struck");
    curate(&mut h, "t2", "And my cat is called Mimi.");
    once_each(&h, "refused B");
    assert_eq!(ids(&h).len(), 2, "both turns handed on: {:?}", h.stderr);
    // Turn 1 sent again: its row keeps the id it was handed on with.
    curate(&mut h, "t1", "My dog is called Bello.");
    let written = ids(&h);
    assert_eq!(written.len(), 3, "{written:?} {:?}", h.stderr);
    assert_eq!(written[2], written[0], "the row sent again keeps its id");
    assert_eq!(
        h.rows(&format!(
            "SELECT turn_id, episode_idx FROM wall WHERE session_id = '{S}' AND kind = 'user' \
             ORDER BY turn_id"
        )),
        vec![vec![json!("t1"), json!(0)], vec![json!("t2"), json!(1)]],
        "{:?}",
        h.stderr
    );
}

#[test]
fn a_refused_claim_hands_no_episode_on() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    // The child dies before the answer of step A: the B claims its index in
    // phase `e`, and the store refuses the claim.
    h.restart_at = Some(("intake".into(), "a".into(), 1));
    h.refuse_legs = vec![("e-c*".into(), "store_error".into())];
    curate(&mut h, "t1", "My dog is called Bello.");
    assert!(h.restart_at.is_none(), "the restart struck");
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    curate(&mut h, "t2", "And my cat is called Mimi.");
    once_each(&h, "refused claim");
    let unindexed = h.rows(&format!(
        "SELECT turn_id FROM wall WHERE session_id = '{S}' AND kind = 'user' \
         AND episode_idx IS NULL"
    ));
    assert_eq!(
        unindexed,
        vec![vec![json!("t1")]],
        "the refused claim leaves its row without an index: {:?}",
        h.stderr
    );
}
