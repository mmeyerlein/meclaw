//! GH #1248 (T4-12) -- a complex turn is bridged, a research turn names its wait.
//!
//! WHY this file exists: the owner's rulings R-WP-4 and R-WP-9. A turn stamped
//! `complex` leaves for the backend whole (GH #1247) and its answer comes in
//! the same turn; meanwhile the quick voice says ONE bridge sentence of its own
//! (no fixed sentence: an instruction, `instructions.bridge`), and the errand
//! carries the mark `bridge_wait_ms` (10 s) as `hop.consult_wait_ms`. A turn
//! stamped `research` gets a conversation move instead: the quick voice names
//! the waiting time (`research_wait_ms`, 2 min) and the conversation goes on;
//! the result comes in a later turn through the curator. The bridge call calls
//! no tool and carries no consult id.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn curate_on(h: &mut Hive, turn_id: &str, path: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path, "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let system = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "sidecar": {"text": "End with a sidecar block."}}});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": system}),
    );
}

fn bridge_of(h: &Hive, way: &str) -> Msg {
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "one bridge call beside the {way} errand: {:?} {:?}",
        h.out,
        h.stderr
    );
    let b = quick[0].clone();
    assert_eq!(b.hop["bridge"], json!(way), "{:?}", b.hop);
    assert!(
        b.hop.get("consult_id").is_none(),
        "the bridge is no consult: {:?}",
        b.hop
    );
    assert_eq!(
        b.body["tool_scope"],
        json!({"allow": []}),
        "the bridge calls no tool"
    );
    assert!(
        b.body["system"]["instructions"].get("sidecar").is_none(),
        "the bridge carries no sidecar contract: {:?}",
        b.body["system"]
    );
    b
}

#[test]
fn a_complex_turn_is_bridged_and_research_names_its_wait() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);

    curate_on(&mut h, "t1", "complex", "plan my week around the move");
    let consult = h.routed("consult");
    assert_eq!(consult.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        consult[0].hop["consult_wait_ms"],
        json!("10000"),
        "the errand carries the mark of the same turn"
    );
    let b = bridge_of(&h, "consult");
    let text = b.body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        text.contains("10 seconds") && text.contains("this turn"),
        "the bridge names the wait of the same turn: {text}"
    );

    curate_on(&mut h, "t2", "research", "find everything on the topic");
    let research = h.routed("research");
    assert_eq!(research.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(research[0].hop["consult_wait_ms"], json!("120000"));
    let b = bridge_of(&h, "research");
    let text = b.body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        text.contains("2 minutes") && text.contains("later turn"),
        "the research move names its waiting time and the later turn: {text}"
    );
}
