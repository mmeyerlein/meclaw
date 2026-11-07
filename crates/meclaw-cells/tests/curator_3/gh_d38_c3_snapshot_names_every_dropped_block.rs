//! GH #1450 (D38 C3b, R-38-4 Nr. 5, R-38-3) -- the snapshot names the cut.
//!
//! WHY this file exists: "was the context there?" is a lookup (R-38-3). Part 1
//! of the snapshot (GH #1448) lists the plan of a call; part 2 lists what
//! stage 2 made of it for the target model: the block ids it sends, in
//! their order, the budget it cut to, the cache boundary, every block of the
//! plan it left out with the reason, and the time the cut took (`cut_ms`,
//! R-38-4 Nr. 6: latency per station). A block the window lacks and the
//! snapshot does not name would be a silent cut (R-IG-1).
//!
//! The cut sits beside the plan in the call's snapshot row (column `cut`,
//! canonical JSON), read through the hive's one audience-gated reader.
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use std::collections::BTreeSet;

fn cut_of(h: &Hive, call: &Msg) -> Option<Value> {
    let has = h.rows("SELECT name FROM pragma_table_info('snapshots') WHERE name = 'cut'");
    if has.is_empty() {
        return None;
    }
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    let text = rows.first()?.first()?.as_str()?.to_string();
    sj::from_str::<Value>(&text).ok()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect()
}

/// A catalogue row in the form of the `in_catalog` lane (GH #1449).
fn row(model: &str, soft: i64, hard: i64, cache_mode: &str, ttl: i64) -> Value {
    json!({"model": model, "input_soft": soft, "input_hard": hard, "max_output": 4096,
           "price_tiers": null, "cache_mode": cache_mode, "cache_ttl_s": ttl,
           "wire_dialect": "chat_completions", "supported_params": [],
           "chars_per_token": 3.5})
}

/// The package line of the model cell `cell` of the parent, on `in_catalog`.
fn catalog(h: &mut Hive, cell: &str, row: Value) {
    h.out.clear();
    h.lane(
        "in_catalog",
        json!({}),
        json!({"package_of": format!("/talky/{cell}")}),
        json!({"messages": [], "package": row}),
    );
}

fn turn_on(h: &mut Hive, turn: &str, path: &str, way: &str, ask: &str, reply: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "chat", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    let call = h
        .routed(way)
        .into_iter()
        .find(|m| {
            m.hop.get("bridge").is_none()
                && !m.hop["curator_call"]
                    .as_str()
                    .unwrap_or("")
                    .ends_with("-bridge")
        })
        .unwrap_or_else(|| panic!("a call on {way}: {:?} {:?}", h.out, h.stderr));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

#[test]
fn the_cut_names_its_ids_budget_cache_boundary_and_time() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    catalog(
        &mut h,
        "brain",
        row("test/row-a", 120000, 180000, "breakpoints", 300),
    );
    turn_on(
        &mut h,
        "t0",
        "talk",
        "brain",
        "first question",
        "first answer",
    );
    let call = turn_on(
        &mut h,
        "t1",
        "talk",
        "brain",
        "second question",
        "second answer",
    );
    let cut = cut_of(&h, &call)
        .expect("the snapshot names the cut of the call (column `cut` of `snapshots`)");
    let ids = strs(&cut["ids"]);
    assert!(
        ids.contains(&block_id("words", &user("second question"))),
        "the cut lists the blocks it sends: {cut}"
    );
    assert_eq!(cut["target"], json!("brain"), "{cut}");
    assert_eq!(cut["model"], json!("test/row-a"), "{cut}");
    assert_eq!(
        cut["budget"]["soft"],
        json!(120000),
        "the budget is the row's: {cut}"
    );
    let hard = cut["budget"]["hard"].as_i64().unwrap_or(0);
    assert!(
        hard > 0 && hard <= 180000,
        "the cut line comes from the row's input_hard: {cut}"
    );
    assert_eq!(cut["cache_mode"], json!("breakpoints"), "{cut}");
    assert_eq!(cut["cache_ttl_s"], json!(300), "{cut}");
    let at = cut["cache_at"].as_str().unwrap_or_default().to_string();
    assert!(
        ids.contains(&at),
        "a cache boundary on a block the cut sends: {cut}"
    );
    assert!(
        cut["dropped"].as_array().is_some_and(|d| d.is_empty()),
        "a window under its budget drops nothing: {cut}"
    );
    assert!(
        cut["cut_ms"].as_f64().is_some_and(|ms| ms >= 0.0),
        "the station writes its time: {cut}"
    );
}

#[test]
fn every_block_of_the_plan_is_sent_or_named_with_its_reason() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..5 {
        turn_on(
            &mut h,
            &format!("t{i}"),
            "talk",
            "brain",
            &format!("question {i}"),
            &format!("answer {i}"),
        );
    }
    // The quick voice keeps its `keep_recent` newest elements: the older
    // rounds of the plan leave this window.
    let call = turn_on(
        &mut h,
        "t9",
        "simple",
        "quick",
        "What day is it?",
        "Monday.",
    );
    let plan: BTreeSet<String> = block_ids(&snapshot_of(&h, &call).expect("the plan"))
        .into_iter()
        .collect();
    let cut = cut_of(&h, &call).expect("the snapshot names the cut of the quick call");
    let sent: BTreeSet<String> = strs(&cut["ids"]).into_iter().collect();
    let dropped = cut["dropped"].as_array().cloned().unwrap_or_default();
    assert!(
        !dropped.is_empty(),
        "the quick window leaves old rounds out -- and names them: {cut}"
    );
    let mut named = BTreeSet::new();
    for d in &dropped {
        let id = d["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !d["why"].as_str().unwrap_or_default().is_empty(),
            "each dropped block carries its reason: {d}"
        );
        assert!(!sent.contains(&id), "dropped and sent at once: {id}");
        named.insert(id);
    }
    assert!(
        named.contains(&block_id("words", &user("question 0"))),
        "the oldest words are named as dropped: {cut}"
    );
    // GH #1450 Z1.8: the tool scope is the cut's -- the snapshot names the
    // tools the call may use, the ones its `tool_scope` allows.
    let allow = call.body["tool_scope"]["allow"].clone();
    assert!(
        allow.is_array(),
        "a quick call carries its scope: {:?}",
        call.body
    );
    assert_eq!(
        cut["tools"], allow,
        "the cut names the tools the call may use: {cut}"
    );
    let all: BTreeSet<String> = sent.union(&named).cloned().collect();
    assert_eq!(
        all, plan,
        "every plan block is either sent or named as dropped, nothing else"
    );
}

#[test]
fn the_reader_hands_the_cut_on() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let call = turn_on(&mut h, "t0", "talk", "brain", "q", "a");
    h.out.clear();
    h.lane(
        "in_read",
        json!({"audience_set": ROUND_E}),
        json!({"read_tag": "cut-1"}),
        json!({"what": "snapshots"}),
    );
    let read = h.routed("read");
    assert_eq!(read.len(), 1, "one answer: {:?} {:?}", h.out, h.stderr);
    let rows = read[0].body["rows"].as_array().cloned().unwrap_or_default();
    let mine = rows
        .iter()
        .find(|r| r["call_id"] == call.hop["curator_call"])
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        mine["cut"],
        cut_of(&h, &call).unwrap_or(Value::Null),
        "the reader hands the cut on as stored: {mine}"
    );
    assert!(mine["cut"].is_object(), "a cut to hand on: {mine}");
}
