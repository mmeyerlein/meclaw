//! D58 judge (GH #1480, contract point 3) -- the contributions to a target's
//! section are ONE text in every curator cell that holds them.
//!
//! WHY this file exists: a contribution is written by two cells and read by
//! two. `./history` keeps it from the lane `in_part` and reads the family of a
//! target for a view, a start window and the live facts; `./intake` keeps it
//! from the `parts` of a curate (in the curate's own store bundle); `./policy`
//! reads the family for the live feed of a running session and digests it
//! with the seed's rule (`seed_fact_state`). A key written by one rule and
//! read by another finds nothing, silently (the seed's and the feed's
//! digests parted once that way, GH #1436). So each function of the block
//! `# >>> parts` stands byte for byte the same in every cell that holds it,
//! and the block holds the functions the cell needs, nothing of its own.
//! Review I3 added `leaf_names` and `family_names` to the block of
//! `./history` and `./policy`: the seed's fact line, the live facts and the
//! live feed name a family with one reading.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use std::collections::BTreeMap;

/// The functions of the block `# >>> parts` ... `# <<< parts` of `cell`'s
/// script, by name, each its whole text.
fn parts_block(cell: &str) -> BTreeMap<String, String> {
    let script = script_of(cell);
    let start = script
        .find("# >>> parts")
        .unwrap_or_else(|| panic!("{cell}: no block `# >>> parts`"));
    let end = script[start..]
        .find("# <<< parts")
        .unwrap_or_else(|| panic!("{cell}: the block `# >>> parts` is not closed"))
        + start;
    let block = &script[start..end];
    let mut out = BTreeMap::new();
    let mut name = String::new();
    let mut text = String::new();
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("def ") {
            if !name.is_empty() {
                out.insert(std::mem::take(&mut name), text.trim_end().to_string());
            }
            name = rest.split('(').next().unwrap_or("").to_string();
            text.clear();
        }
        if !name.is_empty() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if !name.is_empty() {
        out.insert(name, text.trim_end().to_string());
    }
    out
}

#[test]
fn gh_d58_judge_parts_are_one_text() {
    let want: [(&str, &[&str]); 3] = [
        (
            "history",
            &[
                "family_names",
                "leaf_names",
                "part_family",
                "part_key",
                "part_leaf",
                "part_now_ms",
                "part_range",
                "part_row",
                "part_until_ms",
                "parts_text",
            ],
        ),
        (
            "policy",
            &[
                "family_names",
                "leaf_names",
                "part_family",
                "part_key",
                "part_leaf",
                "part_now_ms",
                "part_range",
                "part_until_ms",
                "parts_text",
            ],
        ),
        ("intake", &["part_key", "part_row", "part_until_ms"]),
    ];
    let blocks: Vec<(&str, BTreeMap<String, String>)> =
        want.iter().map(|(c, _)| (*c, parts_block(c))).collect();
    for ((cell, names), (_, block)) in want.iter().zip(&blocks) {
        let got: Vec<&str> = block.keys().map(String::as_str).collect();
        assert_eq!(got, names.to_vec(), "{cell}: the functions of its block");
    }
    let (_, reference) = &blocks[0];
    for (cell, block) in &blocks[1..] {
        for (name, text) in block {
            assert_eq!(
                text, &reference[name],
                "`{name}` in ./{cell} is not the text of ./history"
            );
        }
    }
}
