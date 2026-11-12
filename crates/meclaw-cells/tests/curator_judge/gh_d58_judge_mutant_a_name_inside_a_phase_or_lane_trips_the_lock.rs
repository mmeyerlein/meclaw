//! D58 judge (GH #1480, contract point 9, review I2) -- mutant: a locked
//! name inside the name of a phase or a lane trips the lock.
//!
//! WHY this file exists: review 1 of the strand found the phase
//! `judge-write` in new curator code (`./history`, the writes of `in_part`,
//! `in_seed_line` and the seed). The lock read whole literals only, so the
//! role name `judge` passed as a part of a phase name; contract point 9 means
//! the name, not only the whole literal. The reader now reads a literal in
//! the form of a phase (`view-rows`) or of a lane (`in_view_ask`) by its
//! words as well. Planted here: a name as a word of a phase, of a lane, of a
//! lane in an edge's modifier and of a lane in the contract trips it; the
//! same phase in a comment or a docstring does not, nor a param or key that
//! only starts with a name (`judge_context`, `hints_of`), nor a phase or lane
//! without a locked word.
//!
//! Runs the lock's reader under python3 (`support/curator_target.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::json;

fn src(code: &str) -> Vec<(String, String)> {
    vec![("planted.py".to_string(), code.to_string())]
}

#[test]
fn gh_d58_judge_mutant_a_name_inside_a_phase_or_lane_trips_the_lock() {
    let none = json!([]);
    let empty = json!({});
    let tripped = [
        "if phase == \"judge-write\":\n    pass\n",
        "emit(lstore(w, \"seed-screen-rows\", \"x\", \"\"))\n",
        "if lane == \"in_screen_topics\":\n    pass\n",
        "P = \"hints-read\"\n",
    ];
    for code in tripped {
        let found = lock_findings(&LOCKED_NAMES, &src(code), &none, &empty);
        assert_eq!(found.len(), 1, "planted {code:?} trips the lock: {found:?}");
    }
    let quiet = [
        "# the judge-write phase\nX = 1\n",
        "def f():\n    \"\"\"judge-write\"\"\"\n    return 1\n",
        "K = \"judge_context\"\nH = \"hints_of\"\n",
        "P = \"state-write\"\nL = \"in_view_ask\"\nQ = \"view-rows\"\n",
        "S = \"the judge-write phase\"\n",
    ];
    for code in quiet {
        let found = lock_findings(&LOCKED_NAMES, &src(code), &none, &empty);
        assert!(found.is_empty(), "planted {code:?} is no name: {found:?}");
    }
    let edges = json!([
        {"from": ".", "to": "./history", "modifier": {"set_hop": {"route": "in_judge_ask"}}},
        {"from": ".", "to": "./history", "modifier": {"set_hop": {"route": "in_view_ask"}}}
    ]);
    let found = lock_findings(&LOCKED_NAMES, &[], &edges, &empty);
    assert_eq!(
        found.len(),
        1,
        "a name inside a lane of a modifier: {found:?}"
    );
    let contract = json!({
        "accepts": [{"route": "in_screen_topics", "because": "the topics"},
                    {"route": "in_part", "because": "what the judge reads"}],
        "emits": [{"route": "view", "because": "a view"}]
    });
    let found = lock_findings(&LOCKED_NAMES, &[], &none, &contract);
    assert_eq!(
        found.len(),
        1,
        "a name inside a lane in the contract: {found:?}"
    );
    // The shipped history with one planted phase: the lock sees it.
    if !shipped() {
        return;
    }
    let planted = format!("{}\n_LOCK_PROBE = \"judge-write\"\n", script_of("history"));
    let before = lock_findings(&LOCKED_NAMES, &src(&script_of("history")), &none, &empty).len();
    let after = lock_findings(&LOCKED_NAMES, &src(&planted), &none, &empty).len();
    assert_eq!(after, before + 1, "one planted phase, one more finding");
}
