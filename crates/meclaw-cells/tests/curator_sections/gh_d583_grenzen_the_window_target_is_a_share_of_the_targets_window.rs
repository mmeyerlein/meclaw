//! GH #1526 (d58-3 grenzen, OR-583.grenzen.12) -- the window at which a
//! target orders a rebuild of its own is a share of its window.
//!
//! WHY this file exists: talky's targets ordered a `target` rebuild at 100000
//! tokens (`window_target`, GH #1272), the owner's guide value under luna's
//! window of 250000 -- on a model of 200000 the same number is half the
//! window, on one of 17500 it never fires. The profiles now name
//! `window_target_share` 0.4 (= 100000 / 250000), and the target of a call is
//! that share of the window its catalog row names: the call's own value
//! (`call_profile`) and the wall's (`wall_values`) alike. A `window_target`
//! set beside the share wins.
//!
//! Runs the pure half of the shipped policy (`policy_scope`): the reported
//! rows are the state `ARGS`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn profile_of(cell: &str) -> Value {
    read_json(&repo(&format!("templates/talky/{cell}/config.json")))["params"]["window_profile"]
        .clone()
}

fn rows(targets: &[(&str, u64, Value)]) -> Value {
    let mut st = meclaw_core::serde_json::Map::new();
    for (t, soft, profile) in targets {
        let row = json!({"model": format!("test/{t}"), "input_soft": soft,
                         "input_hard": soft * 4, "window_profile": profile});
        st.insert(format!("catalog:{t}"), Value::String(row.to_string()));
    }
    Value::Object(st)
}

#[test]
fn gh_d583_grenzen_the_window_target_is_a_share_of_the_targets_window() {
    if !shipped() {
        return;
    }
    let brain = profile_of("brain");
    let quick = profile_of("quick");
    for (cell, p) in [("brain", &brain), ("quick", &quick)] {
        assert_eq!(
            p["window_target_share"],
            json!(0.4),
            "talky/{cell} names its target as a share of the window: {p}"
        );
    }
    let st = rows(&[
        ("a", 250000, brain.clone()),
        ("b", 200000, brain.clone()),
        ("q", 17500, quick.clone()),
    ]);
    // The call's own target, for each of the three.
    let (calls, err) = policy_scope(
        json!({}),
        "[call_profile(t, 'in', ARGS)['window_target'] for t in ['a', 'b', 'q']]",
        st.clone(),
    );
    assert_eq!(
        calls,
        json!([100000, 80000, 7000]),
        "0.4 of 250000, 200000 and 17500: {err}"
    );
    // The wall's, read by the tap and the rebuild.
    let (walls, err) = policy_scope(
        json!({}),
        "[wall_values(profile_held(t, ARGS))[10] for t in ['a', 'b']]",
        st,
    );
    assert_eq!(walls, json!([100000, 80000]), "{err}");

    // A token value set beside the share wins.
    let mut fixed = brain.clone();
    fixed["window_target"] = json!(12000);
    let (fixed_calls, err) = policy_scope(
        json!({}),
        "[call_profile('a', 'in', ARGS)['window_target'], wall_values(profile_held('a', ARGS))[10]]",
        rows(&[("a", 250000, fixed)]),
    );
    assert_eq!(fixed_calls, json!([12000, 12000]), "{err}");
}
