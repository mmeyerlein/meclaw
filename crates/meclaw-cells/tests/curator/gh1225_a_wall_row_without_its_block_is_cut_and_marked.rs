//! GH #1225 (K-8) -- a wall row without its block is said, never only logged.
//!
//! The window leaves out a wall row whose block the read did not bring (a
//! block still on its way, a ledger that lost one). Until now only a stderr
//! line said so; now the call's hop names it in `cuts` and the ledger keeps a
//! `missing_block` mark with the row's hash and seq.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn a_wall_row_without_its_block_is_cut_and_marked() {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // A fixed instant: no plan stands, so the window shows the whole wall
    // (cover 0) whatever the day of the run.
    let seq: i64 = 1_792_051_200_000_000;
    let hash = "f".repeat(64);
    h.db.execute(
        "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
         episode_idx, at, audience_set) VALUES (?1, 's', 'r0', 0, 'user', ?2, 0, 0, NULL, ?3, ?4)",
        rusqlite::params![seq, hash, "2026-10-15T08:00:00.000000Z", ROUND_E],
    )
    .unwrap();
    let call = h.curate("s", "t1", 0, json!([user("hello")]), mode("Be brief."));
    let cuts: Vec<Value> = call.hop["cuts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["what"] == "missing_block")
        .collect();
    assert_eq!(cuts.len(), 1, "{:?}", call.hop);
    assert_eq!(cuts[0]["seq"], json!(seq));
    assert_eq!(cuts[0]["block"], json!(&hash[..12]));
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'missing_block'");
    assert_eq!(marks.len(), 1, "{:?}", h.stderr);
    let v: Value = meclaw_core::serde_json::from_str(marks[0][0].as_str().unwrap()).unwrap();
    assert_eq!(v["hash"], json!(hash));
    assert_eq!(v["seq"], json!(seq));
}
