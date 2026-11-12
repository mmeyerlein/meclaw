//! D58 target (GH #1483, contract point 14) -- mutant: a cell name in a
//! curator script trips the lock.
//!
//! WHY this file exists: a lock that finds nothing proves nothing until it
//! is shown to find what it must. The reader of
//! `gh_d58_target_the_curator_names_no_cell_of_a_composition` is run here on
//! planted sources: a locked name as a whole literal of the code, as a
//! quoted value in an edge's condition, as a value of a modifier and as a
//! value in the contract trips it; the same name in a comment, in a
//! docstring, in a bare string statement and inside a longer literal
//! (`consult_id`) does not.
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
fn gh_d58_target_mutant_a_cell_name_in_a_curator_script_trips_the_lock() {
    let none = json!([]);
    let empty = json!({});
    let tripped = [
        "WAY = \"quick\"\n",
        "def f(x):\n    return x == 'brain'\n",
        "D = {\"research\": 1}\n",
        "print(f\"{'cogny'}\")\n",
        // D58 judge (GH #1480): the names of that strand.
        "K = \"judge\"\n",
        "def f(x):\n    return x.get('screen')\n",
        "KINDS = (\"wind_down\",)\n",
        "D = {\"hints\": []}\n",
    ];
    for code in tripped {
        let found = lock_findings(&LOCKED_NAMES, &src(code), &none, &empty);
        assert_eq!(found.len(), 1, "planted {code:?} trips the lock: {found:?}");
    }
    let quiet = [
        "# the quick voice speaks first\nX = 1\n",
        "def f():\n    \"\"\"quick\"\"\"\n    return 1\n",
        "\"brain\"\nX = 2\n",
        "KEY = \"consult_id\"\nY = \"quick_tools\"\n",
        "K = \"judge_context\"\nH = \"hints_of\"\n",
    ];
    for code in quiet {
        let found = lock_findings(&LOCKED_NAMES, &src(code), &none, &empty);
        assert!(found.is_empty(), "planted {code:?} is no name: {found:?}");
    }
    let edges = json!([
        {"from": "./policy", "to": ".", "condition": "has(hop.route) && hop.route == 'brain'"},
        {"from": "./a", "to": "./b", "modifier": {"set_hop": {"route": "strong"}}},
        {"from": "./a", "to": "./c", "condition": "hop.consult_id != ''"}
    ]);
    let found = lock_findings(&LOCKED_NAMES, &[], &edges, &empty);
    assert_eq!(
        found.len(),
        2,
        "a name in a condition and in a modifier: {found:?}"
    );
    let contract = json!({"emits": {"hop": {"route": {"values": ["model", "talky"]}}}});
    let found = lock_findings(&LOCKED_NAMES, &[], &none, &contract);
    assert_eq!(found.len(), 1, "a name in the contract: {found:?}");
    // The shipped policy with one planted literal: the lock sees it.
    if !shipped() {
        return;
    }
    let planted = format!("{}\n_LOCK_PROBE = \"quick_bridge\"\n", script_of("policy"));
    let before = lock_findings(&LOCKED_NAMES, &src(&script_of("policy")), &none, &empty).len();
    let after = lock_findings(&LOCKED_NAMES, &src(&planted), &none, &empty).len();
    assert_eq!(after, before + 1, "one planted name, one more finding");
}
