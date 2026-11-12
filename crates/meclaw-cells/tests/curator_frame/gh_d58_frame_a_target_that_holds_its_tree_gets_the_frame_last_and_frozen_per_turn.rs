//! D58 frame (GH #1478, frame slot point 1) -- a target that holds its tree
//! gets the frame last and frozen per turn.
//!
//! WHY this file exists: the leaf `frame` carries the time to the minute. A
//! target that holds its system tree (`system: delta`) keeps a provider
//! cache over the part of the prompt that does not move; a leaf that changes
//! every turn in the middle of that tree breaks the cache of everything laid
//! out after it (plan-review-schluss R-W2). So for such a target the leaf
//! stands last in the system part, in the order the snapshot lists it (the
//! model cell's order: reported families first, the rest alphabetically,
//! then the families named after the rest marker `"*"`, GH #1478; such a
//! target's `system_order` ends in `"*", "frame"`). Within a turn it freezes
//! with the system part of the turn: the tool round of the same turn carries
//! the time of the first call. The next turn moves the time, and only the frame's family is
//! sent again -- the families before it stay. Shipped, no target that holds
//! its tree sets `frame`.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) on the corpus
//! clock (`curator_target::PRELUDE`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const LOOKUP: &str = "call_weather_00000000frame1";

/// The text of the leaf `frame` the call carried by the ledger's record
/// (`call_blocks`): a call that holds its tree may not send it again.
fn listed_frame(h: &Hive, call: &Msg) -> Option<String> {
    listed_window(h, call)
        .0
        .into_iter()
        .find(|(p, _)| p == "frame")
        .map(|(_, t)| t)
}

/// The model cells of the shipped compositions a curator cuts for.
const SHIPPED_TARGETS: [&str; 7] = [
    "talky/brain",
    "talky/quick",
    "talky/quick_spec",
    "talky/quick_bridge",
    "cogny/brain",
    "simple-agent/brain",
    "simple-agent/strong",
];

#[test]
fn gh_d58_frame_a_target_that_holds_its_tree_gets_the_frame_last_and_frozen_per_turn() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let mut h = Hive::with_prelude(
        &[
            ("policy", "place", json!("Springfield")),
            ("policy", "timezone", json!("Europe/Berlin")),
            ("policy", "snapshot_window", json!("1")),
        ],
        PRELUDE,
    );
    h.set_clock(T0_US);
    // The cell of a target that holds its tree and sets `frame` lays the
    // frame after every other family: the rest marker of its `system_order`
    // (GH #1478), as a composite with such a target writes it.
    announce(
        &mut h,
        "wall",
        roomy("test/wall"),
        json!({"system": "delta", "frame": true}),
        json!(["*", "frame"]),
    );

    // Turn 1, first call: a minute past the corpus clock.
    h.set_clock(T0_US + 60_000_000);
    let first = say(&mut h, Some("wall"), "t1", "Will it rain? Look it up.");
    let f1 = listed_frame(&h, &first);
    if f1.is_none() {
        failures.push(format!("turn 1: no leaf `frame`: {:?}", h.stderr));
    }
    let paths = sent_system_paths(&h, &first);
    if paths.last().map(String::as_str) != Some("frame") {
        failures.push(format!(
            "turn 1: `frame` is not the last leaf of the system part: {paths:?}"
        ));
    }
    h.set_clock(T0_US + 90_000_000);
    answer(
        &mut h,
        &first,
        "tool_calls",
        warm_usage(),
        json!([tool_call(LOOKUP, "weather")]),
    );

    // Turn 1, the tool round, three minutes later: the time of the first call.
    h.set_clock(T0_US + 240_000_000);
    let calls = curate_for(
        &mut h,
        Some("wall"),
        "t1",
        1,
        json!([
            user("Will it rain? Look it up."),
            tool_call(LOOKUP, "weather"),
            tool_result(LOOKUP, "dry all day")
        ]),
        json!({}),
    );
    let round = the_call(&h, &calls);
    let f2 = listed_frame(&h, &round);
    if f2 != f1 {
        failures.push(format!(
            "turn 1, tool round: the frame moved within the turn: {f1:?} -> {f2:?}"
        ));
    }
    let paths = sent_system_paths(&h, &round);
    if paths.last().map(String::as_str) != Some("frame") {
        failures.push(format!(
            "turn 1, tool round: `frame` is not the last leaf: {paths:?}"
        ));
    }
    h.set_clock(T0_US + 250_000_000);
    answer(
        &mut h,
        &round,
        "stop",
        warm_usage(),
        json!([said("It stays dry.")]),
    );

    // Turn 2, ten minutes later: the time moves, the families before the
    // frame are not sent again.
    h.set_clock(T0_US + 600_000_000);
    let next = say(&mut h, Some("wall"), "t2", "Thank you.");
    let f3 = listed_frame(&h, &next);
    if f3.is_none() || f3 == f1 {
        failures.push(format!("turn 2: the frame did not move: {f1:?} -> {f3:?}"));
    }
    let system = next.body.get("system").cloned().unwrap_or(Value::Null);
    if system.get("frame").is_none() {
        failures.push(format!("turn 2: the moved frame is not sent: {system}"));
    }
    for unmoved in ["persona", "tools"] {
        if system.get(unmoved).is_some() {
            failures.push(format!(
                "turn 2: the unmoved family `{unmoved}` is sent again: {system}"
            ));
        }
    }
    let paths = sent_system_paths(&h, &next);
    if paths.last().map(String::as_str) != Some("frame") {
        failures.push(format!("turn 2: `frame` is not the last leaf: {paths:?}"));
    }

    // Shipped: no target that holds its tree sets `frame`.
    for cell in SHIPPED_TARGETS {
        let p =
            read_json(&repo(&format!("templates/{cell}/config.json")))["params"]["window_profile"]
                .clone();
        if p["system"] == json!("delta") && p["frame"] == json!(true) {
            failures.push(format!("{cell}: holds its tree and sets `frame`"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
