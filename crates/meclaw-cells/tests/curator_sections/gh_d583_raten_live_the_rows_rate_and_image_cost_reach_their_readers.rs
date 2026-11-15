//! GH #1532 (d58-3 raten, review I1/I2) -- the rate and the image cost of a
//! model's catalog row reach the cells that size by its window, on the road a
//! colony runs, with no stamp built by hand.
//!
//! WHY this file exists: the strand taught the memory hive, the producers,
//! the route and the start window to read `chars_per_token` and
//! `tokens_per_image` beside the window. Its pins put both keys on the
//! message by hand. Live, nobody set them: the `llm` cell stamped the window
//! (`input_soft`) but not the rate and the image cost of the same row, the
//! curator kept only the window for the memory ask, the edges lifted only
//! the window into the memory hive's context, the view answer named no image
//! cost, and a start window without `hop.limits` never took its share of a
//! window. Every reader therefore counted at the core's 3 characters a token
//! and 1024 tokens an image (review of strand raten, Important 1 and 2).
//!
//! Here a real `llm` cell (its provider a mock) with the row rate 4 / image
//! cost 1600 answers; its header goes into the curator as the tap's usage
//! as it came; the curator's outputs cross the shipped edges (the colony's
//! own CEL evaluator). The values arrive in the context of the memory ask,
//! in the context of the model call (where the model's own `memory_recall`
//! and the tools behind the dispatcher take them from), on the view answer
//! and in the start window.
//!
//! Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;
// harness_fold: #[path = "support/row_stamp.rs"]
#[allow(unused_imports)]
use crate::row_stamp; // harness_fold: mod row_stamp;

use curator_hive::*;
use curator_live::{LIVE, VOICE_SESSION, live_hive};
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use row_stamp::{ROW_IMAGE, ROW_RATE, ROW_SOFT, real_stamp, row};

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn holds(edge: &Value, h: &Headers) -> bool {
    let Some(c) = edge["condition"].as_str() else {
        return true;
    };
    let cond = parse_condition(c).expect("the condition compiles");
    evaluate_condition(&cond, &h.context, &h.hop).unwrap_or(false)
}

/// `h` crosses the one shipped edge `from -> to` of `template` that takes it
/// (a `default` edge only when no other edge out of `from` does).
fn cross(template: &str, from: &str, to: &str, h: &Headers) -> Headers {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let taken: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == json!(from) && holds(e, h))
        .collect();
    let any_plain = taken.iter().any(|e| e["default"] != json!(true));
    let hit: Vec<&&Value> = taken
        .iter()
        .filter(|e| e["to"] == json!(to) && (!any_plain || e["default"] != json!(true)))
        .collect();
    assert_eq!(
        hit.len(),
        1,
        "{template}: one edge {from} -> {to} takes {h:?}: {hit:?}"
    );
    if hit[0]["modifier"].is_null() {
        return h.clone();
    }
    let spec: ModifierSpec = sj::from_value(hit[0]["modifier"].clone()).expect("modifier spec");
    let m = parse_modifier(&spec).expect("the modifier compiles");
    apply_modifier(&m, h).expect("the modifier evaluates")
}

fn outer() -> Map<String, Value> {
    map(json!({"audience_set": ROUND_E, "channel": "test", "session_id": "s1"}))
}

/// The row's rate and image cost stand in `m`.
fn carries_the_row(m: &Map<String, Value>, what: &str) {
    assert_eq!(
        num(m.get("chars_per_token")),
        Some(ROW_RATE),
        "{what}: the row's rate: {m:?}"
    );
    assert_eq!(
        num(m.get("tokens_per_image")),
        Some(ROW_IMAGE as f64),
        "{what}: the row's image cost: {m:?}"
    );
}

/// A talky curator that holds the brain's row (its package line) and whose
/// first round the real answer closed.
fn answered(stamp: &Map<String, Value>) -> Hive {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.report_row("talky", "brain", row());
    let call = h.curate("s1", "t1", 0, json!([user("Who is my son?")]), mode(""));
    h.tap(
        &call,
        "stop",
        Value::Object(stamp.clone()),
        json!([said("noted")]),
    );
    h
}

fn the_roads(stamp: Map<String, Value>) {
    // 0. The stamp itself: the row's rate and image cost beside its window.
    assert_eq!(
        num(stamp.get("input_soft")),
        Some(ROW_SOFT as f64),
        "{stamp:?}"
    );
    carries_the_row(&stamp, "the llm cell's answer");

    // 1. The collector's memory ask: `./push` -> the member's edge into the
    //    memory hive, whose `./recall` and `./tool` read the context only.
    let mut h = answered(&stamp);
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": "t2", "session_id": "s1", "iter": "0",
               "recall_query": "And what does he do?", "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user("And what does he do?")]}),
    );
    let asks = h.routed("recall");
    assert_eq!(asks.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let at = cross(
        "member",
        "./assistants",
        "./memory-hive",
        &Headers::from_parts(outer(), asks[0].hop.clone()),
    );
    carries_the_row(&at.context, "the memory ask's context");

    // 2. The next model call: the composite lifts the row into the call's
    //    context, where the model's own `memory_recall` takes it from.
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("And what does he do?")]),
        mode(""),
    );
    let at = cross(
        "talky",
        "./curator",
        "./brain",
        &Headers::from_parts(outer(), call.hop.clone()),
    );
    carries_the_row(&at.context, "the model call's context");

    // 3. The view of a target whose row the curator holds names its image
    //    cost (`talky/route` counts the decide window's images with it).
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t3", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "brain", "turns": 2}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        num(views[0].body.get("tokens_per_image")),
        Some(ROW_IMAGE as f64),
        "the view carries the image cost of its target's row: {:?}",
        views[0].body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d583_raten_live_a_real_answer_carries_the_row_to_every_reader() {
    if !shipped() {
        return;
    }
    let stamp = real_stamp().await;
    tokio::task::spawn_blocking(move || the_roads(stamp))
        .await
        .expect("the roads hold");
}

/// `n` exchanges of session `s1`, about a hundred tokens a row.
fn talk(h: &mut Hive, n: i64) {
    for i in 0..n {
        let words = format!("{i} {}", "the roses by the old garden wall ".repeat(9));
        h.row(
            backdated(50 - 2 * i),
            "s1",
            &format!("t{i}"),
            "user",
            &user(&words),
            1,
        );
        h.row(
            backdated(49 - 2 * i),
            "s1",
            &format!("t{i}"),
            "assistant",
            &said(&words),
            1,
        );
    }
}

/// A start window asked without `hop.limits`: its target (`seed_target`)
/// has a row of 6000 tokens, and the shipped share 0.032768 of it is 196
/// tokens -- at most one row beside the fact line, where the fallback 8192
/// held all six exchanges.
#[test]
fn gh_d583_raten_live_a_start_window_without_limits_takes_its_share_of_the_targets_row() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = live_hive(&[("history", "seed_target", json!("brain"))]);
    h.report_row(
        "talky",
        "brain",
        json!({"model": "test/small", "input_soft": 6000, "input_hard": 12000,
               "chars_per_token": 3, "tokens_per_image": ROW_IMAGE}),
    );
    talk(&mut h, 6);
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"audience_set": ROUND_E, "live": LIVE}),
        json!({"session_id": VOICE_SESSION, "reason": "start", "renewal_n": 0, "live": LIVE}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let turns = seeds[0]
        .messages()
        .iter()
        .filter(|m| m["role"] != json!("developer"))
        .count();
    let est = seeds[0]
        .body
        .get("est_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    assert!(
        turns <= 1 && (0..=196).contains(&est),
        "the start window is the share of the target's row: {turns} turns, {est} tokens {:?}",
        h.stderr
    );
}
