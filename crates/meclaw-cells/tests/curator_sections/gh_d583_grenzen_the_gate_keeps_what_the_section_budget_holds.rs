//! GH #1526 (d58-3 grenzen, O5) -- the objects gate keeps what the section
//! budget holds; it counts no items and cuts no note.
//!
//! WHY this file exists: `objects/gate` kept at most eight things a turn
//! (`MAX_ITEMS`) and emptied every note over 200 characters (`NOTE_MAX`) --
//! two fixed numbers on top of the one budget the section already has: the
//! curator's intake cuts `things` to its `cap_share` of the window of the
//! model that holds the wall, with a mark (GH #1473). A turn that named
//! twelve things lost four in silence; a note of 300 characters arrived
//! empty. The rule (contract point 3): what the section budget carries
//! stays; only that budget cuts, and it says so.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/objects_hive.rs"]
#[allow(unused_imports)]
use crate::objects_hive; // harness_fold: mod objects_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use objects_hive as obj;

fn twelve_with_a_long_note() -> Vec<Value> {
    let mut items: Vec<Value> = (1..=12)
        .map(|i| json!({"name": format!("thing-{i:02}"), "kind": "device"}))
        .collect();
    items[11]["note"] = json!("n".repeat(300));
    items
}

#[test]
fn the_gate_keeps_what_the_section_budget_holds() {
    if !shipped() || !obj::shipped() {
        return;
    }
    // Inside the budget: every entry and the whole note pass the gate.
    let items = twelve_with_a_long_note();
    let checked = obj::pure("gate", "clean_items(ARGS)", json!(items));
    let checked = checked.as_array().cloned().expect("a list");
    assert_eq!(
        checked.len(),
        12,
        "the gate counts no items -- twelve go: {checked:?}"
    );
    assert_eq!(
        checked[11]["note"].as_str().map(str::len),
        Some(300),
        "the gate cuts no note: {:?}",
        checked[11]
    );

    // Over the budget: the intake cuts, with a mark.
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({"things": {"lane": "thing_seen", "cap_share": 0.1}}),
    )]);
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("The router, the lamp and the desk.")]),
        mode("Be brief."),
    );
    let mut ctx = call.context.clone();
    ctx.insert("input_soft".into(), json!(100));
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        if let Some(v) = call.hop.get(k) {
            ctx.insert(k.into(), v.clone());
        }
    }
    ctx.insert("audience_set".into(), json!(ROUND_E));
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "things"}),
        json!({"messages": [], "section": "things", "payload": {"items": items}}),
    );
    let seen = h.routed("thing_seen");
    assert_eq!(seen.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cut = seen[0].hop["cuts"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["what"] == json!("things")).cloned())
        .unwrap_or_else(|| panic!("the full budget cuts with a mark: {:?}", seen[0].hop));
    assert!(cut["shown"].as_u64() < cut["total"].as_u64(), "{cut}");
}
