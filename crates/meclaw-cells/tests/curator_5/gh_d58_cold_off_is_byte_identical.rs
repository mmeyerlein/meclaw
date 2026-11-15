//! GH #1464 (D58 coldgc, contract point 1) -- `gc_on_cold` off is
//! byte-identical.
//!
//! WHY this file exists: the knob ships "0", and a colony that never turns
//! it must run the curator of before -- the same stroke of the cache, the
//! same plan, the same marks, the same window for the next call, and no
//! ledger operation more. The arm with the knob on is the control: without
//! it the comparison would pass on a tree that knows no such knob.
//!
//! What this file compares (review coldgc M3): the knob unset against "0",
//! both on the code at hand -- "0" is the default and writes nothing of its
//! own. That the default is the curator before the strand is held by the
//! corpus `gh_d38_c0_policy_corpus_is_byte_identical` (its fixtures drive
//! strokes of the clock and stayed untouched by the strand) and by the
//! reading of the code: every new branch hangs on `GC_ON_COLD`. Not byte
//! identical by the plan: `amortise_after` (contract point 6, whatever the
//! knob) and `"cold": 0` in `rebuild.by_mark` of the stats.
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
use meclaw_core::serde_json::Value;

/// The conversation with garbage, the stroke of the cache, one more call:
/// the hive, what the round's plan names and the texts of that call.
fn run(gc_on_cold: Option<&str>) -> (Talk, Vec<String>, Vec<String>) {
    let (mut t, _, _) = garbage_talk("talky", &cold_knobs(gc_on_cold));
    cache_expires(&mut t);
    let cut = shrunk(&t);
    let window = next_window(&mut t, "t5");
    (t, cut, window)
}

fn kinds(h: &Hive) -> Vec<Value> {
    h.rows("SELECT kind, count(*) FROM marks GROUP BY kind ORDER BY kind")
        .into_iter()
        .map(Value::from)
        .collect()
}

#[test]
fn gh_d58_cold_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let (plain, p_cut, p_window) = run(None);
    let (off, o_cut, o_window) = run(Some("0"));
    let (on, _, _) = run(Some("1"));

    assert_eq!(o_window, p_window, "off sends the window of before");
    assert_eq!(o_cut, p_cut, "off cuts what was cut before");
    assert_eq!(
        kinds(&off.h),
        kinds(&plain.h),
        "off writes the marks of before"
    );
    assert_eq!(
        cold_marks(&off.h),
        Vec::<Value>::new(),
        "off knows no `cold` mark"
    );
    assert_eq!(
        cold_marks(&plain.h),
        Vec::<Value>::new(),
        "unset knows no `cold` mark"
    );
    assert_eq!(
        off.h.ledger_ops.len(),
        plain.h.ledger_ops.len(),
        "off runs no ledger operation more"
    );
    assert_eq!(
        off.h.summ.len(),
        plain.h.summ.len(),
        "off asks the summarizer as before"
    );

    // The control: on, the same run leaves the mark of the cold collection.
    assert_eq!(
        cold_marks(&on.h).len(),
        1,
        "with `gc_on_cold` \"1\" the stroke leaves its mark: {:?}",
        on.h.stderr
    );
}
