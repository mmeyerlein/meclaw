//! GH #1225 (K-3, OR-KF-7) -- without a model package the curator still
//! rebuilds.
//!
//! An answer of a model no llm-registry push reached names no `input_soft`,
//! no `context_window` and no `cache_expires_at`: before, no line was crossed
//! and no clock armed, so the window was never rebuilt. Now the catalogue row
//! the summarizer falls back on (`input_soft_fallback_row`) stands in -- its
//! values are params of `./policy`, held here to the catalogue line -- and
//! the round says it once with a `no_package` mark.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn count(h: &Hive, kind: &str) -> i64 {
    h.rows(&format!("SELECT COUNT(*) FROM marks WHERE kind = '{kind}'"))[0][0]
        .as_i64()
        .unwrap()
}

#[test]
fn without_a_package_the_curator_still_rebuilds() {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..20 {
        turn(
            &mut h,
            "s",
            &format!("t{i}"),
            "q",
            "a",
            json!({"tokens_prompt": 100}),
        );
    }
    // The rule ran on every turn, as with a package ...
    assert_eq!(count(&h, "curate"), 20, "{:?}", h.stderr);
    // ... and the round said once that the row stood in.
    assert_eq!(count(&h, "no_package"), 1);
    let mark: Value = meclaw_core::serde_json::from_str(
        h.rows("SELECT value FROM marks WHERE kind = 'no_package'")[0][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        mark["missing"],
        json!(["input_soft", "context_window", "cache_expires_at"])
    );
    assert_eq!(mark["row"], json!("openai/gpt-6-luna"));
    assert_eq!(
        h.stderr
            .iter()
            .filter(|l| l.contains("without a model package"))
            .count(),
        1,
        "{:?}",
        h.stderr
    );
    // The row's TTL armed the clock; its strike (the quiet past the TTL, as
    // an event) rebuilds the round.
    let order = last_add(&h);
    assert_eq!(order.body["emit_body"]["reason"], "cache");
    h.fire(&order);
    assert_ne!(h.plan_in(ROUND_E), json!({}), "{:?}", h.stderr);

    // With a package: no stand-in, no mark.
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..2 {
        turn(
            &mut h,
            "s",
            &format!("t{i}"),
            "q",
            "a",
            json!({"tokens_prompt": 100, "input_soft": 250000, "input_hard": 1000000,
                   "context_window": 1050000, "cache_expires_at": "2099-01-01T00:00:00Z"}),
        );
    }
    assert_eq!(count(&h, "no_package"), 0);
}

#[test]
fn the_fallback_package_is_its_catalogue_row() {
    let p = cell_config("policy")["params"].clone();
    let id = p["input_soft_fallback_row"].as_str().expect("a row named");
    let raw = std::fs::read_to_string(repo("templates/llm-registry/store/seed/models.jsonl"))
        .expect("the catalogue");
    let row: Value = raw
        .lines()
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .find(|r| r["model_id"] == id)
        .unwrap_or_else(|| panic!("no catalogue row {id}"));
    for (param, value) in [
        ("input_soft_fallback", &row["input_soft"]),
        ("input_hard_fallback", &row["input_hard"]),
        ("cost_in_fallback", &row["cost_in"]),
        ("cost_cached_in_fallback", &row["cost_cached_in"]),
        (
            "chars_per_token_fallback",
            &row["package"]["chars_per_token"],
        ),
        ("cache_ttl_s_fallback", &row["cache_ttl_s"]),
    ] {
        assert!(value.as_f64().is_some(), "{id} states {param}");
        assert_eq!(
            p[param].as_f64(),
            value.as_f64(),
            "{param} is the row's own"
        );
    }
}
