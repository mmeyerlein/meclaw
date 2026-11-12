//! GH #1464 (D58 coldgc, contract point 2) -- the cold collection never
//! touches words, answers or errors.
//!
//! WHY this file exists: the collection runs while nobody talks and without
//! a model, so nothing may leave the window that a person would miss. It
//! takes what the classes of the garbage collection name and nothing else:
//! every word of the person and every answer stand byte for byte after it,
//! and a tool result that reports a failure stands whole however old it is
//! -- the model keeps seeing that a tool failed, in the words of the failure.
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

/// The failure of a tool as the tool road reports it: an error object.
const FAILURE: &str =
    r#"{"error": {"code": "not_found", "message": "no such file: plan-cold.txt"}}"#;

#[test]
fn gh_d58_cold_the_collection_never_touches_words_answers_errors() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&cold_knobs(Some("1"))).reporting_composite("talky");
    t.turn("t1", WORDS[0], ANSWERS[0]);
    // The oldest tool round fails; two more follow, so it is older than
    // `keep_rounds` and would be collected if it were no error.
    let id = "call_tool_cold_fail";
    let c0 = t.call("t2", 0, json!([user("Read the plan, please.")]));
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    let failed = tool_result(id, FAILURE);
    let c1 = t.call("t2", 1, json!([tool_call(id, "read_file"), failed.clone()]));
    t.answer(&c1, "stop", json!([said("I could not find that file.")]));
    let old = tool_turn(&mut t, "t3", 1, WORDS[1], ANSWERS[1]);
    tool_turn(&mut t, "t4", 2, WORDS[2], ANSWERS[2]);
    t.turn("t5", WORDS[3], ANSWERS[3]);

    cache_expires(&mut t);
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "check of the setup: the collection ran and took the old result, never the failure: {:?}",
        t.h.stderr
    );
    assert!(
        !shrunk(&t).contains(&short_id(&failed)),
        "a failed result is never collected"
    );

    let window = next_window(&mut t, "t6");
    for line in WORDS
        .iter()
        .chain(ANSWERS.iter())
        .chain(["Read the plan, please.", "I could not find that file."].iter())
    {
        assert!(
            stands(&window, line),
            "`{line}` stands byte for byte after the collection: {window:?}"
        );
    }
    assert!(
        window
            .iter()
            .any(|w| w.contains("no such file: plan-cold.txt")),
        "the failure stands in its own words: {window:?}"
    );
}
