//! GH #1260 -- a simple turn carries the tools of its class, at the ops stage.
//!
//! WHY this file exists: the fast model answers `simple` turns, and those are
//! the turns a search or the weather answers. Until this strand its
//! `tool_scope` allowed only the stamp's families and named every one of them
//! in `hint`: the hint stage shows a family by name, first sentence and the
//! schema `{op}`, and the dispatcher unfolds only `{op, args}` -- so the fast
//! model could not search at all (a lab turn asked for the news and made no
//! tool call), and a stamp without a family left it no tool. The curator owns
//! what the model sees (R-WM-1): the param `quick_tools` (default
//! `web,weather`) names the class's tools, allowed on every quick call at the
//! ops stage with their whole schema; a stamped family outside the class joins
//! the allow and stays at the hint stage; the channel's own allow still cuts.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn quick_scope_of(h: &mut Hive, turn_id: &str, families: &str, channel: Value) -> Value {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": "simple", "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let mut body = json!({"messages": [user("what is the weather in Berlin?")],
                          "system": mode("Be brief.")});
    if !channel.is_null() {
        body["tool_scope"] = channel;
    }
    h.lane("in_curate", ctx, hop, body);
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "one call on quick: {:?} {:?}",
        h.out,
        h.stderr
    );
    quick[0].body["tool_scope"].clone()
}

#[test]
fn a_simple_turn_carries_its_class_tools_at_ops() {
    if !shipped() {
        return;
    }
    // OR-d58-2.target.62: the knob `quick_tools` left the curator; the
    // class's tools are the list `allow` of the fast cell's profile.
    let policy = cell_config("policy");
    assert!(policy["params"].get("quick_tools").is_none());
    assert!(policy["contract"]["settings"].get("quick_tools").is_none());
    let quick = read_json(&repo("templates/talky/quick/config.json"));
    assert_eq!(
        quick["params"]["window_profile"]["allow"],
        json!(["web", "weather"])
    );

    let mut h = Hive::with(&[]).reporting_composite("talky");
    assert_eq!(
        quick_scope_of(&mut h, "t1", "", Value::Null),
        json!({"allow": ["web", "weather"], "hint": []}),
        "without a family the class's tools stand, whole"
    );
    assert_eq!(
        quick_scope_of(&mut h, "t2", "web", Value::Null),
        json!({"allow": ["web", "weather"], "hint": []}),
        "a stamped family of the class stays at the ops stage"
    );
    assert_eq!(
        quick_scope_of(&mut h, "t3", "history", Value::Null),
        json!({"allow": ["web", "weather", "history"], "hint": ["history"]}),
        "a family outside the class joins at the hint stage"
    );
    assert_eq!(
        quick_scope_of(
            &mut h,
            "t4",
            "history",
            json!({"allow": ["web", "history"], "deny": ["bash"]})
        ),
        json!({"allow": ["web", "history"], "deny": ["bash"], "hint": ["history"]}),
        "the channel's allow cuts the class (no weather) and its deny stays"
    );

    // OR-d58-2.target.62: the empty class is an empty `allow` on the fast
    // cells (the knob `quick_tools` "" of before).
    let empty = json!({"allow": []});
    let mut none = Hive::with(&[]).reporting_composite_with(
        "talky",
        &[
            ("quick", empty.clone()),
            ("quick_spec", empty.clone()),
            ("quick_bridge", empty),
        ],
    );
    assert_eq!(
        quick_scope_of(&mut none, "t5", "web", Value::Null),
        json!({"allow": ["web"], "hint": ["web"]}),
        "an empty class keeps the old form: the stamp's families at the hint stage"
    );
}
