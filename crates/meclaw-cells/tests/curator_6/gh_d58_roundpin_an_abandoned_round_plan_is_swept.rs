//! GH #1471 (D58 roundpin, contract point 4) -- the plan an abandoned tool
//! round kept is swept.
//!
//! WHY this file exists: a tool round keeps the plan its first call was cut
//! by until it ends. A tool round that never ends -- its tool never brings
//! a result -- would keep it for good. What it left behind is a parked row
//! like every other one, and the sweep of parked rows takes it once it is
//! older than `pending_sweep_s`, on the probe of the next rebuild. A call
//! of that turn that comes after all finds nothing kept and is cut like a
//! first call, by the plan that stands.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! age is set on the parked rows (`Hive::age_parked_rows`), nothing waits.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const SUMMARY: &str = "- The person told items 1 to 4.";
const ID: &str = "call_tool_rpswept_0001";

fn swept(t: &Talk) -> Vec<String> {
    t.h.rows("SELECT value FROM marks WHERE kind = 'pending_swept'")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn gh_d58_roundpin_an_abandoned_round_plan_is_swept() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);

    // A tool round begins, its answer asks for a tool, and the result of
    // the tool never comes. A rebuild writes its plan meanwhile.
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    let began = cut_epoch(&t.h, &c0);
    t.answer(&c0, "tool_calls", json!([tool_call(ID, "read_file")]));
    t.strike(SUMMARY);
    assert!(
        t.cover() > 0,
        "the plan of the rebuild is written while the tool round is out: {:?}",
        t.h.stderr
    );
    assert_ne!(
        standing_epoch(&t.h, ROUND_E),
        began,
        "check of the setup: a newer plan stands than the one the tool round began with"
    );
    assert!(
        swept(&t).is_empty(),
        "check of the setup: nothing swept yet"
    );

    // What the tool round left is two days old; the probe of the next
    // rebuild sweeps it and says so.
    let aged = t.h.age_parked_rows(2);
    assert!(
        !aged.is_empty(),
        "the abandoned tool round left a parked row with its time: {:?}",
        t.h.rows("SELECT key FROM state")
    );
    t.strike(SUMMARY);
    let marks = swept(&t);
    assert_eq!(
        marks.len(),
        1,
        "the sweep says what it took: {marks:?} {:?}",
        t.h.stderr
    );
    assert!(
        aged.iter().any(|(key, _)| marks[0].contains(key.as_str())),
        "the mark names a row the tool round left: {marks:?} {aged:?}"
    );

    // The result comes after all: nothing is kept for this turn any more,
    // the call is cut like a first call.
    let late = t.h.curate(
        "s1",
        "t7",
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
        mode("Be brief."),
    );
    assert_eq!(
        cut_epoch(&t.h, &late),
        standing_epoch(&t.h, ROUND_E),
        "a call of the swept tool round is cut by the plan that stands: {:?}",
        t.h.stderr
    );
    assert!(
        !late.messages().contains(&said("Noted item 1.")),
        "it shows the window of the plan that stands: {:?}",
        late.messages()
    );
}

/// Check of the fixture, without a rebuild in the tool round: a call whose
/// tool result comes within the age of the sweep leaves as the second call
/// of its tool round.
#[test]
fn a_result_that_comes_in_time_goes_on_with_the_round() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    t.answer(&c0, "tool_calls", json!([tool_call(ID, "read_file")]));
    let c1 = t.call(
        "t7",
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
    );
    assert_eq!(cut_epoch(&t.h, &c1), cut_epoch(&t.h, &c0));
    assert_eq!(t.breaks(), Vec::<String>::new());
    assert!(swept(&t).is_empty(), "nothing swept: {:?}", swept(&t));
}
