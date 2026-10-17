//! GH #1225 (re-check I-4, delta check of KR) -- a ledger that healed once
//! reads its stamp and nothing else on the next child's first round.
//!
//! WHY this file exists: every child of `./intake` heals an older ledger once
//! (phase `h`). Measured: its first read carried the stamp `heal:gh1225`,
//! every pin and 2000 wall rows in one bundle, also when the stamp stood long
//! since -- on every child start, on the serial ledger in front of the next
//! read of `./policy`. Now the stamp is read alone first, and the pins and
//! the wall only when it is missing.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-kch";

fn reads_of_intake(h: &Hive, table: &str) -> Vec<Value> {
    h.ledger_ops
        .iter()
        .filter(|(cell, op)| {
            cell == "intake" && op["operation"] == "select" && op["table"] == table
        })
        .map(|(_, op)| op.clone())
        .collect()
}

fn stamp_reads(h: &Hive) -> usize {
    reads_of_intake(h, "state")
        .iter()
        .filter(|op| op["where"]["key"] == "heal:gh1225")
        .count()
}

/// The heal's read of the pins: every column a copy is told by (step B's
/// `b-pins` reads the hashes only).
fn heal_pin_reads(h: &Hive) -> Vec<Value> {
    reads_of_intake(h, "pins")
        .into_iter()
        .filter(|op| {
            op["columns"]
                .as_array()
                .is_some_and(|c| c.contains(&json!("source")))
        })
        .collect()
}

fn heal_wall_reads(h: &Hive) -> usize {
    reads_of_intake(h, "wall")
        .iter()
        .filter(|op| op["limit"] == json!(2000))
        .count()
}

#[test]
fn a_stamped_ledger_reads_neither_the_pins_nor_the_wall_to_heal() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(&mut h, S, "t1", "I grew up in Leipzig.", "Nice.", json!({}));
    // The first child healed: the stamp stands, its read came first.
    assert!(!h.state("heal:gh1225").is_empty(), "{:?}", h.stderr);
    assert_eq!(stamp_reads(&h), 1, "one stamp read per child");
    assert_eq!(
        heal_pin_reads(&h).len(),
        1,
        "an unstamped ledger reads its pins once"
    );
    assert_eq!(heal_wall_reads(&h), 1, "and one page of its wall");
    // A new child on the stamped ledger: one read of the stamp, nothing else.
    h.ledger_ops.clear();
    h.restart("intake");
    turn(
        &mut h,
        S,
        "t2",
        "My daughter Anna moves to Hamburg.",
        "Good.",
        json!({}),
    );
    assert_eq!(stamp_reads(&h), 1, "the new child reads the stamp once");
    assert!(
        heal_pin_reads(&h).is_empty(),
        "a stamped ledger reads no pins to heal: {:?}",
        heal_pin_reads(&h)
    );
    assert_eq!(
        heal_wall_reads(&h),
        0,
        "a stamped ledger reads no page of its wall to heal"
    );
}
