//! D58 target (GH #1483, contract point 8) -- the window leaves under its
//! target.
//!
//! WHY this file exists: a call left the curator on the route of its way
//! (`brain`, `quick`, `consult`, `research`), and the composite told its
//! cells apart by that route plus `hop.spec` and an ending of the call id;
//! a fifth model cell needed a new way in the policy, the hive's edge and
//! its contract. Now every call to a model leaves on ONE route, the same
//! for every target, and carries the target's name in `hop.target`; the
//! composite delivers by that name. The hive's way out and its contract
//! name no way any more.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const FORMER_WAYS: [&str; 4] = ["brain", "quick", "consult", "research"];

#[test]
fn gh_d58_target_the_window_leaves_under_its_target() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "north", json!({}));
    announce_profile(&mut h, "south", json!({}));
    let a = turn_for(&mut h, "north", "t1", TALK[0].0, TALK[0].1);
    let b = turn_for(&mut h, "south", "t2", TALK[1].0, TALK[1].1);
    assert_eq!(a.hop.get("target"), Some(&json!("north")), "{:?}", a.hop);
    assert_eq!(b.hop.get("target"), Some(&json!("south")), "{:?}", b.hop);
    assert_eq!(
        a.route(),
        b.route(),
        "one route for every call to a model, whatever the target"
    );
    assert!(
        !FORMER_WAYS.contains(&a.route()),
        "the route names no way: {}",
        a.route()
    );

    let hive = read_json(&repo("templates/curator/config.json"));
    let emits: Vec<Value> = hive["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e["route"].clone())
        .collect();
    assert!(
        emits.contains(&json!(a.route())),
        "the hive's contract declares the one route: {emits:?}"
    );
    for w in FORMER_WAYS {
        assert!(
            !emits.contains(&json!(w)),
            "the hive's contract names the way {w}: {emits:?}"
        );
    }
    let out_of_policy: Vec<Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["from"] == "./policy" && e["to"] == ".")
        .collect();
    let to_model: Vec<&Value> = out_of_policy
        .iter()
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains(&format!("'{}'", a.route())))
        })
        .collect();
    assert_eq!(
        to_model.len(),
        1,
        "one edge leads the calls out of the hive: {out_of_policy:?}"
    );
    let cond = to_model[0]["condition"].as_str().unwrap_or_default();
    for w in FORMER_WAYS {
        assert!(
            !cond.contains(&format!("'{w}'")),
            "the way out names the way {w}: {cond}"
        );
    }
}
