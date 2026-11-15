//! GH #1528 (d58-3 ballast, R-39-5, review of the bridges M1, M3, M4) --
//! the plan holds a bridge by its round, not by its short id: two bridges of
//! one wording are two blocks, a raw bridge of the same wording counts whole,
//! a round of two bridges and no result keeps both, and an answer before the
//! bridge is no result of it.
//!
//! WHY this file exists: a short id hangs on the content alone (the same
//! text, the same short id), and a bridge such as "One moment, I am looking
//! it up" is said with the same words in many rounds. The plan counted the
//! bridges it left out, and estimated the window, by the set of their short
//! ids: two bridges of one wording counted as one block of the class
//! `ballast`, a raw bridge of the same wording in a newest round was
//! estimated at its one-line form although it goes out whole, and a bridge
//! left out was estimated at a one-line form although nothing of it is sent.
//! Now every block the plan holds is held by `((session_id, turn_id), short
//! id)`, the key `render` reads. Beside it two edges of the bridge itself:
//! the ids of a round's bridge marks are one set (a round of two bridges
//! without its result has no answer but bridges), and only a final answer
//! AFTER the bridge (`seq`) is its result.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`) on
//! made-up rows; the `{}` texts are not needed, the plan reads sizes and ids.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z in seconds; the plan is made a day later.
const DAY_S: i64 = 1_772_323_200;
const AS_OF_MS: i64 = (DAY_S + 86_400) * 1000;

/// The rounds of the wall; the newest is kept raw (`keep_recent` 1).
const ROUNDS: i64 = 5;

/// The short id of the bridge of one wording, said in more than one round.
const BRIDGE: &str = "bbbbbbbbbbbb";
/// The short id of a second bridge, said in the same round as `BRIDGE`.
const OTHER_BRIDGE: &str = "cccccccccccc";

/// One row of the wall: piece `j` of round `i`.
fn row(i: i64, j: i64, kind: &str, short: &str, chars: i64, final_: i64) -> Value {
    let t = DAY_S + 3600 + 10 * i + j;
    json!({"seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
           "kind": kind, "hash": format!("{short}{}", "0".repeat(52)), "nth": 0,
           "final": final_, "at": "2026-03-01T01:00:00.000000Z", "audience_set": ROUND_E,
           "chars": chars, "type": "text"})
}

/// The wall and the `bridge` marks: every round the person's words and an
/// answer, except the rounds of `bridged`, which say the pieces of `order`
/// -- `u` the words, `b` the bridge `BRIDGE`, `c` the bridge `OTHER_BRIDGE`,
/// `r` the result of the look-up --, each bridge marked as the answer of a
/// side call of its own. The marks of a round come latest bridge first: the
/// side calls answer in any order, and a mark read last that named only the
/// earlier bridge would let the later one pass for its result.
fn wall(bridged: &[i64], order: &str) -> (Vec<Value>, Vec<Value>) {
    let (mut rows, mut marks) = (Vec::new(), Vec::new());
    for i in 0..ROUNDS {
        let pieces = if bridged.contains(&i) { order } else { "ua" };
        for (j, c) in pieces.chars().enumerate() {
            let j = j as i64;
            rows.push(match c {
                'u' => row(i, j, "user", &format!("{:012x}", 100 + i), 300, 0),
                'a' => row(i, j, "assistant", &format!("{:012x}", 200 + i), 400, 1),
                'r' => row(i, j, "assistant", &format!("{:012x}", 300 + i), 500, 1),
                'b' => row(i, j, "assistant", BRIDGE, 70, 1),
                'c' => row(i, j, "assistant", OTHER_BRIDGE, 80, 1),
                _ => unreachable!("a piece of the wall: {c}"),
            });
        }
        if !bridged.contains(&i) {
            continue;
        }
        let ids = pieces
            .chars()
            .filter_map(|c| match c {
                'b' => Some(BRIDGE),
                'c' => Some(OTHER_BRIDGE),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (k, id) in ids.into_iter().rev().enumerate() {
            let t = DAY_S + 3600 + 10 * i + 5 + k as i64;
            marks.push(json!({"seq": t * 1_000_000, "session_id": "s",
                              "turn_id": format!("r{i}"), "kind": "side_answer",
                              "value": json!({"call": format!("c{i}-{k}"), "ids": [id]})
                                  .to_string()}));
        }
    }
    (rows, marks)
}

/// The plan of a rebuild that only collects (the cover stays at 0, every
/// row above it), its `gc` classes and its estimate, with the constants the
/// estimate reads.
fn plan_of(rows: &[Value], marks: &[Value]) -> Value {
    let probe = "(lambda o: {'bridged': o['plan'].get('bridged') or [], \
                 'cover': o['plan']['cover'], 'ballast': o['gc']['classes'].get('ballast'), \
                 'estimate': o['estimate'], 'rate': CHARS_PER_TOKEN})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None, \
                 collect_only=True, bridges=ARGS['marks']))";
    let (out, err) = policy_scope(
        json!({"keep_recent": 1, "keep_recent_tokens": null}),
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS, "marks": marks}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(
        out["cover"],
        json!(0),
        "a collection keeps the cover: {out}"
    );
    out
}

/// The characters of the wall less `left_out`, in tokens as the plan
/// estimates them.
fn estimate_without(rows: &[Value], left_out: i64, rate: i64) -> i64 {
    let all: i64 = rows.iter().map(|r| r["chars"].as_i64().unwrap()).sum();
    (all - left_out) / rate
}

/// The rounds the plan left a bridge out of, by turn.
fn bridged_turns(plan: &Value) -> Vec<String> {
    plan["bridged"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["t"].as_str().unwrap().to_string())
        .collect()
}

/// M1: the same words bridged in two rounds, both with their result,
/// outside the newest round: two blocks of the class `ballast`, and nothing
/// of either in the estimate.
#[test]
fn two_bridges_of_one_wording_count_two_blocks() {
    let (rows, marks) = wall(&[1, 2], "ubr");
    let out = plan_of(&rows, &marks);
    assert_eq!(bridged_turns(&out), vec!["r1", "r2"], "{out}");
    assert_eq!(
        out["ballast"],
        json!(2),
        "each bridge left out is a block: {out}"
    );
    let rate = out["rate"].as_i64().unwrap();
    assert_eq!(
        out["estimate"],
        json!(estimate_without(&rows, 2 * 70, rate)),
        "a bridge left out sends nothing: {out}"
    );
}

/// M1: the same words bridged in an older round (left out) and in the
/// newest round (kept raw): one block of the class `ballast`, and the raw
/// bridge counted whole in the estimate.
#[test]
fn a_raw_bridge_of_the_same_wording_counts_whole() {
    let (rows, marks) = wall(&[1, ROUNDS - 1], "ubr");
    let out = plan_of(&rows, &marks);
    assert_eq!(bridged_turns(&out), vec!["r1"], "{out}");
    assert_eq!(out["ballast"], json!(1), "{out}");
    let rate = out["rate"].as_i64().unwrap();
    assert_eq!(
        out["estimate"],
        json!(estimate_without(&rows, 70, rate)),
        "the bridge of the newest round goes out whole: {out}"
    );
}

/// M3 (i): a round of two bridges -- two side calls -- and no result: each
/// bridge is the other's only "answer", and the ids of the round's marks are
/// one set, so neither is left out.
#[test]
fn a_round_of_two_bridges_without_a_result_keeps_both() {
    let (rows, marks) = wall(&[1], "ubc");
    assert_eq!(marks.len(), 2, "two side calls, two marks");
    let out = plan_of(&rows, &marks);
    assert_eq!(out["bridged"], json!([]), "both bridges stay raw: {out}");
    assert_eq!(out["ballast"], Value::Null, "nothing left out: {out}");
}

/// M4: a final answer that stands before the bridge is no result of its
/// look-up; the bridge is still the last word of its round and stays raw.
#[test]
fn an_answer_before_the_bridge_is_no_result() {
    let (rows, marks) = wall(&[1], "urb");
    let out = plan_of(&rows, &marks);
    assert_eq!(out["bridged"], json!([]), "the bridge stays raw: {out}");
}
