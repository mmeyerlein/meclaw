//! GH #1464 (D58 coldgc, addendum R-58-7) -- the collection is the same for
//! every role.
//!
//! WHY this file exists: the curator keeps the window of every model, and a
//! rule of it holds per kind of block, never per role. Roles differ by their
//! presets and by nothing else: the same conversation under the same knobs
//! is cut at the same places whichever preset the hive runs, and the mark
//! counts the same classes. Which role gets the cold collection is the
//! set's word; the code asks for no name.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The cuts and the classes of the cold collection under the preset `role`,
/// with every knob the collection reads set to the same value.
fn collected(role: &str) -> (Vec<String>, Value, usize) {
    let mut over = cold_knobs(Some("1"));
    over.push(("policy", "stub_tools_after", json!(20)));
    let (mut t, old, _) = garbage_talk(role, &over);
    cache_expires(&mut t);
    assert!(
        t.h.summ.is_empty(),
        "{role}: no model is asked: {:?}",
        t.h.stderr
    );
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "{role}: the old result is collected: {:?}",
        t.h.stderr
    );
    let gc = marks_of(&t.h, "gc");
    assert_eq!(gc.len(), 1, "{role}: one `gc` mark: {gc:?}");
    (shrunk(&t), gc[0]["classes"].clone(), cold_marks(&t.h).len())
}

#[test]
fn gh_d58_cold_the_collection_is_the_same_for_every_role() {
    if !shipped() {
        return;
    }
    let first = collected("talky");
    assert_eq!(first.2, 1, "one `cold` mark");
    for role in ["consult", "coding", "research", "quick"] {
        assert_eq!(
            collected(role),
            first,
            "{role}: the same wall, the same knobs, the same cuts"
        );
    }
}
