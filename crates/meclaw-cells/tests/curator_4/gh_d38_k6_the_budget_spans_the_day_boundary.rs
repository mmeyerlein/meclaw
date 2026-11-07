//! GH #1427 (D38 K6, R-38-1 C) -- the budget spans the day boundary.
//!
//! WHY this file exists: a `day` rebuild (GH #1272) kept raw only the
//! `keep_recent` rounds of the running day, so the day before always took
//! its tier form at midnight -- a calendar day as the raw line, which the
//! ruling R-38-1 ends: raw is what fits the token budget, youngest first,
//! across session and day boundaries. Now the rounds within
//! `keep_recent_tokens` stay raw through a `day` rebuild whatever their day;
//! the day cut applies only to the rounds outside the budget (the floor's
//! rounds of the day before still leave, as before).
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

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

/// The plan of a `day` rebuild (`day_cut`) of `rows` on the morning of day
/// two: `{cover, todo, summary}`.
fn day_plan(over: Value, rows: &[Value]) -> Value {
    let probe = "(lambda r: {'cover': r['plan']['cover'], 'todo': len(r['todo']), \
                 'summary': r['plan']['summary']})(window_plan(ARGS['rows'], [], [], [], {}, \
                 ARGS['as_of'], [], [], 0, None, day_cut=True))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": (DAY_2_S + 7200) * 1000}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// Five rounds of yesterday and three of today, 2 000 tokens each, 16 000 in
/// all: within talky's budget (80 000) a `day` rebuild condenses nothing --
/// yesterday stays raw beside today. With a budget of four rounds (8 000) the
/// three of today and the youngest of yesterday stay, the four older rounds
/// of yesterday take the summary; without the budget every round of
/// yesterday does (the picture of GH #1272).
#[test]
fn the_budget_spans_the_day_boundary() {
    let rows = wall(5, 3, 3000);
    let got = day_plan(json!({"role": "talky"}), &rows);
    assert_eq!(
        got["cover"],
        json!(seq_of(0, 0, 5) - 1),
        "yesterday stays raw within the budget: {got}"
    );
    assert_eq!(got["todo"], json!(0), "{got}");
    assert_eq!(got["summary"], json!(""), "{got}");

    let got = day_plan(json!({"role": "talky", "keep_recent_tokens": 8_000}), &rows);
    assert_eq!(
        got["cover"],
        json!(seq_of(4, 0, 5) - 1),
        "the day cut applies outside the budget only: {got}"
    );
    assert_eq!(got["todo"], json!(8), "{got}");

    let got = day_plan(json!({"role": "talky", "keep_recent_tokens": 0}), &rows);
    assert_eq!(
        got["cover"],
        json!(seq_of(5, 0, 5) - 1),
        "without the budget the day before takes its tier: {got}"
    );
    assert_eq!(got["todo"], json!(10), "{got}");
}
