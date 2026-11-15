//! GH #1471 (D58 roundpin, contract point 5) -- the guard at the hard bound
//! still cuts.
//!
//! WHY this file exists: the guard at the hard bound of the answering model
//! is the one exception to "a tool round never changes its prefix". A call
//! that would not fit is cut in the middle of a tool round, whole older
//! rounds first, whatever plan the round keeps -- also when a rebuild wrote
//! a newer plan while the round ran. The call stays a call of its tool
//! round (cut by the plan of the first call), and the watch excuses the
//! guard's cut like a rebuild.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The hard bound of the answering model, in tokens; the guard cuts at nine
/// tenths of it.
const HARD: i64 = 4000;

/// The stamp of an answering model with the hard bound [`HARD`] and a warm
/// cache, so no limit orders a rebuild.
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

fn rebuilt(t: &Talk) -> bool {
    t.h.rebuild_in(ROUND_E).get("as_of_ms").is_some()
}

/// Three turns of about 800 tokens each (three characters a token); the
/// answers bring the package, so the hard bound is known before the tool
/// round opens.
fn three_long_turns() -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(1)),
    ]);
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
    t
}

#[test]
fn gh_d58_roundpin_the_guard_at_the_hard_bound_still_cuts() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = three_long_turns();
    let id = "call_tool_rpguard_0001";
    let c0 = t.call("t4", 0, json!([user("Read the file, please.")]));
    assert!(
        guard_cuts(&c0).is_empty(),
        "check of the setup: the first call of the tool round is under the hard bound: {:?}",
        c0.hop.get("cuts")
    );
    let began = cut_epoch(&t.h, &c0);
    t.h.tap(
        &c0,
        "tool_calls",
        package(),
        json!([tool_call(id, "read_file")]),
    );

    // A rebuild writes its plan in the middle of the tool round.
    t.strike("What was said before, in short.");
    assert!(
        rebuilt(&t),
        "the plan of a rebuild is written while the tool round runs: {:?}",
        t.h.stderr
    );
    let written = t.h.rebuild_in(ROUND_E);

    // The result of the tool is about 1 500 tokens: with it the call is over
    // the guard's margin.
    let file = format!("file 1: {}", "z".repeat(4500));
    let c1 = t.call(
        "t4",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, &file)]),
    );
    let cuts = guard_cuts(&c1);
    assert_eq!(
        cuts.len(),
        1,
        "the guard cuts the call in the middle of the tool round: {:?} {:?}",
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
        "the tool round itself stays whole"
    );
    assert_eq!(
        cut_epoch(&t.h, &c1),
        began,
        "the call the guard cut is still cut by the plan of the first call"
    );
    assert_eq!(
        t.h.rebuild_in(ROUND_E),
        written,
        "the guard writes no plan of a rebuild"
    );

    // The guard's cut is the one change of a prefix inside a tool round: the
    // windows show it, the watch excuses it.
    assert_eq!(
        t.breaks().len(),
        1,
        "check of the setup: the call the guard cut does not begin with the window before it"
    );
    t.h.tap(&c1, "stop", package(), json!([said("All read.")]));
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}

/// Check of the fixture: between two rounds a stroke of the clock ends in
/// a plan here -- what the pin above asks for in the middle of a tool round.
#[test]
fn between_two_rounds_the_stroke_writes_a_plan() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = three_long_turns();
    assert!(!rebuilt(&t), "check of the setup: no rebuild so far");
    t.strike("What was said before, in short.");
    assert!(rebuilt(&t), "the stroke ends in a plan: {:?}", t.h.stderr);
}
