//! D58 note (GH #1477, contract point 8) -- with the knob on, a `turn` note
//! behaves as before.
//!
//! WHY this file exists: `once` is a new scope beside `turn`, not a new
//! meaning of it. A `turn` note keeps today's life with `note_once` "1": it
//! stands in the calls of the next turn, is used up when the first window is
//! built (shown or left out), never answers, and its row, its marks and every
//! window are byte for byte those of a curator without the knob.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use note_once::*;

/// What one run leaves: every call's messages, the rows, the marks, and how
/// many answers left.
type Run = (Vec<Vec<Value>>, Vec<Vec<Value>>, Vec<Value>, usize);

/// One run of the same conversation: every call's messages, the rows, the
/// marks, and whether anything answered.
fn run(h: &mut Hive) -> Run {
    let mut windows = Vec::new();
    turn(h, "s1", "t1", "a", "b", json!({}));
    post(
        h,
        json!([{"id": "n1", "source": "probe", "text": "Ask one question.",
                "scope": "turn", "receipt": "r-1"}]),
    );
    let c0 = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let mut answered = receipts(h).len();
    windows.push(c0.messages());
    h.tap(
        &c0,
        "tool_calls",
        json!({}),
        json!([tool_call("x1", "look")]),
    );
    let c1 = h.curate("s1", "t2", 1, json!([]), mode("Be brief."));
    answered += receipts(h).len();
    windows.push(c1.messages());
    h.tap(&c1, "stop", json!({}), json!([said("d")]));
    let c2 = h.curate("s1", "t3", 0, json!([user("e")]), mode("Be brief."));
    answered += receipts(h).len();
    windows.push(c2.messages());
    // A mark names its call by a fresh id per hive; the rest is compared.
    let marks = note_marks(h)
        .into_iter()
        .map(|mut m| {
            m.as_object_mut().map(|o| o.remove("call"));
            m
        })
        .collect();
    (windows, note_rows(h), marks, answered)
}

#[test]
fn a_turn_note_behaves_as_before() {
    if !shipped() {
        return;
    }
    let mut before = share_hive(&[]);
    let mut on = once_hive(&[]);
    let (w0, rows0, marks0, n0) = run(&mut before);
    let (w1, rows1, marks1, n1) = run(&mut on);
    assert!(
        w1[0]
            .iter()
            .any(|m| m.to_string().contains("Ask one question.")),
        "shown in its turn: {:?}",
        w1[0]
    );
    assert_eq!(w1, w0, "every window byte for byte");
    assert_eq!(rows1, rows0, "the row");
    assert_eq!(rows1[0][2], json!("s1#t2"), "used by its turn");
    assert_eq!(marks1, marks0, "the marks");
    assert_eq!((n0, n1), (0, 0), "a turn note never answers");
    assert_eq!(on.emit_violations, Vec::<String>::new());
}
