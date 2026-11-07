//! GH #1448 (D38 C2, R-38-4 Nr. 2) -- the plan holds no catalog number.
//!
//! WHY this file exists: stage 1 of the curator is model-free. What the
//! target model's catalog row says (`input_soft`, `input_hard`,
//! `chars_per_token`, prices, cache mode) is stage 2's, the cut per target
//! model (C3b). A block counts its tokens with the neutral estimate; so two
//! answers of the same words from models with different catalog rows leave
//! the same blocks, and no catalog key or value stands in the plan.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CATALOG_KEYS: [&str; 10] = [
    "input_soft",
    "input_hard",
    "chars_per_token",
    "cost_in",
    "cost_cached_in",
    "price_tiers",
    "cache_mode",
    "cache_ttl_s",
    "max_output",
    "wire_dialect",
];

fn keys_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                keys_of(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| keys_of(x, out)),
        _ => {}
    }
}

/// Two turns, answered by a model with the catalog row `row`.
fn blocks_under(row: Value) -> (Vec<Value>, Value) {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for (t, q, a) in [
        ("t1", "q1 about the garden", "a1"),
        ("t2", "q2", "a2 at length"),
    ] {
        let call = h.curate("s1", t, 0, json!([user(q)]), mode("Be brief."));
        let mut extra = row.clone();
        extra["tokens_prompt"] = json!(900);
        h.tap(&call, "stop", extra, json!([said(a)]));
    }
    (plan_blocks(&h, ROUND_E), h.plan_in(ROUND_E))
}

#[test]
fn the_blocks_are_the_same_under_any_catalog_row() {
    if !shipped() {
        return;
    }
    let (a, plan_a) = blocks_under(json!({"input_soft": 123_457, "input_hard": 765_431,
                                          "chars_per_token": 2.25}));
    let (b, _) = blocks_under(json!({"input_soft": 33_331, "input_hard": 77_771,
                                     "chars_per_token": 7.75}));
    assert!(!a.is_empty(), "the plan holds its blocks: {plan_a}");
    assert_eq!(a, b, "the catalog row moves no block");
    let mut keys = Vec::new();
    keys_of(&plan_a, &mut keys);
    for k in CATALOG_KEYS {
        assert!(
            !keys.iter().any(|x| x == k),
            "no catalog key `{k}`: {plan_a}"
        );
    }
    let text = canonical(&plan_a);
    for n in ["123457", "765431", "2.25"] {
        assert!(!text.contains(n), "no catalog value {n}: {text}");
    }
}

/// Two replayed days and the day rebuild between them, answered by a model
/// with the catalog row `row`: the blocks of the rebuilt plan.
fn rebuilt_under(row: Value) -> (Vec<Value>, Value) {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent_tokens", json!(0)),
        ("policy", "summary_form", json!("brief")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ]);
    for (t, at) in [
        ("d1", "2026-03-01T09:00:00Z"),
        ("d2", "2026-03-02T08:00:00Z"),
    ] {
        h.out.clear();
        let hop = json!({"session_id": "s", "turn_id": t, "iter": "0", "phase": ""});
        let ctx = json!({"session_id": "s", "turn_id": t, "iter": "0", "channel": "replay-1",
                         "audience_set": ROUND_E, "happened_at": at});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user(&format!("q at {at}"))]}),
        );
        let call = h.routed("brain").first().cloned().expect("one call");
        let mut extra = row.clone();
        extra["tokens_prompt"] = json!(900);
        h.out.clear();
        h.tap(&call, "stop", extra, json!([said(&format!("a {t}"))]));
    }
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the new day orders a rebuild");
    h.fire(&order);
    h.answer(
        "- person (2026-03-01) garden/state: in bloom [asserted]",
        "stop",
    );
    (plan_blocks(&h, ROUND_E), h.plan_in(ROUND_E))
}

/// Review M6: the blocks a rebuild writes (`rebuild_blocks`) are model free too.
#[test]
fn the_rebuilt_blocks_are_the_same_under_any_catalog_row() {
    if !shipped() {
        return;
    }
    let (a, plan_a) = rebuilt_under(json!({"input_soft": 123_457, "input_hard": 765_431,
                                           "chars_per_token": 2.25}));
    let (b, _) = rebuilt_under(json!({"input_soft": 133_331, "input_hard": 777_771,
                                      "chars_per_token": 7.75}));
    assert!(
        a.iter().any(|x| x["kind"] == json!("brief")),
        "the rebuild wrote its blocks: {plan_a}"
    );
    assert_eq!(a, b, "the catalog row moves no rebuilt block");
    let text = canonical(&json!(a));
    for n in ["123457", "765431", "2.25"] {
        assert!(!text.contains(n), "no catalog value {n}: {text}");
    }
}
