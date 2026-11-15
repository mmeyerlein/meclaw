//! GH #1427 (D38 K6, OR-d38-1.k6.9) -- the window watch reads the raw part the policy stamps.
//!
//! WHY this file exists: `./policy` names the raw part of every call in the
//! turn's `curate` stamp (`raw_tokens`, `raw_rounds`), and `curator/stats`
//! kind `window` reads `raw_tokens` off that very stamp into `raw_p95` (GH
//! #1426) -- if present. Two templates, one key, and nothing in either pins
//! the other: a renamed key in the stamp would leave `raw_p95` at 0 in silence.
//! Here every turn runs through the shipped policy of the talky instance as it
//! ships, the hive's own `stats` door counts the trail, and `raw_p95` must be
//! the nearest-rank 95th percentile of the stamps' own `raw_tokens`.
//! Deterministic: no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{Hive, ROUND_E, clear_of_midnight, mode, read_json, repo, said, shipped, user};
use meclaw_core::serde_json::{self, Value, json};

const TURNS: u32 = 6;
const WINDOW: u64 = 3000;

/// The talky instance as it ships (the marker's overrides).
fn talky() -> Hive {
    let marker = read_json(&repo("templates/talky/curator/config.json"));
    let mut over: Vec<(String, String, Value)> = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            over.push((cell.clone(), k, v));
        }
    }
    let all: Vec<(&str, &str, Value)> = over
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    Hive::with(&all).reporting_composite("talky")
}

/// One turn through the shipped policy; the turn's `curate` stamp.
fn turn(h: &mut Hive, i: u32) -> Value {
    let call = h.curate(
        "s",
        &format!("t{i}"),
        0,
        json!([user(&format!(
            "turn {i} {}",
            "y".repeat(300 + 100 * i as usize)
        ))]),
        mode("Be brief."),
    );
    let usage = json!({"tokens_prompt": WINDOW, "tokens_cached": WINDOW / 2,
                       "input_soft": 5_000_000, "input_hard": 10_000_000,
                       "cost_in": 10, "cost_cached_in": 0});
    h.tap(&call, "stop", usage, json!([said("a")]));
    serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

/// The curator's `stats` answer to `kinds` over the last day and the next.
fn stats(h: &mut Hive, kinds: &[&str]) -> Value {
    let now = chrono::Utc::now();
    let day = chrono::Duration::days(1);
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "wait:s1",
               "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_kinds": serde_json::to_string(&kinds).unwrap()}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    m.body["stats"].clone()
}

#[test]
fn gh_d38_k6_the_window_watch_reads_the_raw_part() {
    if !shipped() {
        return;
    }
    // talky's horizon is the day: a turn across midnight would mark `day`.
    clear_of_midnight();
    let mut h = talky();
    let stamps: Vec<Value> = (0..TURNS).map(|i| turn(&mut h, i)).collect();
    let window = &stats(&mut h, &["calls", "window"])["counts"]["window"];
    assert_eq!(window["n"], TURNS, "{window}");

    // The coupling, read from the watch's side first: a stamp key the stats
    // cell does not know leaves `raw_p95` at 0.
    let p95 = window["raw_p95"].as_u64().unwrap_or(0);
    assert!(
        p95 > 0,
        "`stats` window reads the raw part of the policy's stamp: {window} / {stamps:?}"
    );
    let mut raw: Vec<u64> = stamps
        .iter()
        .map(|s| {
            assert!(s["raw_rounds"].as_u64().unwrap_or(0) >= 1, "{s}");
            s["raw_tokens"]
                .as_u64()
                .unwrap_or_else(|| panic!("the stamp names `raw_tokens`: {s}"))
        })
        .collect();
    raw.sort_unstable();
    let i = (95 * raw.len()).div_ceil(100).max(1) - 1;
    assert_eq!(p95, raw[i.min(raw.len() - 1)], "{window} / {raw:?}");
}
