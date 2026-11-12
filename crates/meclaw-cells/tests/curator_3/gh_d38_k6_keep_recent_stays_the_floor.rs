//! GH #1427 (D38 K6) -- `keep_recent` stays the floor.
//!
//! WHY this file exists: the token budget (`keep_recent_tokens`) decides how
//! much stays raw, but a budget smaller than the role's `keep_recent` rounds
//! must not cut below them: `keep_recent` is the floor in rounds, the newest
//! round is always raw (it may be the one the next call runs), and a budget
//! of one round or none changes neither.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z and the day after, in seconds.
const DAY_1_S: i64 = 1_772_323_200;
const DAY_2_S: i64 = 1_772_409_600;

fn row_s(i: i64, j: i64, yesterday: i64) -> i64 {
    let day = if i < yesterday { DAY_1_S } else { DAY_2_S };
    day + 3600 + 2 * i + j
}

fn seq_of(i: i64, j: i64, yesterday: i64) -> i64 {
    row_s(i, j, yesterday) * 1_000_000
}

/// `yesterday` rounds on day one, `today` rounds on day two, two rows of
/// `chars` characters each (`chars` / 3 tokens a row).
fn wall(yesterday: i64, today: i64, chars: i64) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..yesterday + today {
        for j in 0..2i64 {
            let t = row_s(i, j, yesterday);
            rows.push(json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": chars, "type": "text"}));
        }
    }
    rows
}

fn plan_of(over: Value, rows: &[Value]) -> Value {
    let probe = "(lambda r: {'cover': r['plan']['cover'], 'todo': len(r['todo'])})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": (DAY_2_S + 7200) * 1000}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// 50 rounds of 2 000 tokens (49 of yesterday, one of today) and a budget of
/// half a round: talky's floor of 40 rounds stays raw all the same.
#[test]
fn keep_recent_stays_the_floor() {
    let rows = wall(49, 1, 3000);
    let got = plan_of(
        curator_profiles::set_knobs("talky", json!({"keep_recent_tokens": 1_000})),
        &rows,
    );
    assert_eq!(
        got["cover"],
        json!(seq_of(10, 0, 49) - 1),
        "the floor of 40 rounds holds under a budget of half a round: {got}"
    );
    assert_eq!(got["todo"], json!(20), "{got}");

    // No floor at all: the newest round is raw whatever the budget.
    let got = plan_of(
        curator_profiles::set_knobs(
            "talky",
            json!({"keep_recent": 0, "keep_recent_tokens": 1_000}),
        ),
        &rows,
    );
    assert_eq!(
        got["cover"],
        json!(seq_of(49, 0, 49) - 1),
        "the newest round is never cut: {got}"
    );
    assert_eq!(got["todo"], json!(98), "{got}");
}

/// The constants the knobs resolve to: the preset of talky, off for every
/// other role, an instance's own number, and null for the preset.
#[test]
fn the_budget_knob_resolves_like_every_knob() {
    for (over, want) in [
        (curator_profiles::set_knobs("talky", json!({})), 80_000),
        (
            curator_profiles::set_knobs("talky", json!({"keep_recent_tokens": null})),
            80_000,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"keep_recent_tokens": 12_345})),
            12_345,
        ),
        (
            curator_profiles::set_knobs("talky", json!({"keep_recent_tokens": 0})),
            0,
        ),
        (curator_profiles::set_knobs("consult", json!({})), 0),
        (curator_profiles::set_knobs("coding", json!({})), 0),
        (curator_profiles::set_knobs("research", json!({})), 0),
        (curator_profiles::set_knobs("quick", json!({})), 0),
        (json!({"role": ""}), 0),
    ] {
        let (got, err) = policy_scope(over.clone(), "KEEP_RECENT_TOKENS", Value::Null);
        assert_eq!(got, json!(want), "{over}: {err}");
    }
    // The floor is untouched by the budget.
    let (got, _) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "KEEP_RECENT",
        Value::Null,
    );
    assert_eq!(got, json!(40));
}
