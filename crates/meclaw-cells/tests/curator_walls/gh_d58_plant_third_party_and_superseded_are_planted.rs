//! GH #1490 (D58 plant) -- the wall `over_aim` plants a fact about a third
//! person and a value that a later row replaces.
//!
//! WHY this file exists: condensing memories lose above all the subject of a
//! statement, and the second most frequent error is an old state kept as
//! valid (MedGraph, arXiv 2611.08433, section 4 no. 1: LightMem 74 %, Mem0
//! 56 %). The bank counts both only if the capture carries them: the wall
//! holds points of the kind `third_party` (the person says a fact about a
//! third person, the point names that person as `subject`) and of the kind
//! `superseded` (a row with a value, and at least two planting beats later a
//! row with the new value; the point carries the keyword of both).
//!
//! Pinned: at least two points of each kind; every keyword is carried by its
//! row alone; both kinds are the person's words; a `third_party` row names its
//! subject; the old row of a `superseded` point stands at least two planting
//! beats (thirty-two rows) before the new one. Nothing here counts what a
//! summary keeps -- that is the bank's reader's (B1).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;

use curator_bank::*;

/// A planting beat of the wall: every eighth turn, two rows a turn.
const BEAT_ROWS: usize = 16;

#[test]
fn third_party_and_superseded_are_planted() {
    let bank = bank();
    let wall = bank.named("over_aim");
    let planted = over_aim::planted();
    let mut found = Findings::default();

    // A keyword is carried by its row alone (no other row of the wall).
    let alone = |key: &str, piece: usize| {
        wall.pieces[piece].key == key
            && wall
                .pieces
                .iter()
                .enumerate()
                .all(|(i, p)| i == piece || !p.text().contains(key))
    };
    let third: Vec<_> = planted.iter().filter(|p| p.kind == "third_party").collect();
    if third.len() < 2 {
        found.note(format!(
            "over_aim: at least two planted third_party, counted {}",
            third.len()
        ));
    }
    for p in &third {
        let row = &wall.pieces[p.piece];
        if !alone(&p.key, p.piece) || !row.text().contains(&p.text) {
            found.note(format!(
                "third_party {}: its row alone carries its keyword and sentence",
                p.key
            ));
        }
        if row.what != What::Words {
            found.note(format!("third_party {}: the person's words", p.key));
        }
        match p.subject {
            Some(s) if !s.is_empty() && p.text.contains(s) => {}
            other => found.note(format!(
                "third_party {}: the point names the third person its sentence is about, got {other:?}",
                p.key
            )),
        }
    }

    let sup: Vec<_> = planted.iter().filter(|p| p.kind == "superseded").collect();
    if sup.len() < 2 {
        found.note(format!(
            "over_aim: at least two planted superseded, counted {}",
            sup.len()
        ));
    }
    for p in &sup {
        let Some(old) = &p.old else {
            found.note(format!(
                "superseded {}: the point carries its old row",
                p.key
            ));
            continue;
        };
        for (key, piece, text) in [(&p.key, p.piece, &p.text), (&old.key, old.piece, &old.text)] {
            let row = &wall.pieces[piece];
            if !alone(key, piece) || !row.text().contains(text.as_str()) {
                found.note(format!(
                    "superseded {key}: its row alone carries its keyword and sentence"
                ));
            }
            if row.what != What::Words {
                found.note(format!("superseded {key}: the person's words"));
            }
        }
        if old.piece + 2 * BEAT_ROWS > p.piece {
            found.note(format!(
                "superseded {}: the old row {} stands two planting beats before the new one \
                 (rows {} and {})",
                p.key, old.key, old.piece, p.piece
            ));
        }
        if old.day > p.day {
            found.note(format!("superseded {}: the old value is said first", p.key));
        }
    }

    // No keyword names two points, and the new kinds take no row of the old.
    let mut all: Vec<&str> = planted
        .iter()
        .filter(|p| p.old.is_none())
        .map(|p| p.key.as_str())
        .collect();
    all.extend(
        sup.iter()
            .filter_map(|p| p.old.as_ref().map(|o| o.key.as_str())),
    );
    all.extend(
        sup.iter()
            .filter(|p| p.old.is_some())
            .map(|p| p.key.as_str()),
    );
    let n = all.len();
    all.sort_unstable();
    all.dedup();
    if all.len() != n {
        found.note(format!(
            "over_aim: {} keywords name two planted rows",
            n - all.len()
        ));
    }
    found.finish();
}
