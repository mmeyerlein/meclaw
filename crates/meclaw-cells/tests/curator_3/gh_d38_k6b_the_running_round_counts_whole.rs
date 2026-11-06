//! GH #1434 (D38 K6b, rest list of the review of the second fix round) -- a
//! row of the running round counts whole, as `render` sends it.
//!
//! WHY this file exists: `render` shows a released or expired row in its
//! one-line form only outside the running round (`key != current`); a
//! transient block of the round being answered goes out whole even when its
//! `valid_until` ran out. The plan's one set of short forms (`gone_ids`)
//! counted such a row at `FORM_CHARS` all the same, so the raw token budget
//! measured the newest round far under what the window sends and filled the
//! rest of the budget with older rounds on top: the window overshot the knob
//! by the whole block. Now `gone_ids` leaves the running round out -- the
//! newest round of the rebuild, the round the garbage collection never
//! touches either -- and the budget, its cap, the plan's estimate and the
//! `gc` mark count it as sent.
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
/// The morning of day two.
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

/// An RFC 3339 stamp of `s` seconds.
fn stamp(s: i64) -> String {
    chrono::DateTime::from_timestamp(s, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%S.000000Z")
        .to_string()
}

/// `n` tool rounds of 300 + 60 + `result` + 300 characters, all but the last
/// of yesterday; the last round's result is `newest_result` characters and a
/// transient block that ran out an hour before the plan.
fn wall(n: i64, result: i64, newest_result: i64) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..n {
        let newest = i == n - 1;
        let day = if newest { DAY_2_S } else { DAY_1_S };
        let kinds = [
            ("user", 300, "text"),
            ("tool_call", 60, "tool_call"),
            (
                "tool_result",
                if newest { newest_result } else { result },
                "tool_result",
            ),
            ("assistant", 300, "text"),
        ];
        for (j, (kind, chars, typ)) in kinds.iter().enumerate() {
            let t = day + 3600 + 10 * i + j as i64;
            let mut r = json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
                "kind": kind, "hash": format!("{:012x}{}", 4 * i + j as i64 + 1, "0".repeat(52)),
                "nth": 0, "final": if *kind == "assistant" { 1 } else { 0 },
                "at": stamp(t), "audience_set": ROUND_E, "chars": chars, "type": typ});
            if *kind == "tool_call" {
                r["name"] = json!("lookup");
            }
            if newest && *kind == "tool_result" {
                r["valid_until"] = json!(stamp(DAY_2_S + 3600));
            }
            rows.push(r);
        }
    }
    rows
}

/// A newest round of 10 220 tokens (a 30 000-character expired result sent
/// whole) against a knob of 10 000: the round alone is over the budget, so it
/// is the only raw round -- not the newest round counted at its one-line form
/// (about 250 tokens) and some forty older rounds on top of it.
#[test]
fn an_expired_row_of_the_running_round_counts_whole_in_the_budget() {
    let rows = wall(50, 150, 30_000);
    let probe = "(lambda r: r['plan']['cover'])(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    let (cover, err) = policy_scope(
        json!({"role": "talky", "gc": "0", "keep_rounds": 1, "keep_recent": 1,
               "keep_recent_tokens": 10_000}),
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    let cover = cover.as_i64().unwrap_or(0);
    let mut raw: Vec<&str> = rows
        .iter()
        .filter(|r| r["seq"].as_i64().unwrap() > cover)
        .map(|r| r["turn_id"].as_str().unwrap())
        .collect();
    raw.dedup();
    assert_eq!(
        raw,
        vec!["r49"],
        "the running round, sent whole, fills the budget alone: cover {cover}"
    );
}
