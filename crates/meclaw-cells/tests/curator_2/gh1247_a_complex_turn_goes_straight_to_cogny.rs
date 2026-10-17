//! GH #1247 (T4-11) -- a complex turn goes straight to cogny, without a call
//! of the voice model.
//!
//! WHY this file exists: before, a complex question cost a call of talky's
//! voice model only to decide that it should consult cogny (the tool call
//! `consult_cogny`). With the route stamp the curator knows it at the seam
//! (R-WP-5): a `complex` turn leaves the curator on `consult` with the whole
//! window as the errand and its own `consult_id`, talky hands it out on
//! `consult`, the assistant routes it to cogny like the tool call, and the
//! answer comes back on `in_advice` as today. Until it does, the session's
//! speaking calls carry `system.consult.pending` (#1198); the round that
//! answers the consult leaves on `brain` and takes the mark down.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn curate_with(h: &mut Hive, turn_id: &str, path: &str, extra_ctx: Value, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in extra_ctx.as_object().cloned().unwrap_or_default() {
        ctx[k] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
}

fn edge<'a>(edges: &'a [Value], from: &str, to: &str, needle: &str) -> Option<&'a Value> {
    edges.iter().find(|e| {
        e["from"] == json!(from)
            && e["to"] == json!(to)
            && e["condition"].as_str().unwrap_or("").contains(needle)
    })
}

#[test]
fn a_complex_turn_goes_straight_to_cogny() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let t_edges = talky["params"]["graph"]["edges"].as_array().expect("edges");
    let out = edge(t_edges, "./curator", ".", "'consult'").expect("talky hands a consult out");
    assert_eq!(out["modifier"]["set_hop"]["route"], json!("'consult'"));
    let assistant = read_json(&repo("templates/assistant/config.json"));
    let a_edges = assistant["params"]["graph"]["edges"]
        .as_array()
        .expect("edges");
    let to_cogny = edge(a_edges, "./talky", "./cogny", "hop.route == 'consult'")
        .expect("the assistant routes talky's consult to cogny");
    assert_eq!(to_cogny["modifier"]["set_hop"]["route"], json!("'in_turn'"));
    assert_eq!(
        to_cogny["modifier"]["set_context"]["consult_id"],
        json!("hop.consult_id")
    );

    // A day of `consult_pending_ms`: the mark outlives any test, so no
    // duration between two calls decides this test (dev-rules § 11).
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "consult_pending_ms", json!(86_400_000)),
    ]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    curate_with(
        &mut h,
        "t1",
        "complex",
        json!({}),
        "plan my week around the move",
    );
    assert!(h.routed("brain").is_empty(), "no call of the voice model");
    let consult = h.routed("consult");
    assert_eq!(consult.len(), 1, "one errand: {:?} {:?}", h.out, h.stderr);
    let c = &consult[0];
    let cid = c.hop["consult_id"].as_str().unwrap_or("").to_string();
    assert!(!cid.is_empty(), "the errand carries its consult id");
    assert_eq!(c.hop["consult_class"], json!("consult"));
    assert!(
        c.body.get("system").is_none(),
        "the errand carries no talky system part"
    );
    // The talky role shows short ids (`[#<id>] <text>`).
    assert!(
        texts(c)
            .last()
            .is_some_and(|t| t.ends_with("plan my week around the move")),
        "the whole window, the turn last: {:?}",
        texts(c)
    );

    curate_with(&mut h, "t2", "", json!({}), "are you still there?");
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "a plain turn meanwhile: {:?}", h.out);
    let pending = &brain[0].body["system"]["consult"]["pending"]["text"];
    assert!(
        pending.as_str().unwrap_or("").contains(&cid),
        "the voice model is told the consult is pending: {:?}",
        brain[0].body["system"]
    );

    curate_with(
        &mut h,
        "t3",
        "complex",
        json!({"consult_id": cid}),
        "cogny's answer",
    );
    let back = h.routed("brain");
    assert_eq!(back.len(), 1, "the answer is spoken, not consulted again");
    assert!(
        back[0].body["system"]["consult"].get("pending").is_none(),
        "the answer takes the pending mark down: {:?}",
        back[0].body["system"]
    );
}

/// Re-check I1: a consult whose answer never comes (cogny's error, a lost
/// message) lets go of its mark at its `until` -- the time of the consult plus
/// `consult_pending_ms`, the collector's deadline for the answer -- and the
/// voice model may consult again. The time is the mark's own stamp, moved
/// here into the past; the wall clock decides nothing.
#[test]
fn a_consult_without_an_answer_lets_go_at_its_until() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate_with(
        &mut h,
        "t1",
        "complex",
        json!({}),
        "plan my week around the move",
    );
    assert_eq!(h.routed("consult").len(), 1, "{:?} {:?}", h.out, h.stderr);
    let key = "SELECT value FROM state WHERE key = 'consult_pending:s1'";
    let rows = h.rows(key);
    assert_eq!(rows.len(), 1, "the consult leaves its mark: {:?}", h.stderr);
    let mut mark: Value =
        meclaw_core::serde_json::from_str(rows[0][0].as_str().unwrap_or("")).unwrap_or_default();
    let at = mark["at"].as_str().unwrap_or("").to_string();
    assert!(
        mark["until"].as_str().is_some_and(|u| u > at.as_str()),
        "the mark carries its until, after its at: {mark}"
    );

    mark["until"] = json!("2000-01-01T00:00:00.000000Z");
    h.db.execute(
        "UPDATE state SET value = ?1 WHERE key = 'consult_pending:s1'",
        rusqlite::params![mark.to_string()],
    )
    .expect("age the mark");
    curate_with(&mut h, "t2", "", json!({}), "are you still there?");
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "a plain turn: {:?} {:?}", h.out, h.stderr);
    assert!(
        brain[0].body["system"]["consult"].get("pending").is_none(),
        "past its until the voice model is not told of the consult: {:?}",
        brain[0].body["system"]
    );
    assert!(
        h.rows(key).is_empty(),
        "the mark is gone: {:?}",
        h.rows(key)
    );
}
