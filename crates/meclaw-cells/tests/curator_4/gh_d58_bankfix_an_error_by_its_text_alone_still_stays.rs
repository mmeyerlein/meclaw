//! GH #1468 (D58 bankfix, F2) -- an error by its text alone still stays.
//!
//! WHY this file exists: since GH #1468 `./intake` writes its verdict on a
//! tool result beside the wall row (`wall.failed`) and the rebuild reads it.
//! That verdict is no superset of the judgement the rebuild made before, by
//! the text alone (`is_error_result`). `./intake` (`tool_failure` counts the
//! forms the shipped tools answer in) files two forms with the verdict 0: a
//! result with no text at all, and an error object whose `error` is neither
//! a string nor an object (`true`, a number, a list), is empty (`""` or
//! `{}`), or is a string from `NO_ERROR` (and that carries no string
//! `error_code` and no `ok: false`) -- as `tool_failure` in `curator/intake`
//! reads it, and as the docstring of `result_failed` in `curator/policy`
//! says it.
//! A sentence as `error` is a failure to `./intake` too, only one without a
//! code. This pin takes `{"error": true}` for the second form. Read alone,
//! that verdict would shrink two results the rebuild always kept whole. The
//! fix of F2 must make nothing worse, so the rebuild takes a result for a
//! failure when `./intake` filed it as failed OR the text says so. `tool_failure`
//! itself stays as it is: the mark `tool_error` and the count of tool
//! failures hang on it.
//!
//! Both results come in on the real way: the curate of the tool iteration
//! goes through the lane, `./intake` files the row and its verdict -- no row
//! is written into the ledger by hand. The same wall under two roles: the
//! rule is the block kind's, not a role's.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// An error object by its structure that names no code: `error` is `true`.
fn error_true() -> String {
    json!({"error": true, "detail": "e".repeat(700)}).to_string()
}

/// A fine result of some length.
fn fine(i: usize) -> String {
    format!("result {i}: {}", "m".repeat(700))
}

fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
}

/// The curate of iteration `iter` of `turn` with `messages`; returns the call
/// to the model.
fn curate(h: &mut Hive, turn: &str, iter: u32, messages: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": iter.to_string(), "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E});
    let body = json!({"messages": messages, "system": mode("Be brief.")});
    h.lane("in_curate", ctx, hop, body);
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one curate, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.out.clear();
    call
}

/// One tool round through the lanes: the person asks, the model calls a
/// tool, the result comes back with the curate of iteration 1 (no header
/// code, no `tool_errors` slot), the model answers.
fn tool_round(h: &mut Hive, turn: &str, id: &str, text: &str) {
    let call = curate(h, turn, 0, json!([user(&format!("look up {turn}"))]));
    h.tap(
        &call,
        "tool_calls",
        usage(500),
        json!([tool_call(id, "lookup")]),
    );
    let call = curate(
        h,
        turn,
        1,
        json!([tool_call(id, "lookup"), tool_result(id, text)]),
    );
    h.tap(
        &call,
        "stop",
        usage(500),
        json!([said(&format!("answered {turn}"))]),
    );
}

/// The short id of the tool result `./intake` filed for `turn`.
fn result_id(h: &Hive, turn: &str) -> String {
    let rows = h.rows(&format!(
        "SELECT w.hash FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.turn_id = '{turn}' AND b.body LIKE '%\"tool_result\"%' ORDER BY w.seq"
    ));
    assert_eq!(rows.len(), 1, "one result row of {turn}: {rows:?}");
    rows[0][0].as_str().unwrap()[..12].to_string()
}

/// The text of the one tool result with the call id `id` in `call`.
fn result_text(call: &Msg, id: &str) -> String {
    let found: Vec<String> = call
        .messages()
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["id"] == id)
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(found.len(), 1, "one result {id} in the call: {found:?}");
    found[0].clone()
}

#[test]
fn an_error_by_its_text_alone_still_stays() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for role in ["talky", "coding"] {
        // The collection on, one tool iteration kept whole, every round raw
        // (`keep_recent`), a window target the last round crosses.
        let mut h = Hive::with(&[
            ("policy", "gc", json!("1")),
            ("policy", "keep_rounds", json!(1)),
            ("policy", "keep_recent", json!(40)),
            ("policy", "window_target", json!(2000)),
        ])
        .reporting(curator_profiles::profile(role));
        tool_round(&mut h, "r0", "c0", "");
        tool_round(&mut h, "r1", "c1", &error_true());
        tool_round(&mut h, "r2", "c2", &fine(2));
        tool_round(&mut h, "r3", "c3", &fine(3));

        // `./intake` filed both forms as no failure: the verdict alone would
        // not keep them.
        let verdicts = h.rows(
            "SELECT w.turn_id, w.failed FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE b.body LIKE '%\"tool_result\"%' ORDER BY w.seq",
        );
        assert_eq!(
            verdicts,
            vec![
                vec![json!("r0"), json!(0)],
                vec![json!("r1"), json!(0)],
                vec![json!("r2"), json!(0)],
                vec![json!("r3"), json!(0)],
            ],
            "{role}: intake files the empty result and `error: true` as not failed"
        );

        // The round that crosses the target: its measured window orders the
        // rebuild.
        let call = curate(&mut h, "r4", 0, json!([user(&"x".repeat(12_000))]));
        h.tap(&call, "stop", usage(3000), json!([said("a")]));
        let order = h
            .clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("{role}: the target orders a rebuild: {:?}", h.stderr));
        h.fire(&order);
        while !h.summ.is_empty() {
            h.answer("What came before, in short.", "stop");
        }

        let (empty_id, error_id, fine_id) = (
            result_id(&h, "r0"),
            result_id(&h, "r1"),
            result_id(&h, "r2"),
        );
        let plan = h.plan_in(ROUND_E);
        let shrunk: Vec<String> = plan["shrunk"]
            .as_array()
            .unwrap_or_else(|| panic!("{role}: a plan with `shrunk`: {plan} {:?}", h.stderr))
            .iter()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            shrunk.contains(&fine_id),
            "{role}: the fine result of r2 shrinks: {plan}"
        );
        // Both forms in one statement, so a red run names each form it lost.
        let lost: Vec<&str> = [
            ("the result with no text", &empty_id),
            ("the error object `error: true`", &error_id),
        ]
        .iter()
        .filter(|(_, id)| shrunk.contains(*id))
        .map(|(form, _)| *form)
        .collect();
        assert!(
            lost.is_empty(),
            "{role}: an error by its text alone stays whole, shrunk were {lost:?}: {shrunk:?}"
        );

        // The next call: both results as they came, the fine one in the
        // one-line form.
        let call = curate(&mut h, "r5", 0, json!([user("and now?")]));
        let shown = texts(&call).join("\n");
        // A role that shows short ids sets the id before the text; nothing
        // else may stand there -- no one-line form, no pointer to the store.
        let empty = result_text(&call, "c0");
        assert!(
            empty.is_empty() || empty == format!("[#{empty_id}] "),
            "{role}: the result with no text is sent as it came: {empty:?} in {shown}"
        );
        let error = result_text(&call, "c1");
        // As strict as the empty result: the object itself, or the short id
        // and the object -- nothing else (bankfix re-check, minor 2).
        assert!(
            error == error_true() || error == format!("[#{error_id}] {}", error_true()),
            "{role}: the error object is sent whole: {error:?} in {shown}"
        );
        assert!(
            !shown.contains(&fine(2)),
            "{role}: the fine result of r2 is not sent whole: {shown}"
        );
    }
}

/// 2026-03-02T01:00:00Z in seconds, the hour of the rows of the render probe.
const RENDER_HOUR_S: i64 = 1_772_413_200;

/// A plan from before names both forms in `shrunk`, each filed with the
/// verdict 0 (bankfix re-check, minor 3): the render (`render` ->
/// `shrinkable` -> `result_failed`) still shows them whole, as the plan of
/// today does; only the fine result beside them takes the one-line form.
/// Mutant: `shrinkable` without the `result_failed` question -> red.
#[test]
fn the_render_shows_both_forms_whole_even_when_an_older_plan_shrank_them() {
    let empty_el = json!({"type": "tool_result", "id": "call_x0", "text": ""});
    let error_el = json!({"type": "tool_result", "id": "call_x1", "text": error_true()});
    let fine_el = json!({"type": "tool_result", "id": "call_x2", "text": fine(2)});
    let call_el = |id: &str| {
        json!({"type": "tool_call", "id": id, "name": "lookup",
               "text": json!({"name": "lookup"}).to_string()})
    };
    let turns: Vec<(&str, Vec<Value>)> = vec![
        ("r0", vec![call_el("call_x0"), empty_el.clone()]),
        ("r1", vec![call_el("call_x1"), error_el.clone()]),
        ("r2", vec![call_el("call_x2"), fine_el.clone()]),
        (
            "r3",
            vec![json!({"type": "text", "origin": "user", "text": "and now?"})],
        ),
    ];
    let mut rows = Vec::new();
    for (i, (turn, els)) in turns.iter().enumerate() {
        for (j, el) in els.iter().enumerate() {
            let t = RENDER_HOUR_S + 10 * i as i64 + j as i64;
            let kind = match el["type"].as_str() {
                Some("tool_result") => "tool_result",
                Some("tool_call") => "tool_call",
                _ => "user",
            };
            // The verdict `./intake` files for all three: 0.
            let failed = if kind == "tool_result" {
                json!(0)
            } else {
                Value::Null
            };
            rows.push(json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0,
                "kind": kind, "hash": sha256_hex(&canonical(el)), "nth": 0, "final": 0,
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "element": el, "failed": failed}));
        }
    }
    let plan = json!({"v": 1, "cover": 0, "keep": [], "released": [],
                      "shrunk": [short_id(&empty_el), short_id(&error_el), short_id(&fine_el)],
                      "stubs": [], "summary": "", "sum_from": 0, "sum_to": 0, "marks_to": 0,
                      "as_of_ms": (RENDER_HOUR_S + 3600) * 1000});
    let probe = "[e.get('text') for _, e, _, _ in render(blocks_of_rows(ARGS['rows'], [], None), ARGS['rows'], P, ARGS['plan'], \
                 tuple(ARGS['current'])) if e.get('type') == 'tool_result']";
    for role in ["talky", "coding"] {
        let (got, err) = policy_scope(
            curator_profiles::set_knobs(role, json!({})),
            probe,
            json!({"rows": rows, "plan": plan, "current": ["s", "r3"]}),
        );
        assert!(!err.contains("Traceback"), "{role}: {err}");
        let texts: Vec<String> = got
            .as_array()
            .unwrap_or_else(|| panic!("{role}: a list of texts: {got} {err}"))
            .iter()
            .map(|t| t.as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(texts.len(), 3, "{role}: three results: {got}");
        // A role that shows short ids sets the id before the text; nothing
        // else may stand there.
        let (empty_id, error_id) = (short_id(&empty_el), short_id(&error_el));
        assert!(
            texts[0].is_empty() || texts[0] == format!("[#{empty_id}] "),
            "{role}: the result with no text is shown as it came: {got}"
        );
        assert!(
            texts[1] == error_true() || texts[1] == format!("[#{error_id}] {}", error_true()),
            "{role}: the error object `error: true` is shown whole: {got}"
        );
        assert!(
            texts[2].contains("history_read") && !texts[2].contains(&fine(2)),
            "{role}: the fine result takes the one-line form: {got}"
        );
    }
}
