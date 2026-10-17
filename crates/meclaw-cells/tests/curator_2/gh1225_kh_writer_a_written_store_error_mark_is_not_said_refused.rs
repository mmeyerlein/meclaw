//! GH #1225 (A3, writer) -- a `store_error` mark the store wrote is not said refused.
//!
//! WHY this file exists: a close whose read the store refused twice writes a
//! `store_error` mark, and the answer to that write was checked like a read:
//! an insert answers without rows, so every written mark was said "refused
//! ... too" on stderr (re-check KH M-1). Now only a leg the store refused (or
//! one the answer lacks) is said refused.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-kh-m1";

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

fn store_error_marks(h: &Hive) -> usize {
    h.rows("SELECT value FROM marks WHERE kind = 'store_error'")
        .len()
}

#[test]
fn a_written_store_error_mark_is_not_said_refused() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("c-wall".into(), "query_timeout".into())];
    h.refuse_phase = Some("close-wall-again".into());
    close(&mut h);
    assert!(h.refuse_phase.is_none(), "read once more: {:?}", h.stderr);
    assert_eq!(store_error_marks(&h), 1, "the mark is written");
    let said = h.stderr.join("\n");
    assert!(
        !said.contains("refused the store_error mark"),
        "a written mark is no refusal: {said}"
    );
}

#[test]
fn a_refused_store_error_mark_is_said() {
    if !shipped() {
        return;
    }
    let mut h = session_with_turns();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![
        ("c-wall".into(), "query_timeout".into()),
        ("c-refused".into(), "store_degraded".into()),
    ];
    h.refuse_phase = Some("close-wall-again".into());
    close(&mut h);
    assert!(h.refuse_legs.is_empty(), "both legs were refused");
    assert_eq!(store_error_marks(&h), 0, "the mark was refused");
    let said = h.stderr.join("\n");
    assert!(
        said.contains("refused the store_error mark") && said.contains("c-refused store_degraded"),
        "a refused mark is said: {said}"
    );
}
