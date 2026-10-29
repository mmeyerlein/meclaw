//! GH #1347 -- the quick window keeps the running round whole.
//!
//! WHY this file exists: the person asked "How is the weather right now in
//! London, Paris and Madrid?", the quick voice called `weather` three times
//! side by side, and the formulation round went out with the last four
//! elements only (role `quick`, `keep_recent` 4, counted in elements): one
//! call (Madrid) and the three results -- no question, and two results
//! without their call. The answer named one or two cities; with the whole
//! window the same model named all three (a replay of the formulation
//! request). The rule this file pins: the running round goes
//! out whole (the person's words and every call/result pair), `keep_recent`
//! counts what lies before it, and no call/result pair is cut at the
//! boundary of the window, in older rounds neither. The rows here are
//! synthetic and built after the form of that request, no conversation data.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "How is the weather right now in London, Paris and Madrid?";

fn seam(h: &mut Hive, turn_id: &str, iter: u32, messages: Value) {
    h.out.clear();
    let it = iter.to_string();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": it, "phase": "",
                     "path": "simple", "families": "weather"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": it,
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

fn kind(el: &Value) -> &str {
    el["type"].as_str().unwrap_or("")
}

fn id(el: &Value) -> &str {
    el["id"].as_str().unwrap_or("")
}

/// Every tool result stands after its own call, and every call has its
/// result: a pair is never cut in two.
fn pairs_whole(msgs: &[Value]) -> Result<(), String> {
    for (i, el) in msgs.iter().enumerate() {
        if kind(el) == "tool_result"
            && !msgs[..i]
                .iter()
                .any(|c| kind(c) == "tool_call" && id(c) == id(el))
        {
            return Err(format!("result {} without its call", id(el)));
        }
        if kind(el) == "tool_call"
            && !msgs[i..]
                .iter()
                .any(|r| kind(r) == "tool_result" && id(r) == id(el))
        {
            return Err(format!("call {} without its result", id(el)));
        }
    }
    Ok(())
}

#[test]
fn the_formulation_round_keeps_the_question_and_every_pair() {
    if !shipped() {
        return;
    }
    let (keep, _) = policy_scope(json!({}), "ROLES['quick']['keep_recent']", Value::Null);
    assert_eq!(
        keep,
        json!(4),
        "the preset stays as it is: the fix is not a value"
    );

    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..4 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            &format!("question {i}"),
            &format!("answer {i}"),
            json!({}),
        );
    }
    seam(&mut h, "t9", 0, json!([user(ASK)]));
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    // The tool round's answer: three calls side by side, the results in the
    // order they came back (Paris, Madrid, London), as in the replay.
    seam(
        &mut h,
        "t9",
        1,
        json!([
            tool_call("call_lon", "weather"),
            tool_call("call_par", "weather"),
            tool_call("call_mad", "weather"),
            tool_result("call_par", "Paris: cloudy, 14 degrees."),
            tool_result("call_mad", "Madrid: sunny, 24 degrees."),
            tool_result("call_lon", "London: rain, 11 degrees."),
        ]),
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "the formulation round goes on quick: {:?} {:?}",
        h.out,
        h.stderr
    );
    let msgs: Vec<Value> = quick[0]
        .messages()
        .into_iter()
        .filter(|el| el["origin"] != json!("system"))
        .collect();
    let ask_at = msgs
        .iter()
        .position(|el| el["text"].as_str().is_some_and(|t| t.contains(ASK)))
        .unwrap_or_else(|| panic!("the person's question is in the window: {msgs:?}"));
    let calls: Vec<&str> = msgs[ask_at..]
        .iter()
        .filter(|el| kind(el) == "tool_call")
        .map(id)
        .collect();
    assert_eq!(
        calls,
        vec!["call_lon", "call_par", "call_mad"],
        "every call of the running round is in the window: {msgs:?}"
    );
    let results = msgs[ask_at..]
        .iter()
        .filter(|el| kind(el) == "tool_result")
        .count();
    assert_eq!(results, 3, "every result of the running round: {msgs:?}");
    pairs_whole(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:?}"));
    // `keep_recent` counts what lies before the running round: four
    // elements of the earlier rounds, the newest ones.
    let before: Vec<&str> = msgs[..ask_at]
        .iter()
        .map(|el| el["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        before.len(),
        4,
        "four elements before the running round: {before:?}"
    );
    assert!(
        before[0].ends_with("question 2") && before[3].ends_with("answer 3"),
        "the newest four before the round: {before:?}"
    );
}

/// The wall rows of `items`: position, round.
fn wall_of(rounds: &[(i64, &str)]) -> Value {
    Value::Array(
        rounds
            .iter()
            .map(|(seq, t)| json!({"seq": seq, "session_id": "s1", "turn_id": t, "kind": ""}))
            .collect(),
    )
}

fn items_of(els: &[(i64, Value)]) -> Value {
    Value::Array(
        els.iter()
            .map(|(seq, el)| json!([seq, el, "", false]))
            .collect(),
    )
}

fn quick_items(items: Value, wall: Value, keep: i64) -> Vec<Value> {
    let (got, err) = policy_scope(
        json!({}),
        "[x[1] for x in quick_items(ARGS['items'], ARGS['wall'], ARGS['keep'])]",
        json!({"items": items, "wall": wall, "keep": keep}),
    );
    got.as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a list: {got:?} {err}"))
}

#[test]
fn an_older_pair_at_the_boundary_stays_whole_or_goes_whole() {
    if !shipped() {
        return;
    }
    // An older round with two calls side by side, then the running round.
    let els = vec![
        (1, user("older question")),
        (2, tool_call("call_a", "weather")),
        (3, tool_call("call_b", "weather")),
        (4, tool_result("call_a", "a")),
        (5, tool_result("call_b", "b")),
        (6, said("older answer")),
        (7, user("new question")),
    ];
    let wall = wall_of(&[
        (1, "t1"),
        (2, "t1"),
        (3, "t1"),
        (4, "t1"),
        (5, "t1"),
        (6, "t1"),
        (7, "t2"),
    ]);
    for keep in 1..=6 {
        let got = quick_items(items_of(&els), wall.clone(), keep);
        pairs_whole(&got).unwrap_or_else(|e| panic!("keep {keep}: {e}: {got:?}"));
        assert!(
            got.last()
                .is_some_and(|el| el["text"] == json!("new question")),
            "keep {keep}: the running round is in the window: {got:?}"
        );
        let older = got.len() - 1;
        assert!(
            older >= keep as usize,
            "keep {keep}: at least `keep_recent` elements before the round: {got:?}"
        );
    }
    // The cut lands between the two calls: both pairs stay, whole.
    let got = quick_items(items_of(&els), wall.clone(), 4);
    let ids: Vec<&str> = got.iter().map(id).filter(|i| !i.is_empty()).collect();
    assert_eq!(
        ids,
        vec!["call_a", "call_b", "call_a", "call_b"],
        "both pairs whole: {got:?}"
    );
    // A large `keep_recent` keeps everything as it stood.
    let got = quick_items(items_of(&els), wall, 40);
    assert_eq!(got.len(), els.len(), "nothing cut: {got:?}");
}

#[test]
fn the_research_bridge_counts_before_its_running_round() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..4 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            &format!("question {i}"),
            &format!("answer {i}"),
            json!({}),
        );
    }
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t9", "iter": "0", "phase": "",
                     "path": "research", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t9", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "research"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("find everything on the topic")]}),
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "one bridge call: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        quick[0].hop["bridge"],
        json!("research"),
        "{:?}",
        quick[0].hop
    );
    // The bridge's quick window is cut by the call's own round: four
    // elements before it, then the running turn.
    let texts: Vec<String> = quick[0]
        .messages()
        .into_iter()
        .filter(|el| el["origin"] != json!("system"))
        .map(|el| el["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(texts.len(), 5, "four before the running turn: {texts:?}");
    assert!(
        texts[0].ends_with("question 2") && texts[4].contains("find everything on the topic"),
        "the newest four, then the turn: {texts:?}"
    );
}
