//! GH #1464 (D58 coldgc, contract points 2 and 4) -- no summary under the aim.
//!
//! WHY this file exists: the cold collection is the part of a rebuild that
//! costs nothing and loses nothing. Condensing is the other part: a model
//! call, and words replaced by a brief -- it stays what a window over its
//! aim pays, never what a pause costs. So with `gc_on_cold` "1" the stroke
//! of an expired cache hands no row to the summarizer while the window is
//! under its aim, even where a rebuild would (a day gone by, rounds beyond
//! the raw ones): the older day stays in the words it was said in.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const YESTERDAY: &str = "Yesterday I planted the apple tree behind the shed.";

#[test]
fn gh_d58_cold_no_summary_under_the_aim() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // One round raw and no token budget: the day before is in its summary
    // tier at any rebuild of today.
    let mut over = cold_knobs(Some("1"));
    over.push(("policy", "keep_recent", json!(1)));
    over.push(("policy", "keep_recent_tokens", json!(0)));
    let mut t = Talk::new(&over).reporting_composite("talky");
    let at = days_ago(1, 9);
    t.h.row(at, "s1", "y1", "user", &user(YESTERDAY), 0);
    t.h.row(
        at + chrono::Duration::seconds(1),
        "s1",
        "y1",
        "assistant",
        &said("A good place for it."),
        1,
    );
    t.turn("t1", WORDS[0], ANSWERS[0]);
    let old = tool_turn(&mut t, "t2", 1, WORDS[1], ANSWERS[1]);
    tool_turn(&mut t, "t3", 2, WORDS[2], ANSWERS[2]);
    t.turn("t4", WORDS[3], ANSWERS[3]);

    cache_expires(&mut t);
    assert!(
        t.h.summ.is_empty(),
        "under the aim the summarizer is not asked: {:?}",
        t.h.stderr
    );
    let plan = t.h.plan_in(ROUND_E);
    assert!(
        plan["summary"].as_str().unwrap_or("").is_empty(),
        "the plan of a cold collection names no summary: {plan}"
    );
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "the collection itself ran: {:?}",
        t.h.stderr
    );
    let window = next_window(&mut t, "t5");
    assert!(
        stands(&window, YESTERDAY),
        "the day before stays in its own words: {window:?}"
    );
}
