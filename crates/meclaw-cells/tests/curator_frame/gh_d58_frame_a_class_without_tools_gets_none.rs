//! D58 frame (GH #1478, note per stamp class point 3) -- a class without
//! tools gets none, at every target.
//!
//! WHY this file exists: a turn that only asks back opens no window and
//! writes nothing, so it gets no tool (GH #1279). That held for the one
//! fixed class `clarify` and only on the way the lead sent it; now it is the
//! entry's field `tools: "none"`, for any class and at any target -- one
//! that holds its tree and one with a scoped list alike. The channel's
//! `deny` stays on the empty menu. The frozen system part of a turn serves
//! the tool rounds of that turn; a turn without a tool has none, so a target
//! that holds its tree writes no such row for it (plan-review-schluss R-H1).
//! A class whose entry names no `tools` keeps its target's scope and its
//! row -- `clarify` included.
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

/// The scope the composite brings on every curate of this test.
fn scope_in() -> Value {
    json!({"allow": ["web", "weather"], "deny": ["bash"]})
}

/// One curate of `turn` stamped `path` for `target`, the composite's scope
/// in the body; the one call.
fn curate_class(h: &mut Hive, target: &str, turn: &str, path: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "target": target, "path": path});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("Which one?")], "system": system_tree(),
               "tool_scope": scope_in()}),
    );
    let calls = model_calls(h);
    the_call(h, &calls)
}

fn turn_rows(h: &Hive, turn: &str) -> usize {
    h.rows(&format!(
        "SELECT key FROM state WHERE key LIKE 'turn_system:s1:{turn}%'"
    ))
    .len()
}

#[test]
fn gh_d58_frame_a_class_without_tools_gets_none() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[(
        "policy",
        "class_notes",
        json!({"vague": {"text": "Ask which one is meant.", "tools": "none"},
               "clarify": {"text": "Ask one question back."}}),
    )]);
    announce_profile(&mut h, "wall", json!({"system": "delta", "tools": "all"}));
    announce_profile(
        &mut h,
        "narrow",
        json!({"system": "full", "tools": "scoped", "allow": ["weather"]}),
    );
    let none = json!({"allow": [], "deny": ["bash"]});
    let mut failures = Vec::new();
    let mut n = 0;
    let mut next_turn = || {
        n += 1;
        format!("t{n}")
    };

    // `tools: "none"`: no tool at either target, the deny kept; no frozen
    // system part at the target that holds its tree.
    for target in ["wall", "narrow"] {
        let t = next_turn();
        let c = curate_class(&mut h, target, &t, "vague");
        if c.body.get("tool_scope") != Some(&none) {
            failures.push(format!(
                "{target}, vague: the scope is not empty with the deny kept: {:?} {:?}",
                c.body.get("tool_scope"),
                h.stderr
            ));
        }
        if target == "wall" && turn_rows(&h, &t) != 0 {
            failures.push("wall, vague: a frozen system part for a turn without a tool".into());
        }
        answer(
            &mut h,
            &c,
            "stop",
            warm_usage(),
            json!([said("Which one do you mean?")]),
        );
    }

    // Controls: a class without an entry and a class whose entry names no
    // `tools` (`clarify` too) keep the target's scope and the turn's row.
    for path in ["talk", "clarify"] {
        let t = next_turn();
        let c = curate_class(&mut h, "wall", &t, path);
        if c.body.get("tool_scope") != Some(&scope_in()) {
            failures.push(format!(
                "wall, {path}: the target's scope is not kept: {:?}",
                c.body.get("tool_scope")
            ));
        }
        if turn_rows(&h, &t) == 0 {
            failures.push(format!("wall, {path}: no frozen system part of the turn"));
        }
        answer(&mut h, &c, "stop", warm_usage(), json!([said("Fine.")]));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
