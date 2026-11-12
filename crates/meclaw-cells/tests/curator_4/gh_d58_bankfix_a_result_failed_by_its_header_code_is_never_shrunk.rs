//! GH #1468 (D58 bankfix, F2) -- a result that failed by its header code is
//! never shrunk.
//!
//! WHY this file exists: a tool says it failed in one of two ways -- an
//! `error_code` in the header of its result (the collector hands those on as
//! the slot `tool_errors` of the curate, GH #1279) or an error object as its
//! text. The rebuild of the window knew only the second: it judged a result
//! by its text alone (`is_error_result`), so a failure told in prose under a
//! header code was shrunk to its one-line form like any fine result, and the
//! model lost the words of the failure (GH #1430 promises it keeps them).
//! `./intake` now judges once, when it files the result (`tool_failure` and
//! the slot), and writes the verdict beside the wall row (`wall.failed`); the
//! rebuild reads it.
//!
//! The failure comes in on the real way: the curate of the tool iteration
//! carries the slot through the lane, `./intake` files the row -- no row is
//! written into the ledger by hand. One of the failed results is the memory
//! tool's refusal, in the words and under the code that tool answers with
//! (`templates/memory-hive/tool`: the hive's reason as `error_code`, prose as
//! the text); `./intake` files it under the kind `recall`, and the verdict
//! goes with it all the same. The same wall under two roles: the rule is the
//! block kind's, not a role's.
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

/// A failure in plain prose: no JSON hull, nothing in the form of an error.
fn prose() -> String {
    format!(
        "The service did not answer in time, so nothing was fetched. {}",
        "It may be back later today. ".repeat(24)
    )
}

/// What the memory tool answers when its hive refused the question.
const REFUSED: &str = "this memory refused the question: store_error";

/// A fine result of some length.
fn fine(i: usize) -> String {
    format!("result {i}: {}", "m".repeat(700))
}

fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
}

/// The curate of iteration `iter` of `turn` with `messages` (and the
/// collector's `tool_errors` slot when given); returns the call to the model.
fn curate(h: &mut Hive, turn: &str, iter: u32, messages: Value, errors: Option<Value>) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": iter.to_string(), "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E});
    let mut body = json!({"messages": messages, "system": mode("Be brief.")});
    if let Some(e) = errors {
        body["tool_errors"] = e;
    }
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
/// tool, the result comes back with the curate of iteration 1 (with the
/// header code of the result in `tool_errors` when it failed), the model
/// answers.
fn tool_round(h: &mut Hive, turn: &str, id: &str, name: &str, text: &str, code: Option<&str>) {
    let call = curate(h, turn, 0, json!([user(&format!("look up {turn}"))]), None);
    h.tap(
        &call,
        "tool_calls",
        usage(500),
        json!([tool_call(id, name)]),
    );
    let call = curate(
        h,
        turn,
        1,
        json!([tool_call(id, name), tool_result(id, text)]),
        code.map(|c| json!({ id: c })),
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

fn has_key(v: &Value, key: &str) -> bool {
    match v {
        Value::Object(m) => m.contains_key(key) || m.values().any(|x| has_key(x, key)),
        Value::Array(a) => a.iter().any(|x| has_key(x, key)),
        _ => false,
    }
}

#[test]
fn a_result_failed_by_its_header_code_is_never_shrunk() {
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
        tool_round(&mut h, "r0", "c0", "lookup", &prose(), Some("unavailable"));
        tool_round(
            &mut h,
            "m0",
            "cm",
            "memory_recall",
            REFUSED,
            Some("store_error"),
        );
        tool_round(&mut h, "r1", "c1", "lookup", &fine(1), None);
        tool_round(&mut h, "r2", "c2", "lookup", &fine(2), None);
        // The round that crosses the target: its measured window orders the
        // rebuild.
        let call = curate(&mut h, "r3", 0, json!([user(&"x".repeat(12_000))]), None);
        h.tap(&call, "stop", usage(3000), json!([said("a")]));
        let order = h
            .clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("{role}: the target orders a rebuild: {:?}", h.stderr));
        h.fire(&order);
        while !h.summ.is_empty() {
            h.answer("What came before, in short.", "stop");
        }

        let (failed_id, fine_id) = (result_id(&h, "r0"), result_id(&h, "r1"));
        let plan = h.plan_in(ROUND_E);
        let shrunk: Vec<String> = plan["shrunk"]
            .as_array()
            .unwrap_or_else(|| panic!("{role}: a plan with `shrunk`: {plan} {:?}", h.stderr))
            .iter()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            shrunk.contains(&fine_id),
            "{role}: the fine result of r1 shrinks: {plan}"
        );
        assert!(
            !shrunk.contains(&failed_id),
            "{role}: the result that failed by its header code stays whole: {plan}"
        );
        assert!(
            !shrunk.contains(&result_id(&h, "m0")),
            "{role}: the refusal of the memory tool stays whole: {plan}"
        );

        // The verdict lies beside the wall row of the result, once per result.
        let verdicts = h.rows(
            "SELECT w.turn_id, w.failed FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE b.body LIKE '%\"tool_result\"%' ORDER BY w.seq",
        );
        assert_eq!(
            verdicts,
            vec![
                vec![json!("r0"), json!(1)],
                vec![json!("m0"), json!(1)],
                vec![json!("r1"), json!(0)],
                vec![json!("r2"), json!(0)],
            ],
            "{role}: intake judged every result when it filed it"
        );

        // The next call: the failure in its own words, the fine result in
        // the one-line form, and the verdict nowhere in what is sent.
        let call = curate(&mut h, "r4", 0, json!([user("and now?")]), None);
        let shown = texts(&call).join("\n");
        assert!(
            shown.contains(&prose()),
            "{role}: the model keeps the words of the failure: {shown}"
        );
        assert!(
            !shown.contains(&fine(1)),
            "{role}: the fine result of r1 is not sent whole: {shown}"
        );
        let refusal: Vec<String> = call
            .messages()
            .iter()
            .filter(|m| m["type"] == "tool_result" && m["id"] == "cm")
            .map(|m| m["text"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(refusal.len(), 1, "{role}: {shown}");
        assert!(
            refusal[0].ends_with(REFUSED) && !refusal[0].contains("history_read"),
            "{role}: the refusal of the memory tool reaches the model in its own words: {refusal:?}"
        );
        let sent = json!({"body": Value::Object(call.body.clone()),
                          "hop": Value::Object(call.hop.clone()),
                          "context": Value::Object(call.context.clone())});
        assert!(
            !has_key(&sent, "failed"),
            "{role}: the verdict never reaches the sent window: {sent}"
        );
    }
}
