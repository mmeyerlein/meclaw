//! GH #1464 (D58 coldgc, contract point 4 with the addendum 10.11.) -- over
//! the cold aim the summarizer runs, and the window lands there.
//!
//! WHY this file exists: under its aim an expired cache only collects; over
//! it the rebuild of before runs, with the summarizer -- and with `cold_to`
//! set it aims at the cold aim, not at the aim the order inherited. A
//! window whose collection alone stays over the cold aim is condensed down
//! to it: the summarizer is asked once, the plan carries a summary, the
//! rebuild says no `over_aim`, and the next call sends a window of about
//! the cold aim -- the long words of the oldest turns live on in the
//! summary, not raw.
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
use meclaw_core::serde_json::json;

/// The cold aim of this pin: window_target 3000 x cold_to 0.2.
const AIM: usize = 600;
/// The rate the policy counts a window with when no row names one.
const CHARS_PER_TOKEN: usize = 3;
const SUMMARY: &str = "Three stories about a garden; two files were read.";

#[test]
fn gh_d58_cold_over_the_cold_aim_the_summarizer_runs_and_lands_there() {
    if !shipped() {
        return;
    }
    let mut over = cold_knobs(Some("1"));
    // The measured prompt (2000) stays under the target, so the last turn
    // orders the cache; the cold aim is 600, far under the inherited one.
    over.push(("policy", "window_target", json!(3000)));
    over.push(("policy", "cold_to", json!("0.2")));
    over.push(("policy", "keep_recent", json!(1)));
    over.push(("policy", "keep_recent_tokens", json!(0)));
    let (mut t, _) = long_talk("talky", &over, warm_usage());
    cache_expires(&mut t);

    assert_eq!(
        t.h.summ.len(),
        1,
        "over the cold aim the summarizer is asked once: {:?}",
        t.h.stderr
    );
    t.h.answer(SUMMARY, "stop");
    let plan = t.h.plan_in(ROUND_E);
    assert!(
        !plan["summary"].as_str().unwrap_or("").is_empty(),
        "the plan carries the summary: {plan} {:?}",
        t.h.stderr
    );
    assert!(
        marks_of(&t.h, "over_aim").is_empty(),
        "the rebuild lands on its aim, it says no over_aim: {:?}",
        marks_of(&t.h, "over_aim")
    );
    assert!(
        cold_marks(&t.h).is_empty(),
        "a rebuild with the summarizer is no cold collection: {:?}",
        marks_of(&t.h, "curate")
    );

    let window = next_window(&mut t, "t5");
    assert!(
        !window.iter().any(|w| w.contains(&long_words(1))),
        "the oldest long words are condensed, not sent raw: {window:?}"
    );
    let chars: usize = window.iter().map(|w| w.chars().count()).sum();
    assert!(
        chars <= AIM * CHARS_PER_TOKEN + SUMMARY.len() + "One more thing.".len(),
        "the next call sends about the cold aim ({AIM} tokens, {} characters) beside the \
         summary and its new words: {chars} characters in {window:?}",
        AIM * CHARS_PER_TOKEN
    );
}
