//! GH #1225 (A12, writer) -- a close hands on the wall it read, once.
//!
//! WHY this file exists: a close read the wall twice -- once for the hashes
//! of its blocks (`close-wall`), once more with the blocks (`close-blocks`)
//! -- and built the batch from the SECOND read with the blocks of the FIRST.
//! A row that landed between the two was in the batch when its block happened
//! to be among the first hashes, and silently missing when it was not. Now
//! `close-wall` carries the newest `seq` it read and `close-blocks` reads the
//! wall only up to it: a late row belongs to the next close, whole. A row of
//! the read whose block is not there is said on stderr, never dropped
//! without a word.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-kh-a12";

fn session_with_turns() -> Hive {
    let mut h = Hive::new();
    let call = h.curate(
        S,
        "t1",
        1,
        json!([user("go"), tool_call("c1", "look"), tool_result("c1", "x")]),
        Value::Null,
    );
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("done\n\n```sidecar\n{}\n```")]),
    );
    h.out.clear();
    h.stderr.clear();
    h
}

fn close(h: &mut Hive) {
    h.out.clear();
    h.lane(
        "in_close",
        json!({"session_id": S}),
        json!({}),
        json!({"messages": []}),
    );
}

/// A copy of the session's `user` row, newer than every row: a turn landing
/// on the wall while the close is under way, its block one the first read
/// already asked for.
fn late_row_sql(turn: &str) -> String {
    format!(
        "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
         episode_idx, at, audience_set) \
         SELECT (SELECT MAX(seq) FROM wall) + 1, session_id, '{turn}', iter, kind, hash, nth, \
         final, (SELECT MAX(episode_idx) FROM wall WHERE session_id = '{S}') + 1, at, \
         audience_set FROM wall WHERE session_id = '{S}' AND kind = 'user' LIMIT 1"
    )
}

#[test]
fn a_row_landing_between_the_two_reads_waits_for_the_next_close() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.sql_at_phase = Some(("close-blocks".into(), late_row_sql("t-late")));
    close(&mut h);
    assert!(h.sql_at_phase.is_none(), "the late row landed mid-close");
    let w = h.routed("write");
    assert_eq!(w.len(), 1, "{:?}", h.stderr);
    assert_eq!(
        w[0].hop["turn_count"],
        "2",
        "the batch is the wall the close read first: {:?}",
        w[0].messages()
    );
    assert_eq!(w[0].messages(), vec![user("go"), said("done")]);
    // The next close hands it on, with its block.
    close(&mut h);
    let w = h.routed("write");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].hop["turn_count"], "3");
    assert_eq!(w[0].messages(), vec![user("go"), said("done"), user("go")]);
}

#[test]
fn a_row_without_its_block_is_said_never_dropped_silently() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    let lost = "0".repeat(64);
    h.db.execute_batch(&format!(
        "INSERT INTO wall (seq, session_id, turn_id, iter, kind, hash, nth, final, \
         episode_idx, at, audience_set) \
         SELECT (SELECT MAX(seq) FROM wall) + 1, session_id, 't-lost', iter, kind, '{lost}', \
         nth, final, (SELECT MAX(episode_idx) FROM wall WHERE session_id = '{S}') + 1, at, \
         audience_set FROM wall WHERE session_id = '{S}' AND kind = 'user' LIMIT 1"
    ))
    .expect("a wall row without a block");
    close(&mut h);
    let w = h.routed("write");
    assert_eq!(w.len(), 1, "{:?}", h.stderr);
    assert_eq!(w[0].messages(), vec![user("go"), said("done")]);
    let said = h.stderr.join("\n");
    assert!(
        said.contains("writer:") && said.contains(&lost),
        "the row without a block is said: {said:?}"
    );
}
