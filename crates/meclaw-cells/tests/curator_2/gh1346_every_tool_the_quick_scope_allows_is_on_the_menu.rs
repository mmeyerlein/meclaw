//! GH #1346 -- every tool the quick scope allows is on the menu the model gets.
//!
//! WHY this file exists: on the quick way the curator states the call's
//! `tool_scope` itself (`quick_scope`, GH #1260): the class's tools plus the
//! router stamp's families, and the stamp names ops (`set_timer`). With the
//! fold on, the collector folds the menu into families (`set` holds
//! `set_pin` and `set_timer`), and the `llm` cell matches scope names
//! exactly against that folded menu (`tool_scope.rs`, `matches_name`). An op
//! name in `allow` matched nothing: in a stub run the router stamped the
//! timer family, the scope allowed `set_timer`, and the menu the model got
//! was `[orga, story_read, weather, web]` -- the timer was not offered, and a
//! real model on that path could not set one. The seam names the folded
//! families (`hop.tools_folded`); the curator reads its scope up through
//! them, the same way the collector's `family_scope` reads the channel's.
//! The check is general: whatever the scope allows stands on the folded
//! menu, as its family or under its own name.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The folded menu of the stub run (GH #1346), as the collector hands it on:
/// families by their family name, every other tool by its own.
const FOLDED_MENU: [&str; 8] = [
    "cancel_timer",
    "orga",
    "set",
    "story_read",
    "weather",
    "web",
    "history",
    "orga_plan",
];

fn quick_scope_of(h: &mut Hive, turn_id: &str, families: &str, channel: Value) -> Value {
    quick_scope_in(h, turn_id, families, channel, "history,set", Value::Null)
}

/// One quick call with the seam's folded families `folded` and, when `tools`
/// is an object, the menu leaves the call carries (`system.tools`).
fn quick_scope_in(
    h: &mut Hive,
    turn_id: &str,
    families: &str,
    channel: Value,
    folded: &str,
    tools: Value,
) -> Value {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": "simple", "families": families,
                     "tools_folded": folded});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let mut body = json!({"messages": [user("set a timer for the tea, five minutes")],
                          "system": mode("Be brief.")});
    if !channel.is_null() {
        body["tool_scope"] = channel;
    }
    if tools.is_object() {
        body["system"]["tools"] = tools;
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

fn names(scope: &Value, half: &str) -> Vec<String> {
    scope[half]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The check of GH #1346: every name the scope allows or hints is an entry
/// of the folded menu -- an op name the `llm` cell cannot match is a tool the
/// model never gets.
fn all_on_menu(scope: &Value) {
    for half in ["allow", "hint"] {
        for n in names(scope, half) {
            assert!(
                FOLDED_MENU.contains(&n.as_str()),
                "`{half}` names {n:?}, which is not on the folded menu {FOLDED_MENU:?}: {scope}"
            );
        }
    }
}

#[test]
fn every_tool_the_quick_scope_allows_is_on_the_menu() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        (
            "policy",
            "quick_tools",
            json!("web,weather,story_read,orga"),
        ),
    ]);

    // The stub run: the stamp names the op, the menu holds its family.
    let s = quick_scope_of(&mut h, "t1", "set_timer", Value::Null);
    all_on_menu(&s);
    assert_eq!(
        s,
        json!({"allow": ["web", "weather", "story_read", "orga", "set"], "hint": ["set"]}),
        "the stamped op stands as its family, at the hint stage"
    );

    // An op of an unfolded family and a family name stand for themselves.
    let s = quick_scope_of(&mut h, "t2", "cancel_timer,history", Value::Null);
    all_on_menu(&s);
    assert_eq!(
        names(&s, "hint"),
        vec!["cancel_timer".to_string(), "history".to_string()],
        "{s}"
    );

    // The channel's allow is stated in the folded names (the collector's
    // `family_scope`): it still cuts, and keeps a stamped op's family.
    let s = quick_scope_of(
        &mut h,
        "t3",
        "set_timer",
        json!({"allow": ["web", "set"], "deny": ["bash"]}),
    );
    all_on_menu(&s);
    assert_eq!(
        s,
        json!({"allow": ["web", "set"], "deny": ["bash"], "hint": ["set"]}),
        "the channel's allow keeps the family of the stamped op"
    );

    // Two ops of one family give the family once.
    let s = quick_scope_of(&mut h, "t4", "set_timer,set_pin", Value::Null);
    all_on_menu(&s);
    assert_eq!(names(&s, "hint"), vec!["set".to_string()], "{s}");

    // A tool on the menu under its own name stays so, even under the prefix
    // of a folded family (a turn answerer's tool is never folded, GH #1292).
    let plan = json!({"type": "function", "function": {"name": "orga_plan",
        "description": "Plan a step.", "parameters": {"type": "object", "properties": {}}}});
    let s = quick_scope_in(
        &mut h,
        "t5",
        "orga_plan",
        Value::Null,
        "orga,set",
        json!({"orga_plan": {"text": plan.to_string()}}),
    );
    all_on_menu(&s);
    assert_eq!(
        s,
        json!({"allow": ["web", "weather", "story_read", "orga", "orga_plan"], "hint": []}),
        "the tool keeps its own name; under the class's `orga` it stands at the ops stage"
    );
}
