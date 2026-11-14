//! GH #1436 (c) -- with `topic_aging` "0" (the shipped default) the judge
//! read is the read it was: the same store operations, the same answer
//! bytes as master before GH #1436, and an unnamed topic still carries as
//! before.
//!
//! WHY this file exists: the aging is a set's switch; until a set turns it
//! on, no colony may see a byte of it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::*;
use meclaw_core::serde_json::{Value, json};

fn bytes(run: &Run) -> Vec<(Value, Value)> {
    run.store
        .iter()
        .chain(run.out.iter())
        .map(|m| (Value::Object(m.hop.clone()), Value::Object(m.body.clone())))
        .collect()
}

/// The digest of the three judge reads below -- store operations and answers,
/// hop and body -- as the history cell of master BEFORE GH #1436 gave them
/// (fix round 1, review M1: the lock compares with the bytes of master, not
/// the new script with itself). Measured by this test against the history
/// `config.json` of master; a later change that moves these bytes on purpose
/// takes the digest the failure prints, with its reason in the commit.
/// Re-blessed for D58 live (GH #1482, contract points 1 and 3): the seed ask
/// and its answer carry the handle `live` instead of `call_id` -- a wanted
/// change of the seed's keys, old
/// `59dc3d70056151d230868babca5f96699d5d36531fc8cfd24022e3a5efc2f934`.
/// Re-blessed for D58 judge (GH #1480, contract points 1, 2 and 5): the judge
/// read is a view now -- the lane `in_view_ask`, the answer `view` with
/// `messages` and the `system` tree instead of `turns` and `judge`, the seed's
/// key `context` instead of `judge`, and the view reads no screen row (the
/// contributions come by `part_reads`) -- a wanted change of lanes and keys
/// (free by the plan part: lane names and the key `judge` of `seed`), old
/// `6bba02af2c78ddd4aaecd4068b4cbd56049839051647767560f395bd79390828`. Contract point 4 moves the
/// seed's reads too (the held start window lines as two key ranges, `seed_line:` keys without a kind)
/// and the view and the seed read the target's contributions by `part_reads` -- the joined state of
/// both, old `fa96af97ce4afececc9d59d0bd1d9a26b6ed9f873b80f41548fb459ec742f2db`. Review I2 of
/// the strand renamed the seed's write phase `judge-write` to `state-write` (contract point 9:
/// no role name in a phase) -- the only moved byte: the shipped history with the old phase name
/// gives the old digest, old `d61ede91d26a2e1f08201a1dc36767697a30ba277f480808abdf124433b556e6`.
/// Re-blessed for GH #1436 carried notes (GH #1517, `carry_notes`): history reads the column
/// `not_session` of `notes` in its seeds -- a carried note is never shown in the session that bore
/// it, and history reads the column whatever the knob says, since a set that switches
/// `carry_notes` off leaves its rows standing -- so the seed's read of `notes` names one column
/// more; the only moved bytes: the shipped history without that column gives the old digest, old
/// `7095d791f95210cd03fe967f56fa71b6fda641778f2773e99ba71f9497da1b48`. The carried note names the
/// handle of the running session it was born in as well (`not_live`): a seed names a call by its
/// handle, its turns by the session of their generation, so the session alone let the note into
/// the renewed start window of its own call -- the seed's read of `notes` names one column more
/// again; the only moved bytes: the shipped history without that column gives the digest before,
/// old `2cee5b217179ebed444ea1b8f7b11e9b568a6ee25bb6581adc47f30dc6b9cbd3`.
const MASTER_OFF_DIGEST: &str = "5e3c3e0b44993c24c03105c1a0a33f6338a5da76eb97e96ad78d3c22317ddf0c";

fn digest(runs: &[&Run]) -> String {
    let all: Vec<Value> = runs
        .iter()
        .map(|r| Value::Array(bytes(r).into_iter().map(|(h, b)| json!([h, b])).collect()))
        .collect();
    sha256_hex(&canonical(&Value::Array(all)))
}

fn wall() -> Hive {
    let mut h = Hive::new();
    said_turn(
        &mut h,
        1,
        "text-1",
        "t1",
        "I do not want to go on",
        "crisis",
    );
    for i in 1..=5 {
        said_turn(
            &mut h,
            10 + i,
            "voice-1",
            &format!("v{i}"),
            "what time is it",
            "",
        );
    }
    h
}

#[test]
fn the_switch_off_reads_byte_identical() {
    let h = wall();
    let shipped = history_params(&[], &[]);
    let d = delta_judge_ask(&h, &shipped, "call-1", "v6");
    let j = judge_ask(&h, &shipped, "voice-1", "v6");
    let s = seed_ask(&h, &shipped, "call-1");
    assert_eq!(
        digest(&[&d, &j, &s]),
        MASTER_OFF_DIGEST,
        "the shipped judge reads are not the bytes of master"
    );
    assert_eq!(
        judged_topic(&only(&d, "view")),
        "crisis",
        "carries as before"
    );
    assert!(fact_line(&only(&s, "seed")).contains("topic: crisis"));
    let cfg = cell_config("history");
    assert_eq!(cfg["params"]["topic_aging"], "0", "shipped off");
    // Off with every aging param moved: still the same bytes.
    let off = history_params(
        &[
            ("topic_aging", json!("0")),
            ("topic_age_turns", json!(1)),
            ("topic_age_rows", json!(1)),
        ],
        &[],
    );
    let h = wall();
    let d1 = delta_judge_ask(&h, &off, "call-1", "v6");
    let j1 = judge_ask(&h, &off, "voice-1", "v6");
    let s1 = seed_ask(&h, &off, "call-1");
    assert_eq!(
        digest(&[&d1, &j1, &s1]),
        MASTER_OFF_DIGEST,
        "off, params moved"
    );
}
