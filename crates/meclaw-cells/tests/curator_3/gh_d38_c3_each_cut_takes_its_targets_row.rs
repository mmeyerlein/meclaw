//! GH #1450 (D38 C3b, OR-d38-3.o15.2, OR-d38-3.c3b.14) -- each cut takes the
//! row of its own target, and the row obeys the window the model reported.
//!
//! WHY this file exists: since Z1.7 a call's budget (`cut_budget`) takes the
//! catalogue row of the cell it goes to before the tap's copy. The tap keeps
//! ONE rate (`chars_per_token`, written by whichever cell answered last), so
//! without the row a brain call after a quick answer counted the wall at the
//! quick model's rate. With rows the rate is the target's own -- wanted
//! (review o15-R1 I1 a) and pinned here: the brain call's raw projection
//! (`window_est.raw_tokens`, read by the tap's rebuild rule) is the same
//! whatever rate the quick cell has.
//! Two more edges of the row (review I1 b, M4): the tap cut the answering
//! model's `input_soft` to the window it reported (`context_window`, GH #1040
//! review M1); the row's `input_soft` takes the same cut. A row without
//! `input_hard` leaves the guard line to the tap's `input_hard:<way>`, field
//! by field, never to the fallback row.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn row(model: &str, soft: i64, hard: Value, rate: f64) -> Value {
    json!({"model": model, "input_soft": soft, "input_hard": hard, "max_output": 4096,
           "price_tiers": null, "cache_mode": null, "cache_ttl_s": null,
           "wire_dialect": "chat_completions", "supported_params": [],
           "chars_per_token": rate})
}

/// The cell `cell` of talky reports its package: `row`, and the profile its
/// template writes beside it (D58 target, contract point 13).
fn catalog(h: &mut Hive, cell: &str, row: Value) {
    h.out.clear();
    h.report_row("talky", cell, row);
}

/// The call one turn on `path` sends on `way` (never a bridge), not yet
/// answered.
fn call_on(h: &mut Hive, turn: &str, path: &str, way: &str, ask: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "chat", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    h.routed(way)
        .into_iter()
        .find(|m| {
            m.hop.get("call_kind").is_none_or(|v| *v != "side")
                && !m.hop["curator_call"]
                    .as_str()
                    .unwrap_or("")
                    .ends_with("-bridge")
        })
        .unwrap_or_else(|| panic!("a call on {way}: {:?} {:?}", h.out, h.stderr))
}

fn cut_of(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let rows = h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"));
    let text = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    sj::from_str::<Value>(&text).unwrap_or(Value::Null)
}

/// Two brain rounds of long words, a quick turn answered at `quick_rate`,
/// then a brain call: its `window_est.raw_tokens`. The brain's row counts
/// at `brain_rate`; the quick row and the quick answer at `quick_rate`.
fn brain_raw_after_quick(brain_rate: f64, quick_rate: f64) -> i64 {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(
        &mut h,
        "brain",
        row("test/brain", 100_000, json!(150_000), brain_rate),
    );
    catalog(
        &mut h,
        "quick",
        row("test/quick", 100_000, json!(150_000), quick_rate),
    );
    for i in 0..2 {
        let c = call_on(
            &mut h,
            &format!("t{i}"),
            "talk",
            "brain",
            &"w".repeat(2_000),
        );
        h.tap(
            &c,
            "stop",
            json!({"chars_per_token": brain_rate}),
            json!([said(&"a".repeat(2_000))]),
        );
    }
    let q = call_on(&mut h, "t5", "simple", "quick", "What day is it?");
    h.tap(
        &q,
        "stop",
        json!({"chars_per_token": quick_rate}),
        json!([said("Monday.")]),
    );
    let b = call_on(&mut h, "t6", "talk", "brain", "and tomorrow?");
    let est = h.state(&format!(
        "window_est:{}",
        b.hop["curator_call"].as_str().unwrap_or_default()
    ));
    let v: Value = sj::from_str(&est).unwrap_or(Value::Null);
    v["raw_tokens"]
        .as_i64()
        .unwrap_or_else(|| panic!("the brain call projects its raw part: {est}"))
}

#[test]
fn the_brain_call_counts_at_the_brains_rate_after_a_quick_answer() {
    if !shipped() {
        return;
    }
    let quick_far = brain_raw_after_quick(2.0, 8.0);
    let quick_same = brain_raw_after_quick(2.0, 2.0);
    let brain_far = brain_raw_after_quick(8.0, 8.0);
    assert_ne!(
        quick_same, brain_far,
        "the rate moves the projection (else this pin proves nothing)"
    );
    assert_eq!(
        quick_far, quick_same,
        "a brain call counts at the brain row's rate, whatever the quick cell answered at"
    );
}

#[test]
fn the_rows_soft_is_cut_to_the_window_the_model_reported() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(
        &mut h,
        "brain",
        row("test/wide", 200_000, json!(300_000), 3.5),
    );
    let first = call_on(&mut h, "t0", "talk", "brain", "first question");
    assert_eq!(
        cut_of(&h, &first)["budget"]["soft"],
        json!(200_000),
        "before any answer the row sizes the call"
    );
    h.tap(
        &first,
        "stop",
        json!({"context_window": 50_000, "input_soft": 200_000, "input_hard": 300_000}),
        json!([said("first answer")]),
    );
    let second = call_on(&mut h, "t1", "talk", "brain", "second question");
    assert_eq!(
        cut_of(&h, &second)["budget"]["soft"],
        json!(50_000),
        "the row's input_soft is cut to the reported window like the tap's (GH #1040 M1)"
    );
}

/// The second brain call's cut after a tap that stamped `input_hard`
/// 9 000; the brain's row (without `input_hard`) when `with_row`.
fn hard_after_tap(with_row: bool) -> Value {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    if with_row {
        catalog(
            &mut h,
            "brain",
            row("test/no-hard", 100_000, Value::Null, 3.5),
        );
    }
    let first = call_on(&mut h, "t0", "talk", "brain", "first question");
    h.tap(
        &first,
        "stop",
        json!({"input_soft": 100_000, "input_hard": 9_000}),
        json!([said("first answer")]),
    );
    let second = call_on(&mut h, "t1", "talk", "brain", "second question");
    cut_of(&h, &second)["budget"]["hard"].clone()
}

#[test]
fn a_row_without_input_hard_keeps_the_taps_line() {
    if !shipped() {
        return;
    }
    let tap = hard_after_tap(false);
    assert!(
        tap.as_i64().is_some_and(|x| x > 0 && x < 9_000),
        "without a row the line is the tap's input_hard: {tap}"
    );
    assert_eq!(
        hard_after_tap(true),
        tap,
        "a row without input_hard falls back field by field to the tap's"
    );
}

/// Review m1 of c3b (OR-d38-3.o15.5): the window a quick answer reported is
/// the quick model's; the brain call's row is cut only to the window the
/// brain itself reported (`context_window:<way>`), never to another cell's.
#[test]
fn the_rows_soft_is_never_cut_to_another_cells_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(
        &mut h,
        "brain",
        row("test/wide", 200_000, json!(300_000), 3.5),
    );
    catalog(
        &mut h,
        "quick",
        row("test/narrow", 200_000, json!(300_000), 3.5),
    );
    let first = call_on(&mut h, "t0", "talk", "brain", "first question");
    h.tap(
        &first,
        "stop",
        json!({"input_soft": 200_000, "input_hard": 300_000}),
        json!([said("first answer")]),
    );
    let q = call_on(&mut h, "t1", "simple", "quick", "What day is it?");
    h.tap(
        &q,
        "stop",
        json!({"context_window": 50_000, "input_soft": 200_000, "input_hard": 300_000}),
        json!([said("Monday.")]),
    );
    let second = call_on(&mut h, "t2", "talk", "brain", "second question");
    assert_eq!(
        cut_of(&h, &second)["budget"]["soft"],
        json!(200_000),
        "the quick model's window never cuts the brain's row"
    );
}
