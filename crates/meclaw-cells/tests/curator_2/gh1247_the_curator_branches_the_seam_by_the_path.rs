//! GH #1247 (T4-07) -- the curator branches the seam by the turn's path.
//!
//! WHY this file exists: the route cell of talky stamps every turn with a
//! path (`talk`, `simple`, `complex`, `research`), and the collector carries it
//! on the seam as `hop.path` (GH #1244); the parent edge lifts it into
//! `context.path`. The curator is the one who decides (R-WP-1): a `talk` or
//! `simple` turn leaves on `quick` with the short window of the role `quick`
//! (the last `keep_recent` 4 elements, no recall, no sidecar contract, the
//! tool scope allowing only the families of the stamp), `complex` leaves on
//! `consult`, `research` on `research`, and a turn without a path keeps the
//! road of before, `brain`. Each call that had a path leaves one `route` mark.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn curate_on(h: &mut Hive, turn_id: &str, path: &str, families: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path, "families": families});
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
fn the_curator_branches_the_seam_by_the_path() {
    if !shipped() {
        return;
    }
    let (keep, _) = policy_scope(json!({}), "ROLES['quick']['keep_recent']", Value::Null);
    assert_eq!(
        keep,
        json!(4),
        "the role quick keeps the last four elements"
    );
    let (recall, _) = policy_scope(json!({}), "ROLES['quick']['recall_push']", Value::Null);
    assert_eq!(recall, json!(""), "the role quick pushes no recall");

    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for i in 0..4 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            &format!("question {i}"),
            &format!("answer {i}"),
            json!({}),
        );
    }

    curate_on(&mut h, "t9", "simple", "web", "what is the weather?");
    assert!(
        h.routed("brain").is_empty(),
        "a simple turn does not take the brain's road: {:?}",
        h.out
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "one call on quick: {:?} {:?}",
        h.out,
        h.stderr
    );
    let q = &quick[0];
    let msgs = q.messages();
    assert!(
        !msgs.is_empty() && msgs.len() <= 4,
        "the quick window holds at most four elements: {msgs:?}"
    );
    // The talky role shows short ids (`[#<id>] <text>`).
    assert!(
        texts(q)
            .last()
            .is_some_and(|t| t.ends_with("what is the weather?")),
        "the newest element is the turn itself: {:?}",
        texts(q)
    );
    assert_eq!(
        q.body["tool_scope"],
        json!({"allow": ["web"], "hint": ["web"]}),
        "the quick scope allows the stamp's families and nothing else, in the hint \
         form (GH #1248, T4-12)"
    );
    assert!(
        q.body["system"]["instructions"].get("sidecar").is_none(),
        "the quick window carries no sidecar contract: {:?}",
        q.body["system"]
    );
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'route'");
    assert_eq!(
        marks.len(),
        1,
        "one route mark for the one call with a path"
    );
    let mark: Value = meclaw_core::serde_json::from_str(marks[0][0].as_str().unwrap_or("{}"))
        .expect("the mark is JSON");
    assert_eq!(mark["path"], json!("simple"));
    assert_eq!(mark["way"], json!("quick"));

    curate_on(
        &mut h,
        "t10",
        "research",
        "",
        "find everything on the topic",
    );
    assert_eq!(
        h.routed("research").len(),
        1,
        "research leaves on research: {:?}",
        h.out
    );

    curate_on(&mut h, "t11", "", "", "and without a stamp?");
    assert_eq!(
        h.routed("brain").len(),
        1,
        "no path, the road of before: {:?}",
        h.out
    );
    let brain = &h.routed("brain")[0];
    assert!(
        brain.body["system"]["instructions"]
            .get("sidecar")
            .is_some(),
        "after a quick call the brain gets its whole system part again: {:?}",
        brain.body["system"]
    );
}
