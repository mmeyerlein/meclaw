//! D58 target (GH #1483, contract points 3, 4 and 9) -- a bridge leaves
//! only for the side target the verdict names.
//!
//! WHY this file exists: a required tool round (`must_call` in the profile
//! of its target) sent a bridge beside it -- a short sentence while the
//! tool runs -- whatever the verdict said. Without a side target in the
//! verdict (`side_target`, from the lead's `bridge_target`) that bridge
//! named no target: in one composite it reached the main model, in another
//! no route at all. The bridge is a side call of the composite, so the
//! composite says where it goes; a verdict that names no side target sends
//! none, and the round goes on without one -- no error, nothing parked.
//!
//! D58 moveout (GH #1476, re-check I-1): the bridge is now a side call of
//! the verdict's `side` list, and the rule holds twice. The lead names no
//! `side` entry while `bridge_target` names no target -- also when its
//! `path_routes` names a fast target, so the tool round has an entry to give
//! --, and `./policy` sends no side call for an entry without a `target`,
//! as for one without a slot (OR-d58-2.target.71).
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
use meclaw_core::serde_json::json;

/// The calls of one tool turn for the target `seat`, which must call a
/// tool, on an instance with the knobs `over`.
fn tool_turn(over: &[(&str, &str, meclaw_core::serde_json::Value)]) -> (Hive, Vec<Msg>) {
    let mut h = Hive::with(over);
    announce_profile(&mut h, "seat", json!({"must_call": true}));
    announce_profile(&mut h, "side", json!({"window": "turn"}));
    let calls = curate_for(
        &mut h,
        Some("seat"),
        "t1",
        0,
        json!([user("Will it rain on the walk? Look it up.")]),
        json!({"path": "", "needs": "tool", "families": "web"}),
    );
    (h, calls)
}

fn bridges(calls: &[Msg]) -> Vec<Msg> {
    calls
        .iter()
        .filter(|m| {
            m.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
                || m.hop.get("call_kind") == Some(&json!("side"))
        })
        .cloned()
        .collect()
}

/// The side slot `./policy` gives a `side` entry with `target` and the slot
/// `bridge` (`side_slot_of`; "" -- no side call).
fn side_slot(target: &str) -> String {
    let (got, err) = cell_scope(
        "policy",
        json!({}),
        "side_slot_of(ARGS[0], ARGS[1])",
        json!([{"when": "must_call", "target": target, "window": "turn",
                "text": "One moment.", "slot": "bridge", "first": true}, {}]),
    );
    got.as_str()
        .unwrap_or_else(|| panic!("a slot as text: {got} {err}"))
        .to_string()
}

#[test]
fn gh_d58_target_a_bridge_needs_a_side_target() {
    if !shipped() || !lead_shipped() {
        return;
    }
    // No side target in the verdict: the required round leaves alone.
    let (h, calls) = tool_turn(&[]);
    let main = the_call(&h, &calls);
    assert_eq!(main.hop["target"], json!("seat"), "{:?}", main.hop);
    assert_eq!(
        main.body["tool_scope"]["require"],
        json!(true),
        "the round is required: {:?}",
        main.body.get("tool_scope")
    );
    assert!(
        bridges(&calls).is_empty(),
        "no side target, no bridge: {:?}",
        bridges(&calls)
    );
    let errors: Vec<&String> = h
        .stderr
        .iter()
        .filter(|l| l.contains("no call") || l.contains("Traceback"))
        .collect();
    assert!(errors.is_empty(), "no error without a bridge: {errors:?}");

    // The verdict names a fast target and its fallback, but no side target:
    // the lead gives the tool round no `side` entry -- one without a target
    // would reach `./policy`, which says on stderr that it stays in -- and
    // the required round leaves alone (OR-d58-2.target.71). The harness
    // wires talky's knobs, so `bridge_target` is cleared here.
    let (h, calls) = tool_turn(&[
        ("lead", "path_routes", json!("*:seat")),
        ("lead", "fallback", json!("seat:main")),
        ("lead", "bridge_target", json!("")),
    ]);
    let main = the_call(&h, &calls);
    assert_eq!(main.hop["target"], json!("seat"), "{:?}", main.hop);
    assert_eq!(
        main.body["tool_scope"]["require"],
        json!(true),
        "the round is required: {:?}",
        main.body.get("tool_scope")
    );
    assert!(
        bridges(&calls).is_empty(),
        "a fast target but no side target, no side call: {:?}",
        bridges(&calls)
    );
    let said: Vec<&String> = h
        .stderr
        .iter()
        .filter(|l| l.contains("not sent") || l.contains("no call") || l.contains("Traceback"))
        .collect();
    assert!(
        said.is_empty(),
        "the lead names no entry without a target, so the policy has none to refuse: {said:?}"
    );

    // An entry without a target has no side slot at the policy either; the
    // same entry with a target keeps the entry's slot.
    assert_eq!(
        side_slot(""),
        "",
        "an entry without a target has no side slot"
    );
    assert_eq!(
        side_slot("side"),
        "bridge",
        "an entry with a target keeps its slot"
    );

    // The verdict names the target, its fallback and a side target: the
    // bridge leaves for that target, beside the round.
    let (h, calls) = tool_turn(&[
        ("lead", "path_routes", json!("*:seat")),
        ("lead", "fallback", json!("seat:main")),
        ("lead", "bridge_target", json!("*:side")),
    ]);
    the_call(&h, &calls);
    let b = bridges(&calls);
    assert_eq!(b.len(), 1, "one bridge: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        b[0].hop["target"],
        json!("side"),
        "the bridge goes to the side target the verdict names: {:?}",
        b[0].hop
    );
}
