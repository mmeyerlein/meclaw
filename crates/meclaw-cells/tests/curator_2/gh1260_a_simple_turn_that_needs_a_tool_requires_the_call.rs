//! GH #1260 (OR-WG-6) -- a simple turn that needs a tool requires the call.
//!
//! WHY this file exists: on `simple` the fast model called its tool in 0 of 5
//! lab turns. It wrote "let me check" plus its sidecar block and stopped, or
//! made the weather and the opening hours up. A probe of the same window
//! (lab probe for GH #1260): 7 of 10 calls with the contract, 8 of 10 without, 10
//! of 10 only with the call required -- and then without the sentence beside
//! it. A model alone does not guarantee it, so the way does when the stamp
//! asks for it: in the TOOL ROUND of a `simple` turn whose stamp `needs` a
//! `tool` (no tool result in the round yet) the quick call carries
//! `tool_scope.require` and no sidecar contract (the answer comes in the next
//! round), and beside it the bridge -- the quick model, no tool, no contract
//! -- says one short sentence so phone and screen are not silent. The bridge
//! is `interim` (`hop.interim`, lifted into the context by talky's edge): a
//! final answer would end the round before the tool result came back. The
//! FORMULATION round (a tool result is there) runs as before: no require,
//! the contract, the speaker rule. Without `needs: tool` nothing changes.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn seam(h: &mut Hive, turn_id: &str, iter: u32, needs: &str, channel: Value, messages: Value) {
    h.out.clear();
    let it = iter.to_string();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": it, "phase": "",
                     "path": "simple", "families": "web", "needs": needs});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": it,
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    let mut body = json!({"messages": messages,
                          "system": {"instructions": {
                              "mode": {"text": "Be brief."},
                              "sidecar": {"text": "After your answer append ONE sidecar block."}}}});
    if !channel.is_null() {
        body["tool_scope"] = channel;
    }
    h.lane("in_curate", ctx, hop, body);
}

fn contract_of(m: &Msg) -> Option<Value> {
    m.body
        .get("system")
        .and_then(|s| s.get("instructions"))
        .and_then(|i| i.get("sidecar"))
        .cloned()
}

#[test]
fn a_simple_turn_that_needs_a_tool_requires_the_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // Every emission as the colony checks it: one outside the curator's
    // `contract.emits` is dead-lettered before it reaches `./quick`.
    h.check_emits = true;

    // The tool round: the call is required, the bridge speaks beside it.
    seam(
        &mut h,
        "t1",
        0,
        "tool",
        Value::Null,
        json!([user("how is the weather in Potsdam today?")]),
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "call and bridge: {:?} {:?}",
        h.out,
        h.stderr
    );
    // Fix round 1 (OR-WG-8, way 2): both calls go to the one `./quick`, a
    // stateful llm cell that takes its mailbox one `handle()` at a time
    // (`max_concurrency` is a stateless knob), so they cannot run side by
    // side. The bridge goes FIRST: the first sound comes after the bridge's
    // call, not after the required call plus the bridge's.
    assert!(
        quick[0].hop.contains_key("bridge") && !quick[1].hop.contains_key("bridge"),
        "the bridge leaves before the required call: {:?}",
        quick.iter().map(|m| &m.hop).collect::<Vec<_>>()
    );
    let call = &quick[1];
    let bridge = &quick[0];
    assert_eq!(
        call.body["tool_scope"],
        json!({"allow": ["web", "weather"], "hint": [], "require": true}),
        "the tool round requires the call"
    );
    assert!(
        contract_of(call).is_none(),
        "the answer comes in the next round, so no contract now: {:?}",
        call.body["system"]
    );
    assert_eq!(bridge.hop["bridge"], json!("tool"));
    assert_eq!(
        bridge.hop["interim"],
        json!("1"),
        "the bridge does not end the round"
    );
    assert_eq!(bridge.body["tool_scope"], json!({"allow": []}));
    assert!(contract_of(bridge).is_none(), "{:?}", bridge.body["system"]);
    assert!(
        bridge.body["system"]["instructions"]["bridge"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("no tool")),
        "the bridge is told to call nothing: {:?}",
        bridge.body["system"]
    );
    assert_eq!(
        bridge.hop["curator_call"],
        json!(format!(
            "{}-bridge",
            call.hop["curator_call"].as_str().unwrap()
        )),
    );
    assert_eq!(
        h.rows("SELECT trigger FROM calls WHERE call_id LIKE '%-bridge'"),
        vec![vec![json!("bridge")]],
        "the bridge books its own call row"
    );

    // The formulation round: a result is there -- no require, the contract.
    seam(
        &mut h,
        "t1",
        1,
        "tool",
        Value::Null,
        json!([
            tool_call("c1", "weather"),
            tool_result("c1", "Potsdam: sunny, 21 degrees.")
        ]),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        quick[0].body["tool_scope"].get("require").is_none(),
        "{:?}",
        quick[0].body["tool_scope"]
    );
    assert!(
        contract_of(&quick[0]).is_some(),
        "the answer carries the contract"
    );

    // Without `needs: tool` (a short fact): unchanged.
    seam(
        &mut h,
        "t2",
        0,
        "",
        Value::Null,
        json!([user("what is two and two?")]),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "no bridge: {:?} {:?}", h.out, h.stderr);
    assert!(quick[0].body["tool_scope"].get("require").is_none());
    assert!(contract_of(&quick[0]).is_some());

    // A channel whose scope leaves the quick way no tool: nothing to require.
    seam(
        &mut h,
        "t3",
        0,
        "tool",
        json!({"allow": ["bash"]}),
        json!([user("how is the weather in Potsdam today?")]),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "no bridge: {:?} {:?}", h.out, h.stderr);
    assert!(quick[0].body["tool_scope"].get("require").is_none());
    assert_eq!(
        h.emit_violations,
        Vec::<String>::new(),
        "every emission is one the contract declares"
    );
}

/// talky lifts the bridge's `interim` into the quick call's context, so the
/// dispatcher can leave its sentence as an interim answer.
#[test]
fn talky_carries_the_interim_mark_to_the_quick_voice() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let edge = talky["params"]["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["from"] == "./curator" && e["to"] == "./quick")
        .unwrap_or_else(|| panic!("no ./curator -> ./quick edge"));
    assert_eq!(
        edge["modifier"]["set_context"]["interim"],
        json!("has(hop.interim) ? hop.interim : ''"),
        "{edge}"
    );
}

/// Fix round 1 (review M-1): the curator declares what the tool round sends
/// -- `hop.interim` beside `hop.bridge` -- so the build-time locality check
/// sees the chain that talky's edge lifts into the context.
#[test]
fn the_curator_declares_the_interim_mark() {
    if !shipped() {
        return;
    }
    let policy = read_json(&repo("templates/curator/policy/config.json"));
    let hop = &policy["contract"]["emits"]["hop"];
    assert_eq!(hop["interim"]["type"], json!("string"), "{hop}");
    assert_eq!(hop["interim"]["required"], json!(false), "{hop}");
}
