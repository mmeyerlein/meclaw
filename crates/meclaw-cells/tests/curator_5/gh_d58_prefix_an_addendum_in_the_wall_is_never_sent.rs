//! GH #1517 (d58rest, OR-54g-39 (d)) -- an addendum of `./push`
//! (`call_addendum_`) whose every quote stands raw in the wall is never sent.
//!
//! WHY this file exists: in S8 of the battery on 0.86.1, 15 of 15 recall
//! quotes stood twice in the window sent at k10 (rule: at most 5 %). The
//! ambient pair was left out by `recall_in_wall` (GH #1497), the doubles
//! stood in the addendum pair `call_addendum_<16 hex>` ("[addendum to your
//! last answer -- looked up after it, for: ...]"). `block_kind` makes every
//! element with an id of `PUSH_IDS` a `push` block and the candidates of
//! `recall_in_wall` (`iw_cand`) are recall blocks only, so the addendum was
//! never checked against the wall. Ruling OR-54g-39 (d): `recall_in_wall`
//! holds for the addendum too, all or nothing -- it is left out only when
//! every quote of it stands raw in the wall the call sends and its rows stand
//! in no plan, seen as `left_out` with the reason; the knob "0" sends it; the
//! candidates pair (`call_candidates_`, notes, no quotes) stays out of it.
//! The snapshot names both elements with the kind of their block, `push`
//! (OR-54g.d58rest.b3-1).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The id form of `./push` (`addendum_pair`): `call_addendum_` plus 16 hex.
const ADDENDUM_ID: &str = "call_addendum_00000000000d58a1";
/// The id form of the candidates pair (`candidate_pair`).
const CANDIDATES_ID: &str = "call_candidates_00000000000d58c1";
const SAID: &str = "The garden fence was painted green.";
const ALSO_SAID: &str = "Green suits the garden.";
const ELSEWHERE: &str = "The old bicycle was sold last spring.";
const WHY: &str = "recall_in_wall";

/// The text of an addendum quoting `lines` (the frame of `./push`, the rows
/// of the memory hive).
fn addendum(lines: &[&str]) -> String {
    let mut s = "[addendum to your last answer -- looked up after it, for: the fence]\n\
                 WHAT WAS SAID"
        .to_string();
    for l in lines {
        s.push_str(&format!("\n  user: \"{l}\""));
    }
    s
}

/// The person's words of `t2` with the pair `id` carrying `text`.
fn with_pair(id: &str, text: &str) -> Value {
    json!([
        user("What colour was the fence?"),
        tool_call(id, "memory_recall"),
        tool_result(id, text)
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

/// The `why` entries of the call's `hop.cuts`.
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
/// out, sorted. The kind of both elements of the addendum is `push`, the
/// kind `block_kind` gives every element of `PUSH_IDS` (OR-54g.d58rest.b3-1:
/// the snapshot names the block the plan holds, no kind of its own).
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
        ("push".to_string(), why.to_string(), "tool_call".to_string()),
        (
            "push".to_string(),
            why.to_string(),
            "tool_result".to_string(),
        ),
    ]
}

/// Pin: the addendum whose every quote stands raw in the wall is not sent
/// by the first call that would carry it; the snapshot names its call and
/// its result as left out with the reason, the hop says the cut. The next
/// call of the round and the first call of the next round begin byte-equal
/// with the window before them, cut nothing more and never send the pair.
#[test]
fn the_first_call_does_not_send_an_addendum_already_in_the_wall() {
    if !shipped() {
        return;
    }
    let (mut t, c2) = talk("1", with_pair(ADDENDUM_ID, &addendum(&[SAID])));
    let tx = text(&c2);
    assert!(
        !tx.contains(ADDENDUM_ID),
        "the first call of the round does not send the addendum: {tx}"
    );
    assert_eq!(tx.matches(SAID).count(), 1, "once, as the raw turn: {tx}");
    assert_eq!(
        hop_cuts(&c2, WHY).len(),
        2,
        "the hop names the result and its call: {}",
        c2.hop["cuts"]
    );
    assert_eq!(
        left_out_of(&t.h, &c2),
        the_pair(WHY),
        "the snapshot names the pair as left out, with the reason"
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
    assert!(!text(&c2b).contains(ADDENDUM_ID), "{}", text(&c2b));
    assert_eq!(
        hop_cuts(&c2b, WHY),
        Vec::<Value>::new(),
        "decided once: the later call of the round cuts nothing"
    );
    t.answer(&c2b, "stop", json!([said("It was green.")]));
    let c3 = t.call("t3", 0, json!([user("And the gate?")]));
    assert!(!text(&c3).contains(ADDENDUM_ID), "{}", text(&c3));
    assert_eq!(text(&c3).matches(SAID).count(), 1, "{}", text(&c3));
    assert_eq!(hop_cuts(&c3, WHY), Vec::<Value>::new());
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "every window begins with the one before"
    );
}

/// Lock: all or nothing -- one quote outside the wall keeps the whole
/// addendum, on the first call and, decided, in the next round.
#[test]
fn an_addendum_with_one_quote_outside_the_wall_stays_whole() {
    if !shipped() {
        return;
    }
    let (mut t, c2) = talk("1", with_pair(ADDENDUM_ID, &addendum(&[SAID, ELSEWHERE])));
    let tx = text(&c2);
    assert_eq!(
        tx.matches(ADDENDUM_ID).count(),
        2,
        "the call and the result: {tx}"
    );
    assert!(tx.contains(ELSEWHERE), "{tx}");
    assert_eq!(hop_cuts(&c2, WHY), Vec::<Value>::new());
    t.answer(&c2, "stop", json!([said("It was green.")]));
    let c3 = t.call("t3", 0, json!([user("And the gate?")]));
    assert_eq!(text(&c3).matches(ADDENDUM_ID).count(), 2, "{}", text(&c3));
    assert_eq!(hop_cuts(&c3, WHY), Vec::<Value>::new());
    assert_eq!(t.breaks(), Vec::<String>::new());
}

/// Lock: off, the addendum is sent as before.
#[test]
fn off_the_addendum_is_sent() {
    if !shipped() {
        return;
    }
    let (_, c2) = talk("0", with_pair(ADDENDUM_ID, &addendum(&[SAID])));
    assert_eq!(text(&c2).matches(ADDENDUM_ID).count(), 2, "{}", text(&c2));
    assert_eq!(text(&c2).matches(SAID).count(), 2, "{}", text(&c2));
}

/// Lock: the candidates pair is no quote of the person -- it is never cut by
/// `recall_in_wall`, even when its notes repeat words of the wall.
#[test]
fn the_candidates_pair_is_never_cut() {
    if !shipped() {
        return;
    }
    let notes = format!(
        "[addendum -- notes handed to this conversation for this round; not said by the person]\n\
         WHAT WAS SAID\n  user: \"{SAID}\""
    );
    let (mut t, c2) = talk("1", with_pair(CANDIDATES_ID, &notes));
    assert_eq!(text(&c2).matches(CANDIDATES_ID).count(), 2, "{}", text(&c2));
    assert_eq!(hop_cuts(&c2, WHY), Vec::<Value>::new());
    t.answer(&c2, "stop", json!([said("It was green.")]));
    let c3 = t.call("t3", 0, json!([user("And the gate?")]));
    assert_eq!(hop_cuts(&c3, WHY), Vec::<Value>::new(), "{}", text(&c3));
}
