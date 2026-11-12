//! GH #1468 (D58 bankfix, F2) -- an error by its text alone still stays.
//!
//! WHY this file exists: since GH #1468 `./intake` writes its verdict on a
//! tool result beside the wall row (`wall.failed`) and the rebuild reads it.
//! That verdict is no superset of the judgement the rebuild made before, by
//! the text alone (`is_error_result`): a result with no text at all, and an
//! error object whose `error` is no code and no object (`{"error": true}`),
//! are no failure to `./intake` (`tool_failure` counts the forms the shipped
//! tools answer in) -- it files both with the verdict 0. Read alone, that
//! verdict would shrink two results the rebuild always kept whole. The fix of
//! F2 must make nothing worse, so the rebuild takes a result for a failure
//! when `./intake` filed it as failed OR the text says so. `tool_failure`
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
        assert!(
            error.ends_with(&error_true()) && !error.contains("history_read"),
            "{role}: the error object is sent whole: {error:?} in {shown}"
        );
        assert!(
            !shown.contains(&fine(2)),
            "{role}: the fine result of r2 is not sent whole: {shown}"
        );
    }
}
