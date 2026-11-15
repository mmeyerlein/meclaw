//! GH #1462 (D58 snap, R-38-4 Nr. 5) -- a snapshot rebuilds the sent bytes.
//!
//! WHY this file exists: this is the acceptance of the `sent` list. From the
//! snapshot row and the blocks of the ledger alone -- no plan, no wall, no
//! code of the cut -- the window is put together again and compared with the
//! message the model cell got: the system part leaf by leaf, the messages
//! byte for byte in their order, the tool menu by its names. A list that
//! cannot do that is a description, not a record.
//!
//! A later call carries only the families of the system part that moved; the
//! model cell holds the rest. The list names the whole tree and marks each
//! leaf `carried` or not, and the leaves marked `carried` are exactly the
//! system part of the message to the model cell. That the whole list is the
//! system part the provider gets is not pinned here: this harness has no
//! model cell and so no request to a provider.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn hive() -> Hive {
    Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("policy", "state_block", json!("1")),
        ("policy", "state_lines", json!(8)),
        ("policy", "notes_share", json!(0.02)),
    ])
}

/// The menu part of the call's `tool_scope` as the snapshot lists it
/// (GH #1497, review I2 of the prefix strand: the watch judges the menu, so
/// `deny` and `hint` belong to it): `allow` when the scope names one, `deny`
/// and `hint` when not empty; None without a scope.
fn tools_of(call: &Msg) -> Option<Value> {
    let scope = call.body.get("tool_scope")?.as_object()?;
    let mut menu = meclaw_core::serde_json::Map::new();
    if let Some(a) = scope.get("allow").filter(|a| a.is_array()) {
        menu.insert("allow".into(), a.clone());
    }
    for k in ["deny", "hint"] {
        if let Some(v) = scope
            .get(k)
            .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        {
            menu.insert(k.into(), v.clone());
        }
    }
    Some(Value::Object(menu))
}

#[test]
fn the_first_call_of_a_hive_is_rebuilt_whole() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let call = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    let system = system_leaves(&Value::Object(call.body.clone())["system"]);
    assert!(!system.is_empty(), "a first call sends its system part");
    // The record of before agrees with the call: the fixture stands.
    assert_eq!(
        listed_window(&h, &call),
        (system.clone(), call.messages()),
        "call_blocks names what the call carried"
    );
    let sent = sent_of(&h, &call).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(w.leaves, system, "the system part, leaf by leaf");
    assert_eq!(
        w.carried, w.leaves,
        "a first call carries every leaf the model holds"
    );
    assert_eq!(
        canonical(&json!(w.messages)),
        canonical(&json!(call.messages())),
        "the messages, byte for byte"
    );
    assert_eq!(w.tools, tools_of(&call), "the tool menu by its names");
}

#[test]
fn a_call_with_a_notes_block_and_a_call_with_its_bridge_are_rebuilt() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    // A family of the system part that stands still from call to call: the
    // model cell holds it after the first call, a later call does not carry it.
    let system = || {
        json!({"instructions": {"mode": {"text": "Be brief."}},
               "house": {"rules": {"text": "Shoes off at the door."}}})
    };
    let first = h.curate(
        "s1",
        "t1",
        0,
        json!([user("We planted tomatoes.")]),
        system(),
    );
    h.tap(&first, "stop", json!({}), json!([said("Lovely.")]));
    h.mark(
        backdated(0),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "the garden"})),
    );
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E,
                     "turn_notes": [{"id": "n1", "source": "app", "text": "The person is outside."}]});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("And now?")], "system": system(),
               "tool_scope": {"allow": ["web", "orga"]}}),
    );
    let (noted, _) = calls_out(&h);
    assert_eq!(
        tools_of(&noted),
        Some(json!({"allow": ["web", "orga"]})),
        "this call sends a tool menu with names: {:?}",
        noted.body
    );
    assert!(
        texts(&noted).iter().any(|t| t.contains("[state -- ")
            && t.contains("The person is outside.")
            && t.ends_with("And now?")),
        "the notes block opens the person's newest words: {:?}",
        texts(&noted)
    );
    h.tap(&noted, "stop", json!({}), json!([said("Water them.")]));
    // A call that goes out with a bridge beside it.
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t3", "iter": "0", "phase": "",
                     "path": "complex", "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t3", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "complex"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("plan my trip to Rome")], "system": mode("Be brief.")}),
    );
    let (call, bridge) = calls_out(&h);
    let bridge = bridge.unwrap_or_else(|| panic!("a bridge: {:?} {:?}", h.out, h.stderr));
    for (name, c) in [
        ("call with notes", &noted),
        ("call", &call),
        ("bridge", &bridge),
    ] {
        let sent = sent_of(&h, c)
            .unwrap_or_else(|| panic!("the snapshot row of the {name} carries `sent`"));
        let w = sent_window(&h, &sent);
        assert_eq!(
            canonical(&json!(w.messages)),
            canonical(&json!(c.messages())),
            "the messages of the {name}, byte for byte"
        );
        // A later call carries the families that moved: the leaves the list
        // marks `carried` are the system part of the message, leaf by leaf.
        assert_eq!(
            w.carried,
            system_leaves(&Value::Object(c.body.clone())["system"]),
            "the system part the {name} carried, leaf by leaf"
        );
        assert!(
            w.carried.iter().all(|l| w.leaves.contains(l)),
            "a carried leaf is a leaf of the tree: {:?}",
            w.leaves
        );
        assert_eq!(
            w.tools,
            tools_of(c),
            "the tool menu of the {name}, as its names and their hash"
        );
    }
    // The later call left families where they stood: the list names the
    // whole tree the model holds, and marks the leaves it did not carry.
    let sent = sent_of(&h, &noted).expect("as above");
    let w = sent_window(&h, &sent);
    let still = (
        "house.rules".to_string(),
        "Shoes off at the door.".to_string(),
    );
    assert!(
        w.leaves.contains(&still) && !w.carried.contains(&still),
        "a later call lists the leaf the model holds and did not carry it: {:?} of {:?}",
        w.carried,
        w.leaves
    );
    // The bridge sends its whole tree on every call.
    let sent = sent_of(&h, &bridge).expect("as above");
    let w = sent_window(&h, &sent);
    assert_eq!(
        w.leaves,
        system_leaves(&Value::Object(bridge.body.clone())["system"]),
        "the bridge's system part, leaf by leaf"
    );
    assert_eq!(w.carried, w.leaves, "a bridge carries every leaf");
}
