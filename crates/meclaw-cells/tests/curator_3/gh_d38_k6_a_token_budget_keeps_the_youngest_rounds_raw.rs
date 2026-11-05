//! GH #1427 (D38 K6, R-38-1 C) -- a token budget keeps the youngest rounds raw.
//!
//! WHY this file exists: what stayed raw in the window was a NUMBER OF ROUNDS
//! (`keep_recent`, talky 40) -- a day of forty short turns and a day of forty
//! long ones were held raw alike, and the owner's ruling (R-38-1, option C)
//! says raw is what fits a TOKEN budget, youngest first, across session and
//! day boundaries. The new knob `keep_recent_tokens` (talky 80 000, every
//! other role null = off) holds the youngest rounds raw whose estimated size
//! together is at most the budget; `keep_recent` stays the floor in rounds
//! and the newest round is always raw. The turn's `curate` stamp names the
//! raw part (`raw_tokens`, `raw_rounds`) so the watcher can read it.
//!
//! Runs the pure half of the shipped policy and the shipped curator hive
//! (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z and the day after, in seconds.
const DAY_1_S: i64 = 1_772_323_200;
const DAY_2_S: i64 = 1_772_409_600;

/// The second a row of round `i`, row `j`, is written at: an hour into the
/// day, two seconds a round.
fn row_s(i: i64, j: i64, yesterday: i64) -> i64 {
    let day = if i < yesterday { DAY_1_S } else { DAY_2_S };
    day + 3600 + 2 * i + j
}

/// The seq of round `i`'s first row (`j` = 0) or answer (`j` = 1).
fn seq_of(i: i64, j: i64, yesterday: i64) -> i64 {
    row_s(i, j, yesterday) * 1_000_000
}

/// `yesterday` rounds on day one, `today` rounds on day two, two rows of
/// `chars` characters each -- `chars` / 3 tokens a row by the policy's own
/// rate, so a round of 3 000 characters a row is 2 000 tokens.
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

/// The plan a rebuild makes of `rows` on the morning of day two, without an
/// aim: `{cover, todo, sum_from, sum_to}`.
fn plan_of(over: Value, rows: &[Value]) -> Value {
    let probe = "(lambda r: {'cover': r['plan']['cover'], 'todo': len(r['todo']), \
                 'sum_from': r['plan']['sum_from'], 'sum_to': r['plan']['sum_to']})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": (DAY_2_S + 7200) * 1000}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// 60 rounds of 2 000 tokens, 59 of yesterday and one of today, a budget of
/// 80 000 tokens and a floor of 10 rounds: exactly the youngest 40 rounds stay
/// raw (40 x 2 000 = 80 000, the budget inclusive), the 20 before them take
/// yesterday's tier form (the summary). The same wall without the budget keeps
/// the floor's 10.
#[test]
fn a_token_budget_keeps_the_youngest_rounds_raw() {
    let rows = wall(59, 1, 3000);
    let got = plan_of(
        json!({"role": "talky", "keep_recent": 10, "keep_recent_tokens": 80_000}),
        &rows,
    );
    assert_eq!(
        got["cover"],
        json!(seq_of(20, 0, 59) - 1),
        "the cover stands before the 40th youngest round: {got}"
    );
    assert_eq!(
        got["todo"],
        json!(40),
        "20 rounds of two rows condense: {got}"
    );
    assert_eq!(got["sum_from"], json!(seq_of(0, 0, 59)), "{got}");
    assert_eq!(got["sum_to"], json!(seq_of(19, 1, 59)), "{got}");

    // 0 = off: the floor alone, as before the knob.
    let got = plan_of(
        json!({"role": "talky", "keep_recent": 10, "keep_recent_tokens": 0}),
        &rows,
    );
    assert_eq!(
        got["cover"],
        json!(seq_of(50, 0, 59) - 1),
        "without the budget the floor of 10 rounds stays raw: {got}"
    );
    assert_eq!(got["todo"], json!(100), "{got}");
}

/// The budget counts whole rounds: a round that does not fit as a whole does
/// not fit at all. 41 rounds of 2 000 would be 82 000: with a budget of
/// 81 999 the 41st youngest round condenses, with 82 000 it stays.
#[test]
fn a_round_fits_the_budget_whole_or_not_at_all() {
    let rows = wall(59, 1, 3000);
    let got = plan_of(
        json!({"role": "talky", "keep_recent": 1, "keep_recent_tokens": 81_999}),
        &rows,
    );
    assert_eq!(got["cover"], json!(seq_of(20, 0, 59) - 1), "{got}");
    let got = plan_of(
        json!({"role": "talky", "keep_recent": 1, "keep_recent_tokens": 82_000}),
        &rows,
    );
    assert_eq!(got["cover"], json!(seq_of(19, 0, 59) - 1), "{got}");
}

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

/// Every turn's `curate` stamp names the raw part of the call's window:
/// `raw_rounds` (the rounds shown raw above the plan's cover, the running one
/// among them) and `raw_tokens` (their estimate, the rule of the projection).
#[test]
fn the_curate_stamp_names_the_raw_part() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // Five earlier rounds of today, 600 characters a row: at least 200 tokens
    // a row, 2 000 in all.
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..5 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        let text = format!("{i} {}", "x".repeat(598));
        h.row(at, "s", &format!("r{i}"), "user", &user(&text), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&text),
            1,
        );
    }
    let call = h.curate("s", "t1", 0, json!([user("and now?")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 3_000, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said("a")]),
    );
    let s = stamp(&h);
    let raw_rounds = s["raw_rounds"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_rounds in the stamp: {s} {:?}", h.stderr));
    assert!(
        (5..=6).contains(&raw_rounds),
        "the five rounds, the running one among them or not: {s}"
    );
    let raw_tokens = s["raw_tokens"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_tokens in the stamp: {s}"));
    let projected = s["window_estimate"]
        .as_i64()
        .expect("the projection of the call");
    assert!(
        raw_tokens >= 2_000 && raw_tokens <= projected,
        "the raw part is at least the five rounds and at most the call: {s}"
    );
}
