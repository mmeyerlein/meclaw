//! d58-3 nachlese (GH #1539, OR-583-84, changes OR-WG-6) -- the tool round and
//! the answer round share one prefix.
//!
//! WHY this file exists: since GH #1260 (OR-WG-6) the required tool call of a
//! `simple` turn dropped the sidecar contract and `instructions.quick`, and the
//! answer call of the same round set them again -- the system tree changed
//! inside the round, so every quick tool round broke the cached prefix. The
//! ruling OR-583-84: the required call keeps the round's system tree; the call
//! stays required through `tool_scope.require` (tool_choice required). The lab
//! probe (`plans/d58-3-2026-11-14/lab/nachlese-sonde/ERGEBNIS.md`) called 10 of
//! 10 with the contract standing.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn seam(h: &mut Hive, iter: u32, messages: Value) {
    h.out.clear();
    let it = iter.to_string();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": it, "phase": "",
                     "path": "simple", "families": "web", "needs": "tool"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": it,
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let body = json!({"messages": messages,
                      "system": {"instructions": {
                          "mode": {"text": "Be brief."},
                          "sidecar": {"text": "After your answer append ONE sidecar block."}}}});
    h.lane("in_curate", ctx, hop, body);
}

/// The one call of the round that is not the bridge beside it.
fn main_call(h: &Hive) -> Msg {
    h.routed("quick")
        .into_iter()
        .find(|m| m.hop.get("call_kind").is_none())
        .unwrap_or_else(|| panic!("no main quick call: {:?} {:?}", h.out, h.stderr))
}

#[test]
fn the_tool_round_and_the_answer_round_share_one_prefix() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let ask = user("how is the weather in Potsdam today?");

    // The tool round: the call is required.
    seam(&mut h, 0, json!([ask.clone()]));
    let tool = main_call(&h);
    assert_eq!(
        tool.body["tool_scope"]["require"],
        json!(true),
        "the tool round still requires the call: {:?}",
        tool.body["tool_scope"]
    );
    // OR-WG-6 holds for the bridge beside the required call (nachlese-h,
    // GH #1539): only the required call keeps the round's tree -- the bridge
    // says its own sentence and carries neither the contract nor
    // `instructions.quick` ("When you call a tool, ..."), which would
    // contradict its own slot ("write no tool call").
    let bridge = h
        .routed("quick")
        .into_iter()
        .find(|m| m.hop["call_kind"] == json!("side"))
        .unwrap_or_else(|| {
            panic!(
                "no bridge beside the tool round: {:?} {:?}",
                h.out, h.stderr
            )
        });
    let b_ins = &bridge.body["system"]["instructions"];
    assert!(
        b_ins.get("sidecar").is_none(),
        "the bridge carries no contract: {b_ins:?}"
    );
    assert!(
        b_ins.get("quick").is_none(),
        "the bridge carries no `instructions.quick`: {b_ins:?}"
    );

    // The answer round of the same turn: the tool result is there.
    seam(
        &mut h,
        1,
        json!([
            ask,
            tool_call("c1", "weather"),
            tool_result("c1", "Potsdam: sunny, 21 degrees.")
        ]),
    );
    let answer = main_call(&h);
    assert!(answer.body["tool_scope"].get("require").is_none());

    // One system tree for the round: the contract and `instructions.quick`
    // stand in both calls (OR-583-84).
    for (name, m) in [("tool", &tool), ("answer", &answer)] {
        let ins = &m.body["system"]["instructions"];
        assert!(
            ins.get("sidecar").is_some(),
            "the {name} call carries the sidecar contract: {ins:?}"
        );
        assert!(
            ins["quick"]["text"].as_str().is_some_and(|t| !t.is_empty()),
            "the {name} call carries `instructions.quick`: {ins:?}"
        );
    }
    assert_eq!(
        tool.body["system"], answer.body["system"],
        "the tool round and the answer round share one system tree"
    );

    // The bytes: the answer call begins with the bytes of the tool call --
    // the same system, then the tool call's messages, then the round's new ones.
    let sys_t = meclaw_core::serde_json::to_string(&tool.body["system"]).unwrap();
    let sys_a = meclaw_core::serde_json::to_string(&answer.body["system"]).unwrap();
    assert_eq!(sys_t, sys_a);
    let msgs_t = meclaw_core::serde_json::to_string(&tool.body["messages"]).unwrap();
    let msgs_a = meclaw_core::serde_json::to_string(&answer.body["messages"]).unwrap();
    let open = msgs_t.trim_end_matches(']');
    assert!(
        msgs_a.starts_with(open),
        "the answer call begins with the bytes of the tool call:\n tool:   {msgs_t}\n answer: {msgs_a}"
    );
}
