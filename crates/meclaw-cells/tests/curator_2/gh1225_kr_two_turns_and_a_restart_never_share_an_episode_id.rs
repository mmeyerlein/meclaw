//! GH #1225 (A2) -- two turns and a restart never share an episode id.
//!
//! WHY this file exists: `./intake` minted a round's episode index from the
//! read of step A plus its RAM (`_MEM["episode"]`). Two turns of one round
//! whose A answers both came back before the first B was written (double
//! texting, a burst) got distinct indexes only while the same child lived: a
//! child killed between them (a deploy, a `script_timeout`) minted the same
//! index twice, and `./writer` named two different turns of the person by one
//! id (`turn_write` `<session>#<tag>-<index>`). Now the ledger declares the
//! unique index `wall_episode`, and a B whose A another child sent writes its
//! person rows without an index and claims one in a bundle of its own, after
//! every write before it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-ea2";

fn curate_lane(turn: &str, words: &str) -> (&'static str, Value, Value, Value) {
    (
        "in_curate",
        json!({"session_id": S, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": ROUND_E}),
        json!({"session_id": S, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(words)], "system": mode("Be brief.")}),
    )
}

fn ids(h: &Hive) -> Vec<String> {
    h.routed("turn_write")
        .iter()
        .map(|m| m.hop["turn_id"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn two_turns_and_a_restart_never_share_an_episode_id() {
    if !shipped() {
        return;
    }
    for restart in [false, true] {
        let mut h = Hive::new();
        h.ledger_may_refuse = true;
        if restart {
            // A1, A2, B1 -- the child dies -- then the answer of A2 reaches
            // the new one, read before B1 was written.
            h.restart_at = Some(("intake".into(), "a".into(), 2));
        }
        h.lanes_together(vec![
            curate_lane("t1", "My dog is called Bello."),
            curate_lane("t2", "And my cat is called Mimi."),
        ]);
        assert!(h.restart_at.is_none(), "the restart struck");
        let written = ids(&h);
        assert_eq!(
            written.len(),
            2,
            "restart {restart}: one episode per turn: {written:?} {:?}",
            h.stderr
        );
        assert_ne!(
            written[0], written[1],
            "restart {restart}: two turns, two ids: {:?}",
            h.stderr
        );
        let unindexed = h.rows(&format!(
            "SELECT turn_id FROM wall WHERE session_id = '{S}' AND kind = 'user' \
             AND episode_idx IS NULL"
        ));
        assert!(
            unindexed.is_empty(),
            "restart {restart}: every person row holds its index: {unindexed:?}"
        );
        let twice = h.rows(&format!(
            "SELECT episode_idx FROM wall WHERE session_id = '{S}' AND episode_idx IS NOT NULL \
             GROUP BY audience_set, episode_idx HAVING count(*) > 1"
        ));
        assert!(
            twice.is_empty(),
            "restart {restart}: no index twice: {twice:?}"
        );
    }
}
