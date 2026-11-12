//! GH #1463 (D58 form) -- a warm call only appends.
//!
//! WHY this file exists: the model cell caches the window up to the newest
//! words of the person, and a provider with signed thinking blocks refuses a
//! round whose earlier calls changed. So within a round every call begins
//! with the WHOLE window of the call before it, and across the round boundary
//! with that window up to its running round -- what stood behind that place
//! was never in a cached prefix (the notes block moves on, the recall block
//! of the round is decided once). The rule is one per block kind and holds
//! for every role: the three presets below differ in their knobs only, the
//! third with long tool rounds and large results. Forty turns with recalls
//! and tool rounds, each window read off its snapshot row (`sent`).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const TURNS: u32 = 40;

/// The quote of the recall of turn `i`: words the wall does not hold when
/// the block is decided.
fn old_line(i: u32) -> String {
    format!("An old line from before the wall, number {i}.")
}

/// The person's words of turn `i`. Four turns after a recall from outside
/// the wall -- the block is decided and sent by then -- the person says the
/// quoted words themselves: the cut per call of before would now take the
/// block out of the middle of the window.
fn words(i: u32) -> String {
    if i > 5 && i % 5 == 4 {
        old_line(i - 4)
    } else {
        format!("Line {i}: the person tells item number {i} of the day.")
    }
}

fn recall_id(i: u32) -> String {
    format!("call_recall_{i:016x}form")
}

fn recall(i: u32, quoted: &str) -> Value {
    let id = recall_id(i);
    json!([
        user(&words(i)),
        tool_call(&id, "memory_recall"),
        tool_result(&id, &format!("WHAT WAS SAID\n  user: \"{quoted}\""))
    ])
}

/// A tool round of `calls` model calls: every answer but the last asks for
/// one tool, the second result is an error object, the others `chars` long.
fn tool_round(t: &mut Talk, i: u32, calls: u32, chars: usize) {
    let turn = format!("t{i}");
    let id = |j: u32| format!("call_tool_{i:04}_{j:04}");
    let mut c = t.call(&turn, 0, json!([user(&words(i))]));
    for j in 1..calls {
        t.answer(&c, "tool_calls", json!([tool_call(&id(j), "read_file")]));
        let result = if j == 2 {
            "{\"error\": \"exit status 1\"}".to_string()
        } else {
            format!("file {i}/{j}: {}", "x".repeat(chars))
        };
        c = t.call(
            &turn,
            j,
            json!([tool_call(&id(j), "read_file"), tool_result(&id(j), &result)]),
        );
    }
    t.answer(&c, "stop", json!([said(&format!("Done with {i}."))]));
}

/// Forty turns under the preset `preset`: plain turns, a recall whose quote
/// stands in the wall (left out one call later), a recall from outside the
/// wall (stays, also after the person said its words), and tool rounds of
/// `calls` calls.
fn forty_turns(preset: &str, calls: u32, chars: usize) {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(curator_profiles::profile(preset));
    for i in 1..=TURNS {
        let turn = format!("t{i}");
        match i % 5 {
            2 => {
                let c = t.call(&turn, 0, recall(i, &words(i - 1)));
                t.answer(&c, "stop", json!([said(&format!("I remember {i}."))]));
            }
            0 => {
                let c = t.call(&turn, 0, recall(i, &old_line(i)));
                t.answer(
                    &c,
                    "stop",
                    json!([said(&format!("That was long ago {i}."))]),
                );
            }
            3 => tool_round(&mut t, i, calls, chars),
            _ => {
                t.turn(&turn, &words(i), &format!("Noted {i}."));
            }
        }
    }
    // Check of the setup: the run decided on recall blocks both ways.
    assert!(
        t.left_out > 0,
        "a recall block was left out: {:?}",
        t.h.stderr
    );
    let last = t.sent.last().expect("a call").call.messages();
    assert!(
        last.iter().any(|m| m["id"] == json!(recall_id(35))),
        "a recall block that was sent stays, whatever the person says later"
    );
    assert_eq!(
        last.iter()
            .filter(|m| m["text"]
                .as_str()
                .is_some_and(|x| x.contains(&old_line(35))))
            .count(),
        2,
        "check of the setup: the person said the quoted words"
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "{preset}: every window begins with the one before"
    );
}

#[test]
fn the_conversation_preset_only_appends() {
    forty_turns("talky", 2, 200);
}

#[test]
fn the_consultation_preset_only_appends() {
    forty_turns("consult", 3, 2000);
}

#[test]
fn the_coding_preset_with_long_tool_rounds_only_appends() {
    forty_turns("coding", 7, 12000);
}
