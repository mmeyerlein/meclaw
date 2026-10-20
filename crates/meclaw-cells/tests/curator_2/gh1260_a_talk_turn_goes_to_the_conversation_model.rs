//! GH #1260 -- a turn stamped `talk` goes to the conversation model.
//!
//! WHY this file exists: the route cell stamps a plain conversational turn
//! `talk`, and the owner's line for it is the conversation model with its
//! whole sidecar (the brain), not the fast model. Until this strand the shipped
//! `path_routes` sent `talk` onto `quick`, and every deployment that wanted the
//! line had to override it (the soul2me set did). The shipped default now
//! names `talk:brain`; a turn without a path keeps the brain as before.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DEFAULT: &str = "talk:brain,simple:quick,complex:consult,research:research";

fn curate_on(h: &mut Hive, turn_id: &str, path: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path, "families": ""});
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

#[test]
fn a_talk_turn_goes_to_the_conversation_model() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["path_routes"],
        json!(DEFAULT),
        "the shipped param sends talk to the brain"
    );
    assert_eq!(
        policy["contract"]["settings"]["path_routes"]["default"],
        json!(DEFAULT),
        "the schema default says the same"
    );

    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    curate_on(&mut h, "t1", "talk", "how was your day?");
    assert!(
        h.routed("quick").is_empty(),
        "a talk turn does not take the fast model: {:?}",
        h.out
    );
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "one call of the conversation model: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        brain[0].body["system"]["instructions"]
            .get("sidecar")
            .is_some(),
        "the talk window carries the whole sidecar contract: {:?}",
        brain[0].body["system"]
    );
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'route'");
    assert_eq!(
        marks.len(),
        1,
        "one route mark for the one call with a path"
    );
    let mark: Value = meclaw_core::serde_json::from_str(marks[0][0].as_str().unwrap_or("{}"))
        .expect("the mark is JSON");
    assert_eq!(mark["path"], json!("talk"));
    assert_eq!(mark["way"], json!("brain"));

    curate_on(&mut h, "t2", "", "and without a stamp?");
    assert_eq!(
        h.routed("brain").len(),
        1,
        "no path, the brain as before: {:?}",
        h.out
    );
}
