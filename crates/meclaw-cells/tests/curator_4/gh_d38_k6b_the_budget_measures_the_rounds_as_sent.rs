//! GH #1434 (D38 K6b, OR-38-29 B2) -- the raw token budget measures the rounds
//! in the form the window sends them.
//!
//! WHY this file exists: `keep_recent_tokens` (GH #1427) chose the youngest
//! rounds by their blocks' characters BEFORE the garbage collection (GH
//! #1430) took the old tool results down to their one-line form -- so a knob
//! of 20 000 left about 11 000 raw tokens in the window the model saw, and the
//! cap at the aim of a `target` rebuild (aim less the system part) was measured
//! against the same uncollected sizes. The bench of the wave (receipt
//! `fensterbank`, B2) read 0.49-0.55 of the knob in every arm with the
//! collection on. Now a rebuild with `gc` "1" collects first and measures the
//! rounds as they will be sent: a shrunk or released row at `FORM_CHARS`, every
//! other at its characters, over the row's rate -- the one estimate the plan
//! counts with everywhere else (`estimate`). With `gc` "0" the plan is the plan
//! of before: the collection of before runs over the aim only, after the cover.
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
/// `FORM_CHARS` of the policy: the one-line form of a shrunk or released block.
const FORM_CHARS: i64 = 90;
/// The policy's rate without a measured one, characters a token.
const CHARS_PER_TOKEN: i64 = 3;

/// A round's rows: the person (300), a tool call (60), its result (6 000), the
/// answer (300). Raw whole that is 2 220 tokens; with the result in its
/// one-line form 250.
const ROUND_CHARS: [(&str, i64, &str, Option<&str>); 4] = [
    ("user", 300, "text", None),
    ("tool_call", 60, "tool_call", Some("lookup")),
    ("tool_result", 6000, "tool_result", None),
    ("assistant", 300, "text", None),
];
/// Tokens of a round whose result the collection shrank.
const ROUND_COLLECTED: i64 = (300 + 60 + FORM_CHARS + 300) / CHARS_PER_TOKEN;

/// The second row `j` of round `i` is written at: an hour into its day, ten
/// seconds a round.
fn row_s(i: i64, j: i64, day: i64) -> i64 {
    day + 3600 + 10 * i + j
}

/// `n` tool rounds, all but the last of yesterday (so the day's tier does not
/// keep them raw by itself), the last of today. Distinct short ids: the first
/// twelve hex of the hash name the block in the plan's `shrunk`.
fn wall(n: i64) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..n {
        let day = if i < n - 1 { DAY_1_S } else { DAY_2_S };
        for (j, (kind, chars, typ, name)) in ROUND_CHARS.iter().enumerate() {
            let t = row_s(i, j as i64, day);
            let mut r = json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
                "kind": kind, "hash": format!("{:012x}{}", 4 * i + j as i64 + 1, "0".repeat(52)),
                "nth": 0, "final": if *kind == "assistant" { 1 } else { 0 },
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": chars, "type": typ});
            if let Some(n) = name {
                r["name"] = json!(n);
            }
            rows.push(r);
        }
    }
    rows
}

/// [`wall`] of `n` rounds whose tool results of yesterday are transient
/// blocks that ran out at the end of that day: before the plan, so the window
/// shows each in its one-line form (`EXPIRED_FORM`).
fn expired_wall(n: i64) -> Vec<Value> {
    let mut rows = wall(n);
    let end_of_day_1 = chrono::DateTime::from_timestamp(DAY_2_S - 1, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%S.000000Z")
        .to_string();
    for r in rows.iter_mut() {
        if r["kind"] == "tool_result" && r["seq"].as_i64().unwrap() < DAY_2_S * 1_000_000 {
            r["valid_until"] = json!(end_of_day_1);
        }
    }
    rows
}

/// The plan of a rebuild of `rows` on the morning of day two: `{cover,
/// shrunk, released, over_aim, summary}`; `aim` 0 = none, `slots` the system part.
fn plan_of(over: Value, rows: &[Value], aim: i64, slots: Value) -> Value {
    let probe = "(lambda r: {'cover': r['plan']['cover'], 'shrunk': r['plan']['shrunk'], \
                 'released': r['plan']['released'], 'over_aim': r.get('over_aim'), \
                 'summary': r['plan']['summary']})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], ARGS['slots'], [], 0, \
                 None, aim=ARGS['aim']))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS, "aim": aim, "slots": slots}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// The raw part of the window `plan` sends of `rows`, as the window shows it
/// (`render`): every row above the cover, a shrunk or released one at
/// `FORM_CHARS`, over the policy's rate -- `(tokens, rounds)`.
fn raw_sent(rows: &[Value], plan: &Value) -> (i64, usize) {
    raw_sent_at(rows, plan, None)
}

/// [`raw_sent`] with the plan's moment: a row whose `valid_until` lies at or
/// before `as_of_ms` is shown in its one-line form too (`EXPIRED_FORM`).
fn raw_sent_at(rows: &[Value], plan: &Value, as_of_ms: Option<i64>) -> (i64, usize) {
    let cover = plan["cover"].as_i64().unwrap();
    let gone: Vec<&str> = plan["shrunk"]
        .as_array()
        .unwrap()
        .iter()
        .chain(plan["released"].as_array().unwrap())
        .map(|v| v.as_str().unwrap())
        .collect();
    let above: Vec<&Value> = rows
        .iter()
        .filter(|r| r["seq"].as_i64().unwrap() > cover)
        .collect();
    let chars: i64 = above
        .iter()
        .map(|r| {
            let expired = as_of_ms.is_some_and(|at| {
                r["valid_until"].as_str().is_some_and(|u| {
                    chrono::DateTime::parse_from_rfc3339(u)
                        .is_ok_and(|u| u.timestamp_millis() <= at)
                })
            });
            if expired || gone.contains(&&r["hash"].as_str().unwrap()[..12]) {
                FORM_CHARS
            } else {
                r["chars"].as_i64().unwrap()
            }
        })
        .sum();
    let mut rounds: Vec<&str> = above
        .iter()
        .map(|r| r["turn_id"].as_str().unwrap())
        .collect();
    rounds.dedup();
    (chars / CHARS_PER_TOKEN, rounds.len())
}

/// The budget is met by whole rounds, so the raw part sent is at most the
/// budget and above the budget less the one round that did not fit any more
/// -- a collected round, since every round but the newest is collected.
fn assert_fills(raw: i64, budget: i64, plan: &Value) {
    assert!(
        raw <= budget,
        "the raw part sent ({raw}) is within the budget {budget}: {plan}"
    );
    assert!(
        raw > budget - ROUND_COLLECTED,
        "the raw part sent ({raw}) fills the budget {budget} but for one collected \
         round ({ROUND_COLLECTED}): {plan}"
    );
}

/// With the collection on, 50 tool rounds and a knob of 10 000: the result of
/// every round but the newest shrinks, and the budget is filled with rounds
/// as collected -- 9 970 tokens in the window sent (the newest round whole,
/// 31 collected rounds), not the 2 970 of a budget measured before the
/// collection (four rounds by their raw size, three of them then collected).
#[test]
fn knob_s_fills_the_sent_window_to_s_with_the_collection_on() {
    let rows = wall(50);
    let budget = 10_000;
    let plan = plan_of(
        json!({"role": "talky", "gc": "1", "keep_rounds": 1, "keep_recent": 1,
               "keep_recent_tokens": budget}),
        &rows,
        0,
        json!([]),
    );
    let (raw, rounds) = raw_sent(&rows, &plan);
    assert_fills(raw, budget, &plan);
    assert_eq!(
        plan["shrunk"].as_array().unwrap().len(),
        rounds - 1,
        "every result above the cover but the newest round's is in its one-line form: {plan}"
    );
    assert_eq!(plan["over_aim"], Value::Null, "{plan}");
}

/// The cap at the aim measures the same form: a knob far over the aim, an aim
/// of 20 000 and a system part of 2 000 tokens -- the raw part sent fills
/// 18 000, measured as collected, not the 3 970 of eight rounds chosen by
/// their raw size. Eighty rounds: collected, fifty would fit the cap whole
/// (2 220 + 49 x 250 = 14 470), and the cap would never be reached.
#[test]
fn the_cap_at_the_aim_measures_the_rounds_as_sent() {
    let rows = wall(80);
    let plan = plan_of(
        json!({"role": "talky", "gc": "1", "keep_rounds": 1, "keep_recent": 1,
               "keep_recent_tokens": 1_000_000}),
        &rows,
        20_000,
        json!([["mode", "h", 6000]]),
    );
    let (raw, _) = raw_sent(&rows, &plan);
    assert_fills(raw, 20_000 - 2_000, &plan);
    assert_eq!(
        plan["over_aim"],
        Value::Null,
        "system part and raw part together stay within the aim: {plan}"
    );
}

/// With the collection off the plan is the plan of before: the budget counts
/// the rounds whole (nothing is shrunk, nothing released -- the one estimate
/// has nothing to count differently), four rounds of 2 220 stay raw and no
/// result shrinks, since the plan is not over an aim.
#[test]
fn the_collection_off_plans_as_before() {
    let rows = wall(50);
    let plan = plan_of(
        json!({"role": "talky", "gc": "0", "keep_rounds": 1, "keep_recent": 1,
               "keep_recent_tokens": 10_000}),
        &rows,
        0,
        json!([]),
    );
    let (raw, rounds) = raw_sent(&rows, &plan);
    assert_eq!(rounds, 4, "{plan}");
    assert_eq!(raw, 4 * 2_220, "{plan}");
    assert_eq!(plan["shrunk"], json!([]), "{plan}");
}

/// A transient block whose `valid_until` ran out before the plan is shown in
/// its one-line form (`EXPIRED_FORM`), and the budget measures it so (review
/// M-1 of GH #1434): fifty tool rounds whose results of yesterday expired at
/// the end of that day, the collection off, a knob of 10 000 -- the newest
/// round whole and 31 rounds with the result at its form, 9 970 tokens, not
/// the four rounds of 2 220 a budget of the uncut sizes holds.
#[test]
fn the_budget_counts_an_expired_row_at_its_form() {
    let rows = expired_wall(50);
    let budget = 10_000;
    let plan = plan_of(
        json!({"role": "talky", "gc": "0", "keep_rounds": 1, "keep_recent": 1,
               "keep_recent_tokens": budget}),
        &rows,
        0,
        json!([]),
    );
    let (raw, rounds) = raw_sent_at(&rows, &plan, Some(AS_OF_MS));
    assert_fills(raw, budget, &plan);
    assert!(
        rounds > 4,
        "more rounds than their uncut sizes allow: {rounds} {plan}"
    );
    assert_eq!(
        plan["shrunk"],
        json!([]),
        "nothing shrinks with the collection off: {plan}"
    );
}

/// The cap at the aim and the plan's own estimate measure an expired row
/// alike (review I-1' of GH #1434): eighty tool rounds whose results of
/// yesterday ran out, an aim of 20 000 and a system part of 2 000 tokens, the
/// collection off. The budget fills the cap with rounds whose results it
/// counts at their one-line form -- the form the window sends --, so the plan
/// fits its aim as sent: no `over_aim`, the summary of the older rounds
/// stays, and no result is shrunk for an aim the window never exceeds.
/// Before, the estimate counted the same rows at their full size, found the
/// plan far over the aim, gave the summary up and (with `keep_rounds`)
/// shrank the results.
#[test]
fn the_cap_and_the_estimate_measure_an_expired_row_alike() {
    let rows = expired_wall(80);
    for keep_rounds in [0, 1] {
        let plan = plan_of(
            json!({"role": "talky", "gc": "0", "keep_rounds": keep_rounds, "keep_recent": 1,
                   "keep_recent_tokens": 1_000_000}),
            &rows,
            20_000,
            json!([["mode", "h", 6000]]),
        );
        let (raw, rounds) = raw_sent_at(&rows, &plan, Some(AS_OF_MS));
        assert!(
            rounds < 80,
            "keep_rounds {keep_rounds}: the cap leaves older rounds to the summary: {plan}"
        );
        assert_eq!(
            plan["over_aim"],
            Value::Null,
            "keep_rounds {keep_rounds}: the window as sent fits the aim: {plan}"
        );
        assert_eq!(
            plan["summary"], "pending",
            "keep_rounds {keep_rounds}: the older rounds keep their summary: {plan}"
        );
        assert_eq!(
            plan["shrunk"],
            json!([]),
            "keep_rounds {keep_rounds}: nothing shrinks under the aim: {plan}"
        );
        assert!(
            raw + 2_000 <= 20_000,
            "keep_rounds {keep_rounds}: system part and raw part as sent ({raw}) stay \
             within the aim: {plan}"
        );
    }
}
