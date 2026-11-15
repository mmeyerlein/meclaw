//! GH #1462 (D58 snap, R-27-15) -- a snapshot row carries its audience.
//!
//! WHY this file exists: the `sent` list names every element a round was
//! sent, so it is as private as the round. The row that carries it -- the
//! call's and the bridge's -- carries the audience of the round that caused
//! the call, in the canonical form every audience-gated read of the ledger
//! compares; a row with a `sent` list and no round would be shown to nobody
//! or, read by hand, to anybody.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ROUND_F: &str = "[\"member:f\"]";

fn seam_in(h: &mut Hive, round: &str, session: &str, ask: &str) -> (Msg, Msg) {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "complex", "needs": "", "families": "web"});
    let ctx = json!({"session_id": session, "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": round, "path": "complex"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    let (call, bridge) = calls_out(h);
    let bridge = bridge.unwrap_or_else(|| panic!("a bridge: {:?} {:?}", h.out, h.stderr));
    (call, bridge)
}

fn audience_of_row(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    h.rows(&format!(
        "SELECT audience_set FROM snapshots WHERE call_id = '{id}'"
    ))
    .first()
    .map(|r| r[0].clone())
    .unwrap_or(Value::Null)
}

#[test]
fn the_row_of_a_call_and_of_its_bridge_carry_the_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    let e = seam_in(&mut h, ROUND_E, "s1", "a question in e");
    let f = seam_in(&mut h, ROUND_F, "s2", "a question in f");
    for (round, (call, bridge)) in [(ROUND_E, &e), (ROUND_F, &f)] {
        for (name, c) in [("call", call), ("bridge", bridge)] {
            let sent = sent_of(&h, c).unwrap_or_else(|| {
                panic!("the snapshot row of the {name} of {round} carries `sent`")
            });
            assert!(!sent_window(&h, &sent).messages.is_empty(), "{sent:?}");
            assert_eq!(
                audience_of_row(&h, c),
                json!(round_canon(round).expect("a round")),
                "the row of the {name} carries the round {round}"
            );
        }
    }
    // The words of one round are in no list of the other.
    let words_e = sha256_hex(&canonical(&user("a question in e")));
    for c in [&f.0, &f.1] {
        let sent = sent_of(&h, c).expect("as above");
        assert!(
            sent.iter().all(|x| x["hash"] != json!(words_e)),
            "a row of {ROUND_F} lists nothing of {ROUND_E}: {sent:?}"
        );
    }
    let no_round =
        h.rows("SELECT count(*) FROM snapshots WHERE audience_set IS NULL")[0][0].as_i64();
    assert_eq!(no_round, Some(0), "no snapshot row without its round");
}
