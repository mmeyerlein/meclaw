//! GH #1450 (D38 C3b, R-38-4) -- two models get two windows from one plan.
//!
//! WHY this file exists: the owner's picture (R-38-4): "qwen gets another
//! window than haiku, and than luna or sol". Stage 1 builds ONE plan; stage 2
//! cuts it per target model out of that model's catalogue row. Two claims:
//!
//! 1. The same turn, the same way, two rows: a model with a wide window and a
//!    prefix cache of breakpoints gets every block; a model with a narrow
//!    hard line and no cache of its own gets the newest rounds only -- the
//!    rest named as dropped -- and no cache boundary.
//! 2. The shipped pairing: the brain's row (a Haiku-like one) and the quick
//!    voice's row (a mimo-like one) each size their own calls.
//!
//! Rows are invented (the numbers of no shipped catalogue); runs the shipped
//! curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

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

fn row(model: &str, soft: i64, hard: i64, cache_mode: &str, ttl: i64) -> Value {
    json!({"model": model, "input_soft": soft, "input_hard": hard, "max_output": 4096,
           "price_tiers": null, "cache_mode": cache_mode, "cache_ttl_s": ttl,
           "wire_dialect": "chat_completions", "supported_params": [],
           "chars_per_token": 3.5})
}

fn haiku_like() -> Value {
    row("test/haiku-like", 200000, 200000, "breakpoints", 300)
}

/// A hard line below the history of the turn: only the `keep_recent`
/// newest rounds fit. No cache mode: the provider caches nothing.
fn mimo_like() -> Value {
    row("test/mimo-like", 300, 400, "", 0)
}

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

/// Six rounds of talk with words of some length, then the seventh call --
/// in a hive whose brain row is `brain_row`; the same knobs for every hive.
fn seventh_brain_call(brain_row: Value) -> (Hive, Msg) {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(2)),
    ]);
    catalog(&mut h, "brain", brain_row);
    let long = "the person tells a long story about the garden and the weather ".repeat(4);
    for i in 0..6 {
        turn_on(
            &mut h,
            &format!("t{i}"),
            "talk",
            "brain",
            &format!("{i}: {long}"),
            &format!("answer {i}: {long}"),
        );
    }
    let call = turn_on(&mut h, "t6", "talk", "brain", "and now?", "now this.");
    (h, call)
}

#[test]
fn one_plan_two_rows_two_windows() {
    if !shipped() {
        return;
    }
    let (wide, w_call) = seventh_brain_call(haiku_like());
    let (narrow, n_call) = seventh_brain_call(mimo_like());
    let w = cut_of(&wide, &w_call).expect("the cut of the wide model's call");
    let n = cut_of(&narrow, &n_call).expect("the cut of the narrow model's call");
    assert_eq!(
        block_ids(&snapshot_of(&wide, &w_call).expect("plan")),
        block_ids(&snapshot_of(&narrow, &n_call).expect("plan")),
        "stage 1 is model-free: one plan for both"
    );
    assert_ne!(w["ids"], n["ids"], "two rows, two windows: {w} / {n}");
    assert!(
        w["dropped"].as_array().is_some_and(|d| d.is_empty()),
        "the wide window drops nothing: {w}"
    );
    assert!(
        n["dropped"].as_array().is_some_and(|d| !d.is_empty()),
        "the narrow window names the rounds it left out: {n}"
    );
    assert!(
        n_call.messages().len() < w_call.messages().len(),
        "the narrow call carries fewer messages"
    );
    assert!(w["cache_at"].is_string(), "breakpoints: a boundary: {w}");
    assert!(n["cache_at"].is_null(), "no cache mode: no boundary: {n}");
}

#[test]
fn the_brain_and_the_quick_voice_are_cut_for_their_own_rows() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    catalog(&mut h, "brain", haiku_like());
    catalog(
        &mut h,
        "quick",
        row("test/mimo-like", 60000, 90000, "implicit", 0),
    );
    let b = turn_on(
        &mut h,
        "t0",
        "talk",
        "brain",
        "tell me about the garden",
        "It grows.",
    );
    let q = turn_on(
        &mut h,
        "t1",
        "simple",
        "quick",
        "What day is it?",
        "Monday.",
    );
    let bc = cut_of(&h, &b).expect("the cut of the brain call");
    let qc = cut_of(&h, &q).expect("the cut of the quick call");
    assert_eq!(bc["target"], json!("brain"), "{bc}");
    assert_eq!(bc["model"], json!("test/haiku-like"), "{bc}");
    assert_eq!(bc["budget"]["soft"], json!(200000), "{bc}");
    assert_eq!(qc["target"], json!("quick"), "{qc}");
    assert_eq!(qc["model"], json!("test/mimo-like"), "{qc}");
    assert_eq!(qc["budget"]["soft"], json!(60000), "{qc}");
    assert_eq!(qc["cache_mode"], json!("implicit"), "{qc}");
}
