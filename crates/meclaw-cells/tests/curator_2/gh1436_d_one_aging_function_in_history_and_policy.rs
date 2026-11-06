//! GH #1436 (fix round 1, review C2) -- the topic ages by ONE rule in both
//! cells that read it: `./history` (the route stamp, the judge, the seed of a
//! call) and `./policy` (the live feed of a call). Each carries the same pure
//! function `topic_aged(topic, named, wall, session_now, opened_seq, full)`
//! behind the same switch `topic_aging` and the same params; this lock holds
//! the twins equal on one table of inputs, and pins (a) and (b) of the issue
//! on each cell's function itself.
//!
//! WHY this file exists: with the aging in `./history` alone, the seed of a
//! call said no topic while the policy's feed still read the newest named
//! topic of the round -- the digests of `seed_facts:<call>` parted, every
//! closed voice turn fed the old topic back into the call and renewed its
//! session. Two readers of one fact need one rule; two copies of the rule
//! need a lock that they stay one.
//!
//! The arguments, in the cells' words:
//! * `topic` -- the newest named topic of the read (`topic_now`), "" none;
//! * `named` -- the mark that named it: `{seq, session_id, turn_id?}`;
//! * `wall` -- the person's rows (`kind` `user`, one per turn) of the read's
//!   scope, newest first, through the gate, the running turn left out:
//!   `{seq, session_id, turn_id}` -- the turns are counted here, not marks;
//! * `session_now` -- the session that asks, "" when the context does not
//!   know it (then the session of the newest row of `wall`);
//! * `opened_seq` -- the seq the asking call opened at, 0 unknown;
//! * `full` -- the `wall` read came back with `topic_age_rows` rows.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CELLS: [&str; 2] = ["history", "policy"];

fn named(seq: i64, session: &str, turn: &str) -> Value {
    json!({"seq": seq, "session_id": session, "turn_id": turn})
}

/// `n` turns of `session`, one `user` row each, from seq `from` on, newest
/// first as the read gives them.
fn turns(session: &str, from: i64, n: i64) -> Vec<Value> {
    (0..n)
        .rev()
        .map(|i| {
            json!({"seq": from + 100 * i, "session_id": session,
                        "turn_id": format!("{session}-{i}")})
        })
        .collect()
}

/// `(case, args, expected with the aging on and topic_age_turns 4)`.
fn table() -> Vec<(&'static str, Value, &'static str)> {
    let crisis = named(1_000, "text-1", "t1");
    let mut renamed_wall = turns("voice-1", 3_000, 3);
    renamed_wall.push(json!({"seq": 2_000, "session_id": "voice-1", "turn_id": "v0"}));
    let mut own_turn = turns("text-1", 2_000, 3);
    own_turn.push(json!({"seq": 990, "session_id": "text-1", "turn_id": "t1"}));
    vec![
        // (a) another session, over the bound
        (
            "a: four quiet turns of another session",
            json!([
                "crisis",
                crisis,
                turns("voice-1", 2_000, 4),
                "voice-1",
                0,
                false
            ]),
            "",
        ),
        // (a) another session, under the bound: the session alone ends it
        (
            "a: one turn of another session",
            json!([
                "crisis",
                crisis,
                turns("voice-1", 2_000, 1),
                "voice-1",
                0,
                false
            ]),
            "",
        ),
        // (a) the session unknown to the context: the newest row names it
        (
            "a: session from the newest row",
            json!(["crisis", crisis, turns("voice-1", 2_000, 1), "", 0, false]),
            "",
        ),
        // (a') a call opened after the naming, no turn of it on the wall yet
        (
            "a': opened after, zero turns",
            json!(["crisis", crisis, [], "", 1_500, false]),
            "",
        ),
        (
            "a': opened after, newest row still the naming session",
            json!(["crisis", crisis, turns("text-1", 990, 1), "", 1_500, false]),
            "",
        ),
        // (b) named again in the new session
        (
            "b: named again in the asking session",
            json!([
                "crisis",
                named(2_500, "voice-1", "v1"),
                renamed_wall,
                "voice-1",
                1_500,
                false
            ]),
            "crisis",
        ),
        // the bound counts turns of the own session; the naming turn's own
        // row does not count
        (
            "b: three quiet turns in its own session",
            json!(["crisis", crisis, own_turn, "text-1", 0, false]),
            "crisis",
        ),
        (
            "a: four quiet turns in its own session",
            json!([
                "crisis",
                crisis,
                turns("text-1", 2_000, 4),
                "text-1",
                0,
                false
            ]),
            "",
        ),
        // a full read that never reached the naming counts as the bound
        (
            "a: full read short of the naming",
            json!([
                "crisis",
                crisis,
                turns("text-1", 2_000, 2),
                "text-1",
                0,
                true
            ]),
            "",
        ),
        (
            "no topic stays no topic",
            json!([
                "",
                Value::Null,
                turns("text-1", 2_000, 1),
                "text-1",
                0,
                false
            ]),
            "",
        ),
    ]
}

fn aged(cell: &str, over: Value, args: &[Value]) -> Vec<Value> {
    let (got, err) = cell_scope(
        cell,
        over,
        "[topic_aged(*a) for a in ARGS]",
        Value::Array(args.to_vec()),
    );
    assert!(err.is_empty(), "{cell}: the load said: {err}");
    got.as_array().expect("a list").clone()
}

fn on(n: u64) -> Value {
    json!({"topic_aging": "1", "topic_age_turns": n})
}

#[test]
fn both_cells_age_by_the_issue_pins() {
    let table = table();
    let args: Vec<Value> = table.iter().map(|(_, a, _)| a.clone()).collect();
    for cell in CELLS {
        let got = aged(cell, on(4), &args);
        for ((case, _, want), got) in table.iter().zip(got) {
            assert_eq!(got, json!(want), "{cell}: {case}");
        }
    }
}

#[test]
fn the_twins_age_alike_on_every_input() {
    let table = table();
    let args: Vec<Value> = table.iter().map(|(_, a, _)| a.clone()).collect();
    for over in [
        on(4),
        on(1),
        on(5),
        json!({"topic_aging": "0", "topic_age_turns": 4}),
    ] {
        let h = aged("history", over.clone(), &args);
        let p = aged("policy", over.clone(), &args);
        assert_eq!(h, p, "the twins part under {over}");
    }
}

#[test]
fn the_switch_off_leaves_the_topic_as_read() {
    let table = table();
    let args: Vec<Value> = table.iter().map(|(_, a, _)| a.clone()).collect();
    for cell in CELLS {
        let got = aged(cell, json!({"topic_aging": "0"}), &args);
        for ((case, a, _), got) in table.iter().zip(got) {
            assert_eq!(got, a[0], "{cell} off: {case}");
        }
    }
}

#[test]
fn both_cells_declare_the_same_knobs_and_presets() {
    let h = cell_config("history");
    let p = cell_config("policy");
    for knob in ["topic_aging", "topic_age_turns", "topic_age_rows"] {
        assert!(!h["params"][knob].is_null(), "history declares {knob}");
        assert_eq!(h["params"][knob], p["params"][knob], "the preset of {knob}");
        assert_eq!(
            h["contract"]["settings"][knob], p["contract"]["settings"][knob],
            "the setting of {knob}"
        );
    }
    assert_eq!(h["params"]["topic_aging"], "0", "shipped off");
}
