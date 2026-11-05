//! GH #1430 (D38 K3) -- with the collection off, the plan is the plan of before.
//!
//! WHY this file exists: `gc` is a knob with a preset of "1" for talky alone;
//! every other role ships "0", and an instance may switch talky off. Off, the
//! plan a rebuild makes must be byte for byte the plan the policy made before
//! the knob existed: the fixtures below are that plan, written from the
//! policy as it shipped at master 86d44d853 (after K6, GH #1427) for one
//! fixed wall (three tool rounds of today, a window of 2 000 tokens, so the
//! shrink of before runs), talky with `keep_rounds` 2 said explicitly (the
//! preset moves to 1 with this strand) and consult as it ships. The `gc`
//! value a plan now carries rides beside the plan, never in it.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

fn row(i: i64, j: i64, turn: &str, kind: &str, chars: i64, typ: &str, name: Option<&str>) -> Value {
    let t = DAY_2_S + 3600 + 10 * i + j;
    let mut r = json!({
        "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0, "kind": kind,
        "hash": sha256_hex(&format!("{turn}/{i}/{j}")), "nth": 0,
        "final": if kind == "assistant" { 1 } else { 0 },
        "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
        "audience_set": ROUND_E, "chars": chars, "type": typ});
    if let Some(n) = name {
        r["name"] = json!(n);
    }
    r
}

/// Three rounds of today, each with one tool call and its result.
fn wall() -> Vec<Value> {
    let mut rows = Vec::new();
    for (i, turn) in ["r0", "r1", "r2"].iter().enumerate() {
        let i = i as i64;
        rows.push(row(i, 0, turn, "user", 3000, "text", None));
        rows.push(row(
            i,
            1,
            turn,
            "tool_call",
            100,
            "tool_call",
            Some("lookup"),
        ));
        rows.push(row(i, 2, turn, "tool_result", 3000, "tool_result", None));
        rows.push(row(i, 3, turn, "assistant", 3000, "text", None));
    }
    rows
}

/// The plan of master 86d44d853 for `wall()` and a window of 2 000 tokens:
/// over the aim, the oldest result (r0) shrunk with `keep_rounds` 2, nothing
/// condensed (three rounds under the floor).
const TALKY_BEFORE: &str = r#"{"chain":false,"notes":["the plan stays over rebuild_to of the usable window with only the rounds it keeps raw (8130 > 900)"],"over_aim":{"aim":900,"estimate":8130},"plan":{"as_of_ms":1772416800000,"cover":1772413199999999,"keep":[],"marks_to":0,"released":[],"role":"talky","shrunk":["35c3b606e90d"],"stubs":[],"sum_from":0,"sum_to":0,"summary":"","v":1},"todo":0}"#;

/// The same for consult (`rebuild_to` 0.35, `keep_rounds` 2).
const CONSULT_BEFORE: &str = r#"{"chain":false,"notes":["the plan stays over rebuild_to of the usable window with only the rounds it keeps raw (8130 > 700)"],"over_aim":{"aim":700,"estimate":8130},"plan":{"as_of_ms":1772416800000,"cover":1772413199999999,"keep":[],"marks_to":0,"released":[],"role":"consult","shrunk":["35c3b606e90d"],"stubs":[],"sum_from":0,"sum_to":0,"summary":"","v":1},"todo":0}"#;

fn plan_now(over: Value) -> Value {
    let probe = "(lambda r: {'plan': r['plan'], 'todo': len(r['todo']), 'notes': r['notes'], \
                 'chain': r['chain'], 'over_aim': r.get('over_aim')})(window_plan(ARGS['rows'], \
                 [], [], [], {}, ARGS['as_of'], [], [], 2000, None))";
    let (got, err) = policy_scope(over, probe, json!({"rows": wall(), "as_of": AS_OF_MS}));
    assert!(!err.contains("Traceback"), "{err}");
    got
}

#[test]
fn gc_off_is_byte_identical() {
    let before: Value = meclaw_core::serde_json::from_str(TALKY_BEFORE).unwrap();
    assert_eq!(
        plan_now(json!({"role": "talky", "gc": "0", "keep_rounds": 2})),
        before,
        "talky, collection off"
    );
    let before: Value = meclaw_core::serde_json::from_str(CONSULT_BEFORE).unwrap();
    assert_eq!(
        plan_now(json!({"role": "consult"})),
        before,
        "consult, no collection"
    );
}
