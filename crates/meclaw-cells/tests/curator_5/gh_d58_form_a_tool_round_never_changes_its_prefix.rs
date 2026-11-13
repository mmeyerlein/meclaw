//! GH #1463 (D58 form) -- a tool round never changes its prefix.
//!
//! WHY this file exists: every answer orders a rebuild at the clock, the one
//! that asks for a tool too. A strike between two calls of a round rebuilt
//! the plan, and the next call of the SAME round read it: the window changed
//! in its middle, under the signed thinking blocks of the round and under
//! the cache. The rule: between the calls of a round nothing is cut, and
//! every call of the round begins with the whole window before it, whatever
//! a strike rebuilt meanwhile (GH #1471: the round is cut by what its first
//! call was cut by; a rebuild shows in the next round). It holds for every
//! role -- three presets, the third with a long round and large results.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::json;

const SAID: &str = "My sister Anna moved to Hamburg.";

fn rebuilt(t: &Talk) -> bool {
    t.h.rebuild_in(ROUND_E).get("as_of_ms").is_some()
}

/// The clock strikes with the round's newest order.
fn strike(t: &mut Talk) {
    let order = t
        .h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("check of the setup: an order at the clock: {:?}", t.h.clock));
    t.h.fire(&order);
    if !t.h.summ.is_empty() {
        t.h.answer("What was said before, in short.", "stop");
    }
}

fn a_round_under_strikes(preset: &str, calls: u32, chars: usize) {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(curator_profiles::profile(preset));
    t.turn("t1", SAID, "Hamburg is lovely.");
    let id = "call_recall_00000000000formtr";
    let c = t.call(
        "t2",
        0,
        json!([
            user("Tell me about Anna."),
            tool_call(id, "memory_recall"),
            tool_result(id, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
        ]),
    );
    t.answer(&c, "stop", json!([said("She likes it there.")]));
    assert!(!rebuilt(&t), "check of the setup: no rebuild so far");

    let tid = |j: u32| format!("call_tool_formtr_{j:04}");
    let mut c = t.call("t3", 0, json!([user("Read the files, please.")]));
    for j in 1..calls {
        t.answer(&c, "tool_calls", json!([tool_call(&tid(j), "read_file")]));
        // The clock strikes while the round waits for its tool.
        strike(&mut t);
        // Roundpin review M-6: the first stroke in the round writes a plan
        // -- without it the later calls would keep their prefix for nothing.
        if j == 1 {
            assert!(
                rebuilt(&t),
                "{preset}: check of the setup: a plan was written in the round: {:?}",
                t.h.stderr
            );
        }
        let result = if j == 2 {
            "{\"error\": \"exit status 1\"}".to_string()
        } else {
            format!("file {j}: {}", "x".repeat(chars))
        };
        c = t.call(
            "t3",
            j,
            json!([
                tool_call(&tid(j), "read_file"),
                tool_result(&tid(j), &result)
            ]),
        );
    }
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "{preset}: every call of the round begins with the whole window before it"
    );
    // A rebuild is not lost over the round.
    t.answer(&c, "stop", json!([said("All read.")]));
    strike(&mut t);
    assert!(
        rebuilt(&t),
        "{preset}: after the round the strike rebuilds: {:?}",
        t.h.stderr
    );
}

#[test]
fn the_conversation_preset_keeps_the_round() {
    a_round_under_strikes("talky", 3, 200);
}

#[test]
fn the_consultation_preset_keeps_the_round() {
    a_round_under_strikes("consult", 4, 2000);
}

#[test]
fn the_coding_preset_keeps_a_long_round() {
    a_round_under_strikes("coding", 7, 12000);
}
