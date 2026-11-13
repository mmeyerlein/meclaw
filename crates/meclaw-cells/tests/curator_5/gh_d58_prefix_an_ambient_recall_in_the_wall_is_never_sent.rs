//! GH #1497 (D58 prefix) -- an ambient recall whose quotes stand in the wall
//! is never sent.
//!
//! WHY this file exists: the form decided a recall block only at the first
//! call after its round, so the pair the collector lays beside the person's
//! words (`ambient_of` "recall", no model call) always left with the call of
//! its own round, even when every quote already stood raw in the window. In
//! the measured run (S8, the brain call at k10/k30/k60) 83-100 % of the
//! recall quotes were such doubles, and `recall_in_wall` cut 0 blocks. The
//! exception "the recall of the running round stays" has one reason only: a
//! result the model fetched itself with `memory_recall` must stand in its
//! round, or the model calls it again. That reason does not touch the
//! ambient pair. So the ambient pair is decided at the first call that would
//! carry it, at the end of the window and before it was ever sent: every
//! quote raw in the wall -> the pair never reaches the plan (the snapshot
//! names it with the form `cut` and the reason); one quote off or missing ->
//! the whole pair stays. The decision holds for the round and after it,
//! until a rebuild. The model's own `memory_recall` keeps the old rule.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's id form: `call_recall_` plus 16 hex.
const AMBIENT_ID: &str = "call_recall_00000000000a1b2c";
/// A provider's id: a `memory_recall` the model called itself.
const OWN_ID: &str = "toolu_01OwnRecallOfTheModel";
const SAID: &str = "My sister Anna moved to Hamburg.";
const ALSO_SAID: &str = "Hamburg is lovely.";
const ELSEWHERE: &str = "We sold the old car last spring.";
const WHY: &str = "recall_in_wall";

fn quotes(lines: &[&str]) -> String {
    let mut s = "WHAT WAS SAID".to_string();
    for l in lines {
        s.push_str(&format!("\n  user: \"{l}\""));
    }
    s
}

/// The person's words of `t2` with a recall pair `id` quoting `quoted`.
fn with_recall(id: &str, quoted: &[&str]) -> Value {
    json!([
        user("Tell me about Anna."),
        tool_call(id, "memory_recall"),
        tool_result(id, &quotes(quoted))
    ])
}

/// A conversation with the form on: `t1` says [`SAID`] and is answered
/// [`ALSO_SAID`]; then the first call of `t2` brings `round`.
fn talk(knob: &str, round: Value) -> (Talk, Msg) {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!(knob)),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.turn("t1", SAID, ALSO_SAID);
    let c = t.call("t2", 0, round);
    (t, c)
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

/// The `recall_in_wall` entries of the call's `hop.cuts`.
fn hop_cuts(c: &Msg, why: &str) -> Vec<Value> {
    c.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == why)
        .collect()
}

/// `(kind, why, type)` of every element the call's snapshot lists as left
/// out, sorted.
fn left_out_of(h: &Hive, c: &Msg) -> Vec<(String, String, String)> {
    let sent = sent_of(h, c).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(h, &sent);
    assert_eq!(
        w.messages,
        c.messages(),
        "the entries without `why` are the window sent"
    );
    let mut gone: Vec<(String, String, String)> = w
        .left_out
        .iter()
        .map(|(kind, why, el)| {
            (
                kind.clone(),
                why.clone(),
                el["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    gone.sort();
    gone
}

fn the_pair(why: &str) -> Vec<(String, String, String)> {
    vec![
        (
            "recall".to_string(),
            why.to_string(),
            "tool_result".to_string(),
        ),
        (
            "tool_call".to_string(),
            why.to_string(),
            "tool_call".to_string(),
        ),
    ]
}

/// Pin: the ambient pair whose every quote stands raw in the wall is not
/// sent by the very first call that would carry it; the snapshot names both
/// elements with the form `cut` and the reason, the hop says the cut. The
/// next call of the round (a tool round) and the first call of the next
/// round begin byte-equal with the window before them, and neither sends
/// the pair.
#[test]
fn the_first_call_does_not_send_an_ambient_recall_already_in_the_wall() {
    if !shipped() {
        return;
    }
    let (mut t, c2) = talk("1", with_recall(AMBIENT_ID, &[SAID]));
    let tx = text(&c2);
    assert!(
        !tx.contains(AMBIENT_ID),
        "the first call of the round does not send the pair: {tx}"
    );
    assert_eq!(tx.matches(SAID).count(), 1, "once, as the raw turn: {tx}");
    assert_eq!(tx.matches(ALSO_SAID).count(), 1, "{tx}");
    assert_eq!(
        hop_cuts(&c2, WHY).len(),
        2,
        "the hop names the result and its call: {}",
        c2.hop["cuts"]
    );
    assert_eq!(
        left_out_of(&t.h, &c2),
        the_pair(WHY),
        "the snapshot names the pair, `cut` with the reason"
    );

    // The round goes on: a tool call of the model, the next call of `t2`.
    t.answer(
        &c2,
        "tool_calls",
        json!([tool_call("call_tool_0001", "read_file")]),
    );
    let c2b = t.call(
        "t2",
        1,
        json!([
            tool_call("call_tool_0001", "read_file"),
            tool_result("call_tool_0001", "file: nothing")
        ]),
    );
    assert!(!text(&c2b).contains(AMBIENT_ID), "{}", text(&c2b));
    assert_eq!(
        hop_cuts(&c2b, WHY),
        Vec::<Value>::new(),
        "decided once: the later call of the round cuts nothing"
    );
    t.answer(&c2b, "stop", json!([said("She likes it there.")]));
    let c3 = t.call("t3", 0, json!([user("And?")]));
    assert!(!text(&c3).contains(AMBIENT_ID), "{}", text(&c3));
    assert_eq!(text(&c3).matches(SAID).count(), 1, "{}", text(&c3));
    assert_eq!(hop_cuts(&c3, WHY), Vec::<Value>::new());
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "every window begins with the one before"
    );
    assert!(
        plan_blocks(&t.h, ROUND_E)
            .iter()
            .all(|b| b["kind"] != "recall"),
        "the pair left out is no block of the plan"
    );
}

/// Lock: all or nothing -- one quote outside the wall keeps the whole pair
/// on the first call, and it stays sent in the next round.
#[test]
fn an_ambient_recall_with_one_quote_outside_the_wall_stays_whole() {
    if !shipped() {
        return;
    }
    let (mut t, c2) = talk("1", with_recall(AMBIENT_ID, &[SAID, ELSEWHERE]));
    let tx = text(&c2);
    assert_eq!(
        tx.matches(AMBIENT_ID).count(),
        2,
        "the call and the result: {tx}"
    );
    assert!(tx.contains(ELSEWHERE), "{tx}");
    assert_eq!(hop_cuts(&c2, WHY), Vec::<Value>::new());
    t.answer(&c2, "stop", json!([said("She likes it there.")]));
    let c3 = t.call("t3", 0, json!([user("And?")]));
    assert_eq!(text(&c3).matches(AMBIENT_ID).count(), 2, "{}", text(&c3));
    assert_eq!(
        hop_cuts(&c3, WHY),
        Vec::<Value>::new(),
        "decided and kept: not checked again"
    );
    assert_eq!(t.breaks(), Vec::<String>::new());
}

/// Lock: the model's own `memory_recall` keeps the old rule -- it stays in
/// its round (the model sees its call and result) and is decided at the
/// first call after it.
#[test]
fn the_models_own_recall_stays_in_its_round() {
    if !shipped() {
        return;
    }
    let (mut t, c2) = talk("1", with_recall(OWN_ID, &[SAID]));
    let tx = text(&c2);
    assert_eq!(
        tx.matches(OWN_ID).count(),
        2,
        "the model sees its call and the result: {tx}"
    );
    assert_eq!(hop_cuts(&c2, WHY), Vec::<Value>::new());
    t.answer(&c2, "stop", json!([said("She likes it there.")]));
    let c3 = t.call("t3", 0, json!([user("And?")]));
    assert!(
        !text(&c3).contains(OWN_ID),
        "decided at the first call after its round: {}",
        text(&c3)
    );
    assert_eq!(left_out_of(&t.h, &c3), the_pair(WHY));
    assert_eq!(t.breaks(), Vec::<String>::new());
}

/// Lock: off, the ambient pair is sent as before.
#[test]
fn off_the_ambient_recall_is_sent() {
    if !shipped() {
        return;
    }
    let (_, c2) = talk("0", with_recall(AMBIENT_ID, &[SAID]));
    assert_eq!(text(&c2).matches(AMBIENT_ID).count(), 2, "{}", text(&c2));
    assert_eq!(text(&c2).matches(SAID).count(), 2, "{}", text(&c2));
}

/// The stamp keys of a judged `talk` turn: no `memory` among the needs, its
/// yes-value `yes_memory` (the form of `gh_d58_recall_*`).
fn judged(yes_memory: f64) -> Value {
    json!({"path": "talk", "needs": "", "families": "",
           "needs_yes": json!({"memory": yes_memory, "tool": 0.1}).to_string()})
}

/// Pin: `recall_skip_sure` and `recall_in_wall` both hold for one ambient
/// pair -- one decision, one line: the rule that checks first
/// (`recall_skip_sure`, where the pair arrives) takes it; the form books
/// nothing on the hop and nothing in the snapshot.
#[test]
fn skip_sure_and_the_form_book_an_ambient_pair_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "recall_skip_sure", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    turn(&mut h, "s1", "t1", SAID, ALSO_SAID, json!({}));
    let stamp = judged(0.05);
    let mut hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let mut ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                         "channel": "test", "audience_set": ROUND_E});
    for (k, v) in stamp.as_object().expect("the stamp keys are an object") {
        hop[k] = v.clone();
    }
    ctx["path"] = stamp["path"].clone();
    h.out.clear();
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": with_recall(AMBIENT_ID, &[SAID]), "system": mode("Be brief.")}),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let c2 = brain[0].clone();
    assert!(!text(&c2).contains(AMBIENT_ID), "{}", text(&c2));
    assert_eq!(
        left_out_of(&h, &c2),
        the_pair("recall_skip_sure"),
        "one line per element, one reason"
    );
    assert_eq!(
        hop_cuts(&c2, "recall_skip_sure").len(),
        2,
        "the result and its call, under the rule that checks first: {}",
        c2.hop["cuts"]
    );
    assert_eq!(
        hop_cuts(&c2, WHY),
        Vec::<Value>::new(),
        "no second booking by the form: {}",
        c2.hop["cuts"]
    );
}
