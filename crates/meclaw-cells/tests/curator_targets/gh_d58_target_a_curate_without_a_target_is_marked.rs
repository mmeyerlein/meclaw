//! D58 target (GH #1483, contract point 1) -- a curate without a target is
//! marked.
//!
//! WHY this file exists: there is no built-in way any more that a curate
//! without a target falls back to. Such a curate still gets its window --
//! the window of the one target the curator knows when it knows exactly one,
//! else a window on the values of the instance -- and a mark of the kind
//! `target_unknown` that says the name was missing, so the omission of a composite
//! is visible in the ledger instead of silently cut for some model.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_target_a_curate_without_a_target_is_marked() {
    if !shipped() {
        return;
    }
    // One reported target: the curate without a name gets its window.
    let mut one = Hive::new();
    announce(
        &mut one,
        "only",
        row("test/only", 20, 30),
        json!({}),
        json!([]),
    );
    let c = say(&mut one, None, "t1", "Hello there.");
    assert_eq!(
        c.hop.get("target"),
        Some(&json!("only")),
        "the one known target takes the curate that names none: {:?} {:?}",
        c.hop,
        one.stderr
    );
    assert!(
        has_overflow_cut(&c),
        "the window is cut by that target's row: {:?}",
        c.hop
    );
    let marks = marks_of_kind(&one, TARGET_MARK);
    assert_eq!(
        marks.len(),
        1,
        "one `target_unknown` mark: {marks:?} {:?}",
        one.stderr
    );
    assert_eq!(marks[0]["missing"], json!("target"), "{marks:?}");
    assert_eq!(
        marks[0]["call"], c.hop["curator_call"],
        "the mark names the call: {marks:?}"
    );

    // Two reported targets: none is guessed, the window runs on the
    // values of the instance and is marked the same way.
    let mut two = Hive::new();
    announce(
        &mut two,
        "first",
        row("test/first", 20, 30),
        json!({}),
        json!([]),
    );
    announce(
        &mut two,
        "second",
        roomy("test/second"),
        json!({}),
        json!([]),
    );
    let c = say(&mut two, None, "t1", "Hello there.");
    assert!(
        c.hop
            .get("target")
            .is_none_or(|t| t != "first" && t != "second"),
        "no target is guessed among two: {:?}",
        c.hop
    );
    assert!(
        !has_overflow_cut(&c),
        "neither row cuts a window it was not named for: {:?}",
        c.hop
    );
    let marks = marks_of_kind(&two, TARGET_MARK);
    assert_eq!(marks.len(), 1, "{marks:?} {:?}", two.stderr);
    assert_eq!(marks[0]["missing"], json!("target"), "{marks:?}");

    // A named target is no such case: no mark.
    let mut named = Hive::new();
    announce(
        &mut named,
        "first",
        roomy("test/first"),
        json!({}),
        json!([]),
    );
    say(&mut named, Some("first"), "t1", "Hello there.");
    assert!(
        marks_of_kind(&named, TARGET_MARK).is_empty(),
        "a named, reported target is not marked"
    );
}
