//! GH #1462 (D58 snap, R-38-4 Nr. 5) -- the snapshot holds the window as sent.
//!
//! WHY this file exists: the snapshot row of a call lists its plan (GH #1448)
//! and its cut (GH #1450) by block id -- what the call was MEANT to hold.
//! "Was the context there?" asks what the model was SENT: every element in
//! the order and the form it left in. With `snapshot_window` "1" the row
//! carries the list `sent`, one `{hash, kind, form, bytes}` per element, and
//! every hash names a block of the ledger -- for the call of a turn and for
//! the bridge, a call of its own with a row of its own.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::announce_profile;
use meclaw_core::serde_json::{Value, json};

fn entry_of<'a>(sent: &'a [Value], el: &Value) -> Option<&'a Value> {
    let hash = sha256_hex(&canonical(el));
    sent.iter().find(|e| e["hash"] == json!(hash))
}

#[test]
fn every_element_of_the_call_is_listed_in_the_order_sent() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    turn(
        &mut h,
        "s1",
        "t1",
        "first question",
        "first answer",
        json!({}),
    );
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("second question")]),
        mode("Be brief."),
    );
    // The window this test speaks of, before the list is read.
    let msgs = call.messages();
    assert_eq!(
        msgs,
        vec![
            user("first question"),
            said("first answer"),
            user("second question")
        ],
        "the call sends the wall as it is"
    );
    let sent = sent_of(&h, &call).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(
        w.messages, msgs,
        "the messages of `sent` are the messages sent, in their order"
    );
    for el in &msgs {
        let e = entry_of(&sent, el).unwrap_or_else(|| panic!("{el} is listed: {sent:?}"));
        assert_eq!(e["form"], json!("raw"), "sent as the wall holds it: {e}");
    }
    assert_eq!(
        entry_of(&sent, &user("second question")).map(|e| e["kind"].clone()),
        Some(json!("words")),
        "the person's words are `words`"
    );
    assert_eq!(
        entry_of(&sent, &said("first answer")).map(|e| e["kind"].clone()),
        Some(json!("answer")),
        "the model's answer is `answer`"
    );
    assert!(
        w.leaves
            .iter()
            .any(|(p, t)| p == "instructions.mode" && t == "Be brief."),
        "the system part is listed leaf by leaf: {:?}",
        w.leaves
    );
    let leaf = json!({"path": "instructions.mode", "text": "Be brief."});
    assert_eq!(
        entry_of(&sent, &leaf).map(|e| e["kind"].clone()),
        Some(json!("frame")),
        "a leaf of the system part is `frame`"
    );
    assert!(
        w.pieces.is_empty(),
        "nothing made in the cut here: {:?}",
        w.pieces
    );
    // The system part stands before the messages, as it is sent.
    let first_msg = sent
        .iter()
        .position(|e| e["hash"] == json!(sha256_hex(&canonical(&msgs[0]))))
        .expect("listed");
    let leaf_at = sent
        .iter()
        .position(|e| e["hash"] == json!(sha256_hex(&canonical(&leaf))))
        .expect("listed");
    assert!(leaf_at < first_msg, "system first: {sent:?}");
}

#[test]
fn the_bridge_holds_its_own_window_and_the_frame_line_is_built() {
    if !shipped() {
        return;
    }
    // The bridge goes to a target whose profile asks for the moment: the
    // leaf `frame` is the target's word, the place the instance's (D58 frame,
    // GH #1478, OR-d58-2.frame.2) -- a bridge to a target without a profile
    // gets none.
    let mut h = Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("policy", "place", json!("Lisbon")),
        ("lead", "bridge_target", json!("*:aside")),
    ]);
    announce_profile(&mut h, "aside", json!({"frame": true}));
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "complex", "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "complex"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("plan my trip to Rome")], "system": mode("Be brief.")}),
    );
    let (call, bridge) = calls_out(&h);
    let bridge = bridge.unwrap_or_else(|| panic!("a bridge: {:?} {:?}", h.out, h.stderr));
    // The frame line the bridge was sent, before the list is read.
    let frame = system_leaves(&Value::Object(bridge.body.clone())["system"])
        .into_iter()
        .find(|(p, _)| p == "frame")
        .map(|(_, t)| t)
        .unwrap_or_else(|| panic!("the bridge carries `frame`: {:?}", bridge.body));
    assert!(frame.contains("place: Lisbon"), "{frame}");

    let sent = sent_of(&h, &bridge).expect("the snapshot row of the bridge carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(w.messages, bridge.messages(), "the bridge's own messages");
    let e = entry_of(&sent, &json!({"path": "frame", "text": frame}))
        .unwrap_or_else(|| panic!("the frame line is listed, its text a block: {sent:?}"));
    assert_eq!(e["kind"], json!("frame"), "{e}");
    assert_eq!(
        e["form"],
        json!("built"),
        "made in the cut, held by no slot: {e}"
    );
    let own = sent_of(&h, &call).expect("the call beside the bridge carries its own `sent`");
    assert_ne!(own, sent, "two calls, two windows");
    assert_eq!(sent_window(&h, &own).messages, call.messages());
}
