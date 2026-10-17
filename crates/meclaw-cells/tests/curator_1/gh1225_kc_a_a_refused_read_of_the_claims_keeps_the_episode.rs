//! GH #1225 (A2, delta check of KR) -- a refused read of the parked claims
//! never leaves an episode index on the wall without its episode.
//!
//! WHY this file exists: a turn whose step A another child sent claims its
//! episode index in a bundle of its own (phase `e`) and hands the episodes on
//! from the store's answer (phase `ec`), reading the parked claims once more
//! there (`e-again`). Measured: `e-again` refused by the store -- the claim
//! was taken, the wall row holds index 0, but no episode reached `./writer`
//! (only stderr), and `e-drop` in the same bundle deleted the parked claims:
//! an index without its episode and nothing left to try again with. Now the
//! parked claims stay until the episodes left, a refused read is read once
//! more from the store's answer (no timer), and a second refusal frees the
//! claimed index again, said with one `store_error` mark.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-kca";

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

fn user_rows(h: &Hive) -> Vec<Vec<meclaw_core::serde_json::Value>> {
    h.rows(&format!(
        "SELECT turn_id, episode_idx FROM wall WHERE session_id = '{S}' AND kind = 'user' \
         ORDER BY turn_id"
    ))
}

fn parked_claims(h: &Hive) -> usize {
    h.rows("SELECT key FROM state WHERE key LIKE 'pending:%:e'")
        .len()
}

/// The child dies before the answer of step A, so the B claims its index in
/// phase `e`; `refuse` names the reads of the parked claims the store refuses.
fn claimed_with(refuse: &[&str]) -> Hive {
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    h.restart_at = Some(("intake".into(), "a".into(), 1));
    h.refuse_legs = refuse
        .iter()
        .map(|l| (l.to_string(), "store_error".to_string()))
        .collect();
    curate(&mut h, "t1", "My dog is called Bello.");
    assert!(h.restart_at.is_none(), "the restart struck");
    assert!(
        h.refuse_legs.is_empty(),
        "every refusal struck: {:?} {:?}",
        h.refuse_legs,
        h.stderr
    );
    h
}

#[test]
fn a_refused_read_of_the_claims_is_read_once_more_and_the_episode_goes_on() {
    if !shipped() {
        return;
    }
    let h = claimed_with(&["e-again"]);
    assert_eq!(
        ids(&h).len(),
        1,
        "the episode of the claimed row reached the writer: {:?}",
        h.stderr
    );
    assert_eq!(
        user_rows(&h),
        vec![vec![json!("t1"), json!(0)]],
        "{:?}",
        h.stderr
    );
    assert_eq!(
        parked_claims(&h),
        0,
        "the parked claims go once the episode left"
    );
}

#[test]
fn a_read_of_the_claims_refused_twice_leaves_no_index_without_its_episode() {
    if !shipped() {
        return;
    }
    let mut h = claimed_with(&["e-again", "e-again2"]);
    assert!(
        ids(&h).is_empty(),
        "no episode without its words: {:?}",
        ids(&h)
    );
    assert_eq!(
        user_rows(&h),
        vec![vec![json!("t1"), json!(null)]],
        "the claimed index is freed again -- never an index without its episode: {:?}",
        h.stderr
    );
    assert_eq!(
        parked_claims(&h),
        1,
        "the parked claims are never dropped before their episode left"
    );
    let marks: Vec<String> = h
        .rows("SELECT value FROM marks WHERE kind = 'store_error' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        marks,
        vec![canonical(&json!({"step": "ec", "legs": {
            "e-again": "store_error", "e-again2": "store_error"}}))],
        "one store_error mark in the one form: {:?}",
        h.stderr
    );
    // The next turn takes the next index: the freed one is a gap, never a
    // second id.
    curate(&mut h, "t2", "And my cat is called Mimi.");
    assert_eq!(ids(&h).len(), 1, "{:?} {:?}", ids(&h), h.stderr);
    assert_eq!(
        user_rows(&h),
        vec![vec![json!("t1"), json!(null)], vec![json!("t2"), json!(1)]],
        "{:?}",
        h.stderr
    );
}
