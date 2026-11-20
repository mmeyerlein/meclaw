//! GH #1573 -- a held `contacts` slot reaches the window of a target whose
//! system part goes whole, and a target that keeps the diff gets it as a
//! changed family; without the slot the window is the one of before.
//!
//! WHY this file exists: on a member's own turn the fast voice was offered a
//! tool that addresses a contact, and no window showed the contacts (d54g
//! diagnosis of rollout R046, round 1: `roster` empty, no contact id in the
//! request). The collector now writes the family `contacts` out of the menu
//! answers (`in_slots`, like `tools` and `instructions.sidecar`); the
//! curator learns no new name for it -- a durable family goes where the
//! profile's `system` sends the durable families. This file pins that the
//! family really arrives, so a later family filter cannot drop it unseen.
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

const LIST: &str = "Contacts (address a contact by its id):\nk-1 = The workshop";

/// The system part of the last call of a conversation on a target with
/// `profile`, the `contacts` slot held (`with`) or not.
fn system_seen(profile: Value, with: bool) -> Value {
    let mut h = Hive::with(&[]);
    announce_profile(&mut h, "seat", profile);
    for (i, (words, reply)) in TALK.iter().take(2).enumerate() {
        turn_for(&mut h, "seat", &format!("t{i}"), words, reply);
    }
    // The slot lands between two calls, as a menu answer does: a delta
    // target has sent every other family by now, so this one is the change.
    if with {
        h.lane(
            "in_slots",
            json!({}),
            json!({}),
            json!({"system": {"contacts": {"text": LIST}}}),
        );
    }
    let calls = curate_for(
        &mut h,
        Some("seat"),
        "t9",
        0,
        json!([user("Ask the workshop whether the car is ready.")]),
        json!({"path": "", "needs": "", "families": ""}),
    );
    let c = the_call(&h, &calls);
    c.body.get("system").cloned().unwrap_or(Value::Null)
}

#[test]
fn gh1573_a_held_contacts_slot_reaches_the_window() {
    if !shipped() {
        return;
    }
    let full = json!({"system": "full", "recall": false});
    let sys = system_seen(full.clone(), true);
    assert_eq!(
        sys["contacts"]["text"],
        json!(LIST),
        "a whole-tree window carries the held contacts family: {sys:#?}"
    );
    let sys = system_seen(full, false);
    assert!(
        sys.get("contacts").is_none(),
        "no slot, no family: the window of before: {sys:#?}"
    );
    // A delta target gets the family on the call after it changed.
    let sys = system_seen(json!({"system": "delta"}), true);
    assert!(
        sys["contacts"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("k-1 = The workshop")),
        "a delta window gets the family as a changed one: {sys:#?}"
    );
}
