//! GH #1225 (A3/EA3, writer) -- a close read the store refused is no empty session.
//!
//! WHY this file exists: `./writer` took the wall and the blocks of a close
//! from the legs of the store's answer and never looked at a leg's
//! `error_code`. A refused read (`query_timeout`, `store_degraded`) came back
//! as "no rows", and the session left as a `write` with `turn_count` 0 -- the
//! memory learned that a session with turns said nothing. Now a refused read
//! is sent once more (the phase with `-again`, `error_code` in the header);
//! the second read writes as any close does. Refused twice, the close says so
//! -- stderr and a `store_error` mark `{step, legs}` in the close round -- and
//! hands on NO `write` (OR-KF.KH.2: no receiver reads an `error_code` on
//! `write`, and an empty batch is exactly the misreading).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-kh-ea3";

/// A session with two participant turns and a tool round on the wall.
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
    h.lane(
        "in_close",
        json!({"session_id": S}),
        json!({}),
        json!({"messages": []}),
    );
}

fn store_error_marks(h: &Hive) -> Vec<(String, Value, String)> {
    h.rows("SELECT session_id, value, audience_set FROM marks WHERE kind = 'store_error'")
        .into_iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or("").to_string(),
                meclaw_core::serde_json::from_str(r[1].as_str().unwrap_or("")).unwrap_or_default(),
                r[2].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn assert_written_whole(h: &Hive) {
    let w = h.routed("write");
    assert_eq!(w.len(), 1, "one close batch: {:?}", h.stderr);
    assert_eq!(
        w[0].hop["turn_count"], "2",
        "the session's turns: {:?}",
        h.stderr
    );
    assert_eq!(w[0].hop["round_count"], "2");
    assert_eq!(w[0].messages(), vec![user("go"), said("done")]);
    assert!(
        store_error_marks(h).is_empty(),
        "a read that came is no error"
    );
}

#[test]
fn a_wall_read_refused_once_is_read_again_and_written() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("c-wall".into(), "query_timeout".into())];
    close(&mut h);
    assert!(h.refuse_legs.is_empty(), "the wall read was refused");
    assert_written_whole(&h);
}

#[test]
fn a_blocks_read_refused_once_is_read_again_and_written() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("c-blocks".into(), "store_degraded".into())];
    close(&mut h);
    assert!(h.refuse_legs.is_empty(), "the blocks read was refused");
    assert_written_whole(&h);
}

#[test]
fn a_wall_read_refused_twice_hands_on_no_write_and_says_so() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    // The first read times out; the read once more is refused as well (the
    // hive strikes one code per message, so the second is the phase's).
    h.refuse_legs = vec![("c-wall".into(), "query_timeout".into())];
    h.refuse_phase = Some("close-wall-again".into());
    close(&mut h);
    assert!(h.refuse_legs.is_empty(), "the wall read was refused");
    let w = h.routed("write");
    assert!(
        w.is_empty(),
        "a refused read is no session without turns: {:?}",
        w.iter().map(|m| &m.hop).collect::<Vec<_>>()
    );
    assert!(
        h.refuse_phase.is_none(),
        "the wall is read once more before the close gives up: {:?}",
        h.stderr
    );
    let said = h.stderr.join("\n");
    assert!(
        said.contains("query_timeout") && said.contains("store_error") && said.contains(S),
        "said on stderr, both refusals and the session: {said}"
    );
    let marks = store_error_marks(&h);
    assert_eq!(
        marks,
        vec![(
            S.to_string(),
            json!({"step": "close-wall", "legs": {"c-wall": "store_error"}}),
            ROUND_E.to_string()
        )],
        "one store_error mark in the curator's form, in the close round"
    );
}

#[test]
fn a_blocks_read_refused_twice_hands_on_no_write_and_says_so() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("c-blocks".into(), "store_degraded".into())];
    h.refuse_phase = Some("close-blocks-again".into());
    close(&mut h);
    assert!(h.refuse_legs.is_empty(), "the blocks read was refused");
    assert!(h.refuse_phase.is_none(), "read once more: {:?}", h.stderr);
    assert!(
        h.routed("write").is_empty(),
        "no batch without its blocks: {:?}",
        h.out
    );
    let marks = store_error_marks(&h);
    assert_eq!(marks.len(), 1, "one mark: {marks:?}");
    assert_eq!(marks[0].1["step"], "close-blocks");
    assert_eq!(marks[0].1["legs"]["c-blocks"], "store_error");
    assert!(h.stderr.join("\n").contains("store_degraded"));
}
