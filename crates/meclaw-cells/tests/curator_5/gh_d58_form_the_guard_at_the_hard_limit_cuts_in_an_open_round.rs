//! GH #1463 (D58 form, contract point 5) -- the guard at the hard limit cuts
//! in an open round.
//!
//! WHY this file exists: a round is cut by what its first call was cut by,
//! and no rebuild reaches its later calls (GH #1471). The guard at the hard
//! line of the model's window is no rebuild and is the one exception: it
//! cuts inside the call itself and writes no plan, so a call in the middle
//! of an open round never goes out over the hard line.
//!
//! The hard line a turn is guarded at is the one its first call found (the
//! turn keeps its system and the window that system was fitted to), so the
//! line stands before the round opens, and the round itself grows over it:
//! a tool result too long for what is left.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The hard line of the answering model, in tokens; the guard cuts at nine
/// tenths of it.
const HARD: i64 = 4000;

/// The stamp of an answering model with the hard line [`HARD`]: far under
/// its soft line and with a warm cache, so no limit orders a rebuild.
fn package() -> Value {
    json!({"tokens_prompt": 100, "tokens_cached": 50,
           "input_soft": HARD - 100, "input_hard": HARD,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

fn guard_cuts(call: &Msg) -> Vec<Value> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == "overflow_cut")
        .collect()
}

#[test]
fn gh_d58_form_the_guard_at_the_hard_limit_cuts_in_an_open_round() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(1)),
    ]);
    // Three rounds of about 800 tokens each (three characters a token); the
    // answers bring the package, so the hard line is known before the round
    // of the tool opens.
    for i in 1..=3 {
        let c = t.call(
            &format!("t{i}"),
            0,
            json!([user(&format!("item {i} {}", "x".repeat(1200)))]),
        );
        t.h.tap(
            &c,
            "stop",
            package(),
            json!([said(&format!("noted {i} {}", "y".repeat(1200)))]),
        );
    }
    let id = "call_tool_formguard_0001";
    let c0 = t.call("t4", 0, json!([user("Read the file, please.")]));
    assert!(
        guard_cuts(&c0).is_empty(),
        "check of the setup: the first call of the round is under the hard line: {:?}",
        c0.hop.get("cuts")
    );
    t.h.tap(
        &c0,
        "tool_calls",
        package(),
        json!([tool_call(id, "read_file")]),
    );
    let plan_before = t.h.rebuild_in(ROUND_E);

    // The result of the tool is about 1 500 tokens: with it the round is
    // over the guard's margin.
    let file = format!("file 1: {}", "z".repeat(4500));
    let c1 = t.h.curate(
        "s1",
        "t4",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, &file)]),
        mode("Be brief."),
    );
    let cuts = guard_cuts(&c1);
    assert_eq!(
        cuts.len(),
        1,
        "the guard cuts the call in the middle of the open round: {:?} {:?}",
        c1.hop.get("cuts"),
        t.h.stderr
    );
    let margin = HARD * 9 / 10;
    assert!(cuts[0]["total"].as_i64().unwrap() >= margin, "{cuts:?}");
    assert!(cuts[0]["shown"].as_i64().unwrap() < margin, "{cuts:?}");
    assert!(
        cuts[0]["rounds"].as_i64().unwrap() >= 1,
        "whole older rounds left the call: {cuts:?}"
    );
    let sent = texts(&c1);
    assert!(
        !sent.iter().any(|x| x.starts_with("item 1 ")),
        "the oldest round left the call"
    );
    assert!(
        sent.iter().any(|x| x == "Read the file, please.") && sent.contains(&file),
        "the open round itself stays whole"
    );
    assert_eq!(
        t.h.rebuild_in(ROUND_E),
        plan_before,
        "the guard writes no plan of a rebuild"
    );
}
