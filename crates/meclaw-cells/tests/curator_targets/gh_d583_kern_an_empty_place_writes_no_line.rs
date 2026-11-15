//! D58-3 kern (GH #1524, contract point 6, N14) -- an empty place writes no
//! line in the history's frame.
//!
//! WHY this file exists: `./history` gives a view the leaf `frame`, the
//! moment of the read context as facts, one per line. With `place` unset (the
//! shipped value) it still wrote the line `place: ` with nothing after it: a
//! fact without a value, which an asker renders as "the person is nowhere".
//! An empty place writes no line; a set place writes its line as before.
//!
//! Reads the pure half of the shipped cell (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn frame_text(place: &str) -> String {
    let (got, err) = cell_scope(
        "history",
        json!({"place": place}),
        "frame_leaf(context_of([], ''))['text']",
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    got.as_str()
        .unwrap_or_else(|| panic!("the frame is text: {got} {err}"))
        .to_string()
}

#[test]
fn gh_d583_kern_an_empty_place_writes_no_line() {
    if !shipped() {
        return;
    }
    let set = frame_text("Lisbon");
    assert!(
        set.lines().any(|l| l == "place: Lisbon"),
        "a set place writes its line: {set:?}"
    );
    for empty in ["", "   "] {
        let text = frame_text(empty);
        assert!(
            !text.lines().any(|l| l.starts_with("place:")),
            "an empty place ({empty:?}) writes no line: {text:?}"
        );
        assert!(
            text.lines().any(|l| l.starts_with("now: ")),
            "the moment stays: {text:?}"
        );
    }
}
