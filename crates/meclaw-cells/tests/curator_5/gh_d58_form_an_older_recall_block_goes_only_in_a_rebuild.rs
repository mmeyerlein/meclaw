//! GH #1463 (D58 form) -- an older recall block goes only in a rebuild.
//!
//! WHY this file exists: a recall block that was sent is part of the prefix
//! the model's cache holds. `recall_in_wall` used to take such a block out
//! of any later call once its quote stood in the wall, in the middle of the
//! window, and everything behind it was written to the cache anew. The form
//! keeps what was sent while the window is warm; the garbage collection of a
//! rebuild -- which writes the cache anew anyway -- checks every recall block
//! of an earlier round with the same rule and collects the ones whose every
//! quote stands raw in the wall (class `dup_recall`, a whole block now, not
//! only a row a younger result repeats).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_0000000000formrb";
const SAID: &str = "My sister Anna moved to Hamburg.";
const LATER: &str = "We sold the old car last spring.";
const WHY: &str = "recall_in_wall";

/// The answering model's stamp: a cache that is warm for the whole test.
fn warm() -> Value {
    json!({"cache_expires_at": "2099-01-01T00:00:00Z"})
}

fn call(h: &mut Hive, turn_id: &str, words: &str) -> Msg {
    h.curate("s1", turn_id, 0, json!([user(words)]), mode("Be brief."))
}

fn close(h: &mut Hive, c: &Msg, reply: &str) {
    h.tap(c, "stop", warm(), json!([said(reply)]));
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

fn hop_cuts(c: &Msg) -> Vec<Value> {
    // The policy writes `cuts` only when it cut something: a missing key is "no cut".
    c.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == WHY)
        .collect()
}

/// A recall block of `t2` whose quote [`LATER`] stands outside the wall when
/// the block is decided (`t3`), and in it from `t4` on: the person says the
/// words. Returns the hive after the answer of `t4` and the call of `t4`.
fn an_older_block() -> (Hive, Msg) {
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "gc", json!("1")),
    ]);
    let c1 = call(&mut h, "t1", SAID);
    close(&mut h, &c1, "Hamburg is lovely.");
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{LATER}\""))
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    close(&mut h, &c2, "She likes it there.");
    let c3 = call(&mut h, "t3", "And?");
    assert!(
        text(&c3).contains(RECALL_ID),
        "check of the setup: the quote from outside the wall stays: {}",
        text(&c3)
    );
    close(&mut h, &c3, "I do not know more.");
    let c4 = call(&mut h, "t4", LATER);
    close(&mut h, &c4, "A good price, I hope.");
    (h, c4)
}

/// Pin: while the window is warm the block stays, although its quote now
/// stands in the wall -- on the call that brings the words and on the one
/// after it.
#[test]
fn a_warm_call_keeps_the_older_block() {
    if !shipped() {
        return;
    }
    let (mut h, c4) = an_older_block();
    let t = text(&c4);
    assert!(t.contains(RECALL_ID), "sent before, sent again: {t}");
    assert_eq!(
        t.matches(LATER).count(),
        2,
        "the person's words and the quote of before: {t}"
    );
    assert_eq!(hop_cuts(&c4), Vec::<Value>::new(), "no cut on a warm call");
    let c5 = call(&mut h, "t5", "It was.");
    let t = text(&c5);
    assert!(t.contains(RECALL_ID), "{t}");
    assert_eq!(t.matches(LATER).count(), 2, "{t}");
    assert_eq!(hop_cuts(&c5), Vec::<Value>::new(), "{}", c5.hop["cuts"]);
}

/// Pin: the rebuild collects it. After the strike of the clock the next
/// call carries the quoted words once, as the person's turn, and it is the
/// plan of the rebuild that says so -- the call itself cuts nothing.
#[test]
fn the_rebuild_collects_the_older_block() {
    if !shipped() {
        return;
    }
    let (mut h, _) = an_older_block();
    let order = h
        .clock
        .iter()
        .rev()
        .find(|m| m.body["op"] == "add")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "check of the setup: the clock holds an order: {:?}",
                h.clock
            )
        });
    h.fire(&order);
    let plan = h.rebuild_in(ROUND_E);
    assert!(
        plan.get("as_of_ms").is_some(),
        "check of the setup: the strike rebuilt the plan: {plan} {:?}",
        h.stderr
    );
    let c5 = call(&mut h, "t5", "It was.");
    let t = text(&c5);
    assert_eq!(
        hop_cuts(&c5),
        Vec::<Value>::new(),
        "the rebuild collected the block, the call cuts nothing: {t}"
    );
    assert_eq!(
        t.matches(LATER).count(),
        1,
        "once, as the person's turn: {t}"
    );
    assert_eq!(t.matches(SAID).count(), 1, "{t}");
}
