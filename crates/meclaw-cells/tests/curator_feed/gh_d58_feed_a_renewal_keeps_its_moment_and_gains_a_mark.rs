//! D58 feed (GH #1481, plan `u-feed.md` contract point 6, review F-W1) -- a
//! renewal keeps its moment and gains a mark.
//!
//! WHY this file exists: the renewal after a closed turn (`live_feed`
//! `renew`, `at_turn_end` "1") had neither a claim nor a mark; its "once" is
//! the digest of the held state. It now goes through the one function every
//! feed goes through, but without `once`: a claim would put one more round
//! trip before the send and move the moment the voice hears it. It gains a
//! mark `feed` (section `renew`), written in the same message to the store
//! as the held state the renewal leaves -- whatever `feed_in` says (shipped
//! "0" here).

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_feed::*;
use curator_hive::*;
use curator_live::{ask_seed, closed_turn, live_keys, topic_value};
use meclaw_core::serde_json::{Value, json};

/// The index in `ops` of the first operation `pred` holds for.
fn first(ops: &[(String, Value)], pred: impl Fn(&Value) -> bool) -> Option<usize> {
    ops.iter().position(|(_, op)| pred(op))
}

#[test]
fn gh_d58_feed_a_renewal_keeps_its_moment_and_gains_a_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "live_feed", json!("1"))]);
    h.check_emits = true;
    clear_of_midnight();
    assert_eq!(ask_seed(&mut h, &live_keys(FEED_LIVE), "start").len(), 1);
    h.mark(
        backdated(20),
        SESSION,
        "t4",
        "topic",
        &topic_value("garden", "start"),
    );
    let from = h.ledger_ops.len();
    closed_turn(&mut h, SESSION, "t5", &live_keys(FEED_LIVE));

    let renew = feeds_of(&h, "renew");
    assert_eq!(renew.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        renew[0].hop["at_turn_end"],
        json!("1"),
        "the voice still takes it at its turn boundary"
    );
    let marks = feed_marks_of(&h, "renew");
    assert_eq!(
        marks.len(),
        1,
        "the renewal books `feed`: {marks:?} {:?}",
        h.stderr
    );
    assert_eq!(marks[0].session, SESSION);
    assert_eq!(marks[0].turn, "t5");
    assert!(
        claims(&h).is_empty(),
        "no claim for a renewal: {:?}",
        claims(&h)
    );

    // The moment: the mark rides the message that writes the held state --
    // one message to the store, not the next one (review m2: neighbours in
    // `ledger_ops` may be two messages; `ledger_op_step` names the message
    // of each operation).
    assert_eq!(
        h.ledger_op_step.len(),
        h.ledger_ops.len(),
        "one message index per operation"
    );
    let ops = &h.ledger_ops[from..];
    let steps = &h.ledger_op_step[from..];
    let key = json!(format!("seed_facts:{FEED_LIVE}"));
    let held_at = first(ops, |op| {
        op["table"] == json!("state")
            && op["operation"] != json!("select")
            && (op["row"]["key"] == key || op["where"]["key"] == key)
    })
    .unwrap_or_else(|| panic!("the renewal writes the held state: {ops:?}"));
    let mark_at = first(ops, |op| {
        op["table"] == json!("marks") && op["row"]["kind"] == json!(MARK)
    })
    .unwrap_or_else(|| panic!("the renewal writes its mark: {ops:?}"));
    assert_eq!(
        held_at.abs_diff(mark_at),
        1,
        "the held state and the mark side by side: {ops:?}"
    );
    assert_eq!(
        steps[held_at], steps[mark_at],
        "one step: the held state and the mark ride one message to the store: {ops:?} {:?}",
        h.ledger_steps
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
