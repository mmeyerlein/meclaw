//! GH #1430 (D38 K3) -- a tool result that is an error is never shrunk.
//!
//! WHY this file exists: the garbage collection of a rebuild (R-38-2) takes
//! old tool results out of the raw window (`shrunk`, one line and an id,
//! `history_read` brings them back) -- but never an error result: the model
//! must keep seeing that a tool failed, in the words of the failure, or it
//! speaks a result it never got. The policy already knows an error result by
//! its structure (`is_error_result`, `error`/`error_code` in a JSON text) and
//! used that knowledge for routing and facts, not for shrinking. Two locks:
//! the plan (`window_plan`, pure: an enriched wall row says `error`) leaves
//! the error result out of `shrunk`, and the render (`render`) shows an error
//! result whole even when a plan from before named it -- so the count of the
//! `gc` mark and the window agree. Mutant: the check gone -> both red.
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

/// 2026-03-02T00:00:00Z in seconds; the plan is made two hours in.
const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

/// Row `j` of round `i` of the day, with the fields `rb-data` enriches a
/// wall row with (`chars`, `type`, `name`) and what the extra says.
fn row(i: i64, j: i64, turn: &str, kind: &str, chars: i64, typ: &str, extra: Value) -> Value {
    let t = DAY_2_S + 3600 + 10 * i + j;
    let mut r = json!({
        "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0, "kind": kind,
        "hash": sha256_hex(&format!("{turn}/{i}/{j}")), "nth": 0,
        "final": if kind == "assistant" { 1 } else { 0 },
        "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
        "audience_set": ROUND_E, "chars": chars, "type": typ});
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            r[k] = v.clone();
        }
    }
    r
}

/// A round with one tool call and its result (3 000 characters each side).
fn tool_round(i: i64, turn: &str, result: Value) -> Vec<Value> {
    vec![
        row(i, 0, turn, "user", 3000, "text", json!({})),
        row(
            i,
            1,
            turn,
            "tool_call",
            100,
            "tool_call",
            json!({"name": "lookup"}),
        ),
        row(i, 2, turn, "tool_result", 3000, "tool_result", result),
        row(i, 3, turn, "assistant", 3000, "text", json!({})),
    ]
}

fn id_of(r: &Value) -> String {
    r["hash"].as_str().unwrap()[..12].to_string()
}

fn plan_of(over: Value, rows: &[Value], cw: i64) -> Value {
    let probe = "(lambda r: {'shrunk': r['plan']['shrunk'], 'gc': r.get('gc')})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], ARGS['cw'], None))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS, "cw": cw}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// Three tool rounds, the oldest result an error, `keep_rounds` 1 and a
/// window of 2 000 tokens (the plan is over its aim, so the shrink of
/// before runs too): the middle result shrinks, the error result stays raw,
/// the `gc` mark counts one.
#[test]
fn an_error_result_is_never_shrunk() {
    let wall: Vec<Value> = [
        tool_round(0, "r0", json!({"error": true})),
        tool_round(1, "r1", json!({"error": false})),
        tool_round(2, "r2", json!({"error": false})),
    ]
    .concat();
    let got = plan_of(
        curator_profiles::set_knobs("talky", json!({"gc": "1", "keep_rounds": 1})),
        &wall,
        2000,
    );
    assert_eq!(
        got["shrunk"],
        json!([id_of(&wall[6])]),
        "only the fine result of r1 shrinks; the error of r0 stays whole: {got}"
    );
    assert_eq!(
        got["gc"]["classes"]["shrunk_tools"],
        json!(1),
        "the mark counts what shrank: {got}"
    );
}

/// A plan from before named an error result among `shrunk`: the window still
/// shows it whole; the fine result beside it takes the one-line form.
#[test]
fn the_window_shows_an_error_result_whole_even_when_the_plan_shrank_it() {
    let err_el = json!({"type": "tool_result", "id": "call_x1",
                        "text": json!({"error": "boom"}).to_string()});
    let ok_el = json!({"type": "tool_result", "id": "call_x2",
                       "text": "a fine result\nwith a second line"});
    let call_el = |id: &str| {
        json!({"type": "tool_call", "id": id, "name": "lookup",
               "text": json!({"name": "lookup"}).to_string()})
    };
    let turns: Vec<(&str, Vec<Value>)> = vec![
        ("r0", vec![call_el("call_x1"), err_el.clone()]),
        ("r1", vec![call_el("call_x2"), ok_el.clone()]),
        (
            "r2",
            vec![json!({"type": "text", "origin": "user", "text": "and now?"})],
        ),
    ];
    let mut rows = Vec::new();
    for (i, (turn, els)) in turns.iter().enumerate() {
        for (j, el) in els.iter().enumerate() {
            let t = DAY_2_S + 3600 + 10 * i as i64 + j as i64;
            let kind = match el["type"].as_str() {
                Some("tool_result") => "tool_result",
                Some("tool_call") => "tool_call",
                _ => "user",
            };
            rows.push(json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0,
                "kind": kind, "hash": sha256_hex(&canonical(el)), "nth": 0, "final": 0,
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "element": el}));
        }
    }
    let plan = json!({"v": 1, "cover": 0, "keep": [], "released": [],
                      "shrunk": [short_id(&err_el), short_id(&ok_el)], "stubs": [],
                      "summary": "", "sum_from": 0, "sum_to": 0, "marks_to": 0,
                      "as_of_ms": AS_OF_MS});
    let probe = "[e.get('text') for _, e, _, _ in render(blocks_of_rows(ARGS['rows'], [], None), ARGS['rows'], P, ARGS['plan'], \
                 tuple(ARGS['current'])) if e.get('type') == 'tool_result']";
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        probe,
        json!({"rows": rows, "plan": plan, "current": ["s", "r2"]}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    let texts: Vec<String> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(texts.len(), 2, "{got}");
    // talky shows short ids: the error result keeps its `[#<id>] ` prefix
    // and its whole text, no one-line form (OR-d38-1.k3.9).
    assert_eq!(
        texts[0],
        format!(
            "[#{}] {}",
            short_id(&err_el),
            err_el["text"].as_str().unwrap()
        ),
        "the error result is shown as it is: {got}"
    );
    assert!(
        texts[1].contains("shortened") && texts[1].contains("history_read"),
        "the fine result takes the one-line form: {got}"
    );
}
