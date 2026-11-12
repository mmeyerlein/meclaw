//! GH #1463 (D58 form) -- a recall block is decided once, at the tail.
//!
//! WHY this file exists: `recall_in_wall` cut on every call. The plan kept
//! the block, the next call took it for a new one, checked it again and cut
//! it again, and a block that had stayed could leave any later call in the
//! middle of the window -- as soon as the person happened to say the quoted
//! words. Every such cut changes the prefix the model's cache holds: in the
//! measured run the cost of a call rose by 121 %, almost all of it cache
//! write. The rule of the form: the block of a round is decided ONCE, at the
//! first call after its round, where it stands at the tail of the window.
//! Left out, it leaves the plan with its call, the snapshot names it with
//! the form `cut` and the reason, and no later call cuts it again. Kept, it
//! stays until a rebuild, whatever the wall says later. The recall of the
//! running round stays, as before.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_0000000000formd1";
const SAID: &str = "My sister Anna moved to Hamburg.";
const LATER: &str = "We sold the old car last spring.";
const WHY: &str = "recall_in_wall";

fn result_of(quoted: &str) -> Value {
    tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{quoted}\""))
}

/// `t1` says [`SAID`], `t2` carries a `memory_recall` pair that quotes
/// `quoted` and is answered. Returns the hive and the call of `t2`.
fn two_turns(knob: &str, quoted: &str) -> (Hive, Msg) {
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!(knob)),
        ("policy", "snapshot_window", json!("1")),
    ]);
    turn(&mut h, "s1", "t1", SAID, "Hamburg is lovely.", json!({}));
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        result_of(quoted)
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("She likes it there.")]));
    (h, c2)
}

/// The first call of the turn `turn_id` with the words `words`.
fn call(h: &mut Hive, turn_id: &str, words: &str) -> Msg {
    h.curate("s1", turn_id, 0, json!([user(words)]), mode("Be brief."))
}

fn close(h: &mut Hive, c: &Msg, reply: &str) {
    h.tap(c, "stop", json!({}), json!([said(reply)]));
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

/// The `recall_in_wall` entries of the call's `hop.cuts`.
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

/// The `recall_in_wall` entries of `dropped` in the cut of the call's
/// snapshot row.
fn snapshot_drops(h: &Hive, c: &Msg) -> Vec<Value> {
    let id = c.hop["curator_call"].as_str().unwrap_or_default();
    let cut = h
        .rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r.first().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    let v: Value = sj::from_str(&cut).unwrap_or(Value::Null);
    v["dropped"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d["why"] == WHY)
        .collect()
}

/// Lock: the decision falls at the first call after the block's round. The
/// call of the round itself sends the pair, the next one does not, and its
/// hop says what it left out.
#[test]
fn the_block_is_decided_at_the_first_call_after_its_round() {
    if !shipped() {
        return;
    }
    let (mut h, c2) = two_turns("1", SAID);
    assert!(
        text(&c2).contains(RECALL_ID),
        "the recall of the running round stays: {}",
        text(&c2)
    );
    let c3 = call(&mut h, "t3", "And?");
    let t = text(&c3);
    assert_eq!(t.matches(SAID).count(), 1, "once, as the raw turn: {t}");
    assert!(!t.contains(RECALL_ID), "the pair goes whole: {t}");
    assert!(
        !hop_cuts(&c3).is_empty(),
        "the call that decides says so on its hop: {}",
        c3.hop["cuts"]
    );
}

/// Pin: the snapshot of the deciding call names the pair it left out -- the
/// result and its call, each with the form `cut` and the reason -- beside
/// the window it sent. Without the entries "was the context there?" has no
/// answer for a block that never reached the plan.
#[test]
fn the_snapshot_names_the_block_left_out_with_its_reason() {
    if !shipped() {
        return;
    }
    let (mut h, _) = two_turns("1", SAID);
    let c3 = call(&mut h, "t3", "And?");
    let sent = sent_of(&h, &c3).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(
        w.messages,
        c3.messages(),
        "the entries without `why` are the window sent"
    );
    let mut gone: Vec<(String, String, String)> = w
        .left_out
        .iter()
        .map(|(kind, why, el)| {
            assert_eq!(
                el["id"],
                json!(RECALL_ID),
                "the element left out is of the recall pair: {el}"
            );
            (
                kind.clone(),
                why.clone(),
                el["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    gone.sort();
    assert_eq!(
        gone,
        vec![
            (
                "recall".to_string(),
                WHY.to_string(),
                "tool_result".to_string()
            ),
            (
                "tool_call".to_string(),
                WHY.to_string(),
                "tool_call".to_string()
            ),
        ],
        "the result and its call, each `cut` with the reason: {sent:?}"
    );
    assert!(
        w.left_out.iter().any(|(_, _, el)| *el == result_of(SAID)),
        "the entry names the block of the wall, the bytes that were not sent: {sent:?}"
    );
}

/// Pin: a block left out is not cut again. It left the plan with the
/// decision, so the next call finds no recall block to decide on: no cut on
/// its hop, none in its snapshot, the window still without the pair.
#[test]
fn a_block_left_out_is_not_cut_again() {
    if !shipped() {
        return;
    }
    let (mut h, _) = two_turns("1", SAID);
    let c3 = call(&mut h, "t3", "And?");
    close(&mut h, &c3, "She moved in March.");
    let kept: Vec<Value> = plan_blocks(&h, ROUND_E)
        .into_iter()
        .filter(|b| b["kind"] == "recall")
        .collect();
    assert_eq!(
        kept,
        Vec::<Value>::new(),
        "the block left out is no block of the plan"
    );
    let c4 = call(&mut h, "t4", "Go on.");
    let t = text(&c4);
    assert_eq!(t.matches(SAID).count(), 1, "{t}");
    assert!(!t.contains(RECALL_ID), "{t}");
    assert_eq!(
        hop_cuts(&c4),
        Vec::<Value>::new(),
        "decided once: the later call cuts nothing"
    );
    assert_eq!(
        snapshot_drops(&h, &c4),
        Vec::<Value>::new(),
        "decided once: the later snapshot drops nothing"
    );
}

/// Pin: a block that stayed is not checked again. Its quote stood outside
/// the wall when the block was decided; two turns later the person says the
/// very words. The block was sent, so it stays where it is -- leaving now
/// would change the window in its middle. Only a rebuild collects it.
#[test]
fn a_block_that_stayed_is_not_checked_again() {
    if !shipped() {
        return;
    }
    let (mut h, _) = two_turns("1", LATER);
    let c3 = call(&mut h, "t3", "And?");
    assert!(
        text(&c3).contains(RECALL_ID),
        "check of the setup: a quote from outside the wall stays: {}",
        text(&c3)
    );
    close(&mut h, &c3, "I do not know more.");
    let c4 = call(&mut h, "t4", LATER);
    let t = text(&c4);
    assert!(
        t.contains(RECALL_ID),
        "the block was decided and sent, it stays: {t}"
    );
    assert_eq!(
        t.matches(LATER).count(),
        2,
        "the person's words and the quote of before: {t}"
    );
    assert_eq!(hop_cuts(&c4), Vec::<Value>::new(), "no cut on a warm call");
    close(&mut h, &c4, "A good price, I hope.");
    let c5 = call(&mut h, "t5", "It was.");
    let t = text(&c5);
    assert!(t.contains(RECALL_ID), "and it stays on the call after: {t}");
    assert_eq!(hop_cuts(&c5), Vec::<Value>::new(), "{}", c5.hop["cuts"]);
    assert_eq!(snapshot_drops(&h, &c5), Vec::<Value>::new());
}

/// Check of the setup: off, the pair of the quote in the wall stays on every
/// call.
#[test]
fn off_the_pair_stays() {
    if !shipped() {
        return;
    }
    let (mut h, _) = two_turns("0", SAID);
    let c3 = call(&mut h, "t3", "And?");
    assert!(text(&c3).contains(RECALL_ID), "{}", text(&c3));
    assert_eq!(text(&c3).matches(SAID).count(), 2);
}
