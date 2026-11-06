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
const MASTER_OFF_DIGEST: &str = "59dc3d70056151d230868babca5f96699d5d36531fc8cfd24022e3a5efc2f934";

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
        judged_topic(&only(&d, "turns")),
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
