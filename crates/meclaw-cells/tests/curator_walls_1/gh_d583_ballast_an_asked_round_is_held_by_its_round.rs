//! GH #1528 (d58-3 ballast, R-583-2, review of the bridges M1) -- the plan
//! holds an asked round by its round, not by the short ids of its blocks:
//! two asked rounds of one wording count every block, and a raw round of the
//! same wording counts whole.
//!
//! WHY this file exists: a short id hangs on the content alone (the same
//! text, the same short id), and a round that was only asked -- "What time
//! is it?" -- is asked with the same words on many days. The plan counted
//! the blocks of its asked rounds, and estimated the window, by the set of
//! their short ids: two asked rounds of one wording counted their blocks
//! once, and a raw round of the same wording among the newest was estimated
//! at the one-line form although it goes out whole. Now every block the
//! plan holds is held by `((session_id, turn_id), short id)`, the key
//! `render` reads.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`) on
//! made-up rows.

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

/// The short ids of the words and the answer of the asked round, the same
/// in every round that asks it.
const WORDS: &str = "dddddddddddd";
const ANSWER: &str = "eeeeeeeeeeee";
/// Their sizes: the round is 300 characters, two one-line forms are not.
const WORDS_CHARS: i64 = 50;
const ANSWER_CHARS: i64 = 250;

/// One row of the wall: piece `j` of round `i`.
fn row(i: i64, j: i64, kind: &str, short: &str, chars: i64, final_: i64) -> Value {
    let t = DAY_S + 3600 + 10 * i + j;
    json!({"seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
           "kind": kind, "hash": format!("{short}{}", "0".repeat(52)), "nth": 0,
           "final": final_, "at": "2026-03-01T01:00:00.000000Z", "audience_set": ROUND_E,
           "chars": chars, "type": "text"})
}

/// The wall and the `ballast` marks of the judge: the rounds of `asked` ask
/// the same thing with the same words, every other round is talk.
fn wall(asked: &[i64]) -> (Vec<Value>, Vec<Value>) {
    let (mut rows, mut marks) = (Vec::new(), Vec::new());
    for i in 0..ROUNDS {
        if asked.contains(&i) {
            rows.push(row(i, 0, "user", WORDS, WORDS_CHARS, 0));
            rows.push(row(i, 1, "assistant", ANSWER, ANSWER_CHARS, 1));
            let t = DAY_S + 3600 + 10 * i + 5;
            marks.push(json!({"seq": t * 1_000_000, "session_id": "s",
                              "turn_id": format!("r{i}"), "kind": "ballast",
                              "value": json!({"line": "asked the time",
                                              "seen": [WORDS, ANSWER]}).to_string()}));
        } else {
            rows.push(row(i, 0, "user", &format!("{:012x}", 100 + i), 300, 0));
            rows.push(row(i, 1, "assistant", &format!("{:012x}", 200 + i), 400, 1));
        }
    }
    (rows, marks)
}

/// The plan of a rebuild that only collects (the cover stays at 0), its
/// `gc` classes and its estimate, with the constants the estimate reads.
fn plan_of(rows: &[Value], marks: &[Value]) -> Value {
    let probe = "(lambda o: {'asked': [a['t'] for a in o['plan'].get('asked') or []], \
                 'cover': o['plan']['cover'], 'ballast': o['gc']['classes'].get('ballast'), \
                 'estimate': o['estimate'], 'rate': CHARS_PER_TOKEN, 'form': FORM_CHARS})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None, \
                 collect_only=True, asked=ARGS['marks']))";
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

/// The estimate of the wall with the asked rounds `held` at two one-line
/// forms each.
fn estimate_with(rows: &[Value], held: i64, out: &Value) -> i64 {
    let all: i64 = rows.iter().map(|r| r["chars"].as_i64().unwrap()).sum();
    let form = out["form"].as_i64().unwrap();
    (all - held * (WORDS_CHARS + ANSWER_CHARS) + held * 2 * form) / out["rate"].as_i64().unwrap()
}

/// The same words asked in two rounds outside the newest: both held as
/// asked, four blocks of the class `ballast`.
#[test]
fn two_asked_rounds_of_one_wording_count_every_block() {
    let (rows, marks) = wall(&[1, 2]);
    let out = plan_of(&rows, &marks);
    assert_eq!(out["asked"], json!(["r1", "r2"]), "{out}");
    assert_eq!(out["ballast"], json!(4), "two blocks per round: {out}");
    assert_eq!(
        out["estimate"],
        json!(estimate_with(&rows, 2, &out)),
        "{out}"
    );
}

/// The same words asked in an older round (held as asked) and in the newest
/// round (kept raw): two blocks of the class `ballast`, and the raw round
/// counted whole in the estimate.
#[test]
fn a_raw_round_of_the_same_wording_counts_whole() {
    let (rows, marks) = wall(&[1, ROUNDS - 1]);
    let out = plan_of(&rows, &marks);
    assert_eq!(out["asked"], json!(["r1"]), "{out}");
    assert_eq!(out["ballast"], json!(2), "{out}");
    assert_eq!(
        out["estimate"],
        json!(estimate_with(&rows, 1, &out)),
        "the newest round goes out whole: {out}"
    );
}
