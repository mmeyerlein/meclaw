//! GH #1494 (D58 close, form review M-1 first part) -- the watch finds the
//! start of the running round by place, not by content.
//!
//! WHY this file exists: `window_watch` compares two windows across a round
//! boundary only up to the start of the running round of the window before.
//! It found that start by the first element whose hash has a place in that
//! round. An element of an older round with the same content as one of the
//! running round (the same words said twice, the same short answer) has
//! that hash too, so the comparison ended at the early element: a prefix
//! that broke between the two did not count. The rule now: of the
//! occurrences of one hash, the last ones are the running round's -- as many
//! as the round holds places of it.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn el(hash: &str, kind: &str) -> Value {
    json!({"hash": hash, "kind": kind, "form": "raw", "bytes": 10})
}

/// The verdicts of `window_watch` for the window before `prev` (its running
/// round `t2`) and this call's `now` (round `t3`).
fn watch(prev: Value, now: Value) -> Value {
    // `ok` and `twice` are said in round `t1` and again in `t2`.
    let placed = json!({
        "first": [[1, ["s", "t1"]]],
        "twice": [[2, ["s", "t1"]], [4, ["s", "t2"]]],
        "ask": [[3, ["s", "t2"]]],
        "other": [[2, ["s", "t1"]]],
        "next": [[5, ["s", "t3"]]],
    });
    let probe = "window_watch(ARGS['prev'], ARGS['prev_round'], ARGS['now'], \
                 ARGS['now_round'], dict((h, [(s, tuple(r)) for s, r in v]) \
                 for h, v in ARGS['placed'].items()), set(), 0, lambda h: False, False)";
    let (got, err) = policy_scope(
        json!({}),
        probe,
        json!({"prev": prev, "prev_round": ["s", "t2"], "now": now,
               "now_round": ["s", "t3"], "placed": placed}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// Lock: an element of an older round that changed breaks the prefix, also
/// when its content stands again in the running round.
#[test]
fn a_change_before_the_round_counts_when_its_content_repeats_in_it() {
    if !shipped() {
        return;
    }
    let prev = json!([
        el("first", "words"),
        el("twice", "words"),
        el("ask", "words"),
        el("twice", "answer")
    ]);
    let now = json!([
        el("first", "words"),
        el("other", "words"),
        el("ask", "words"),
        el("twice", "answer"),
        el("next", "words")
    ]);
    let got = watch(prev, now);
    assert_eq!(
        got["prefix_break"],
        json!(1),
        "the second element lay before the running round and changed: {got}"
    );
}

/// Check of the rule: the running round itself may change across the
/// boundary -- the same windows with the change inside `t2` count nothing.
#[test]
fn a_change_inside_the_round_counts_nothing() {
    if !shipped() {
        return;
    }
    let prev = json!([
        el("first", "words"),
        el("twice", "words"),
        el("ask", "words"),
        el("twice", "answer")
    ]);
    let now = json!([
        el("first", "words"),
        el("twice", "words"),
        el("next", "words")
    ]);
    let got = watch(prev, now);
    assert_eq!(got["prefix_break"], json!(0), "{got}");
}
