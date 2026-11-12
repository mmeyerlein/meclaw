//! GH #1473 (D58 sections) -- a section books the mark its registration
//! names, and no other.
//!
//! WHY this file exists: until GH #1473 the `topic` mark was tied to the name
//! `memory` (`section_marks`): any `memory` section booked one, registered or
//! not, and no other section could. With registration the mark kind is the
//! composition's (`mark`): a section the test calls `thread`, registered with
//! `mark` "topic", books the topic it names; `memory`, registered without a
//! mark, books none. Both still leave on their lane. A registration with a
//! mark and no lane is spent here by its mark and is not checked, though the
//! splitter stamped `sidecar_verify` (review I1).
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn call_of(h: &mut Hive) -> Msg {
    h.curate(
        "s1",
        "t1",
        0,
        json!([user("Let us talk about the garden.")]),
        mode("Be brief."),
    )
}

fn send(h: &mut Hive, call: &Msg, section: &str, payload: Value) {
    let mut ctx = call.context.clone();
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
        json!({"section": section}),
        json!({"messages": [], "section": section, "payload": payload}),
    );
}

fn topic_marks(h: &Hive) -> Vec<String> {
    h.rows("SELECT value FROM marks WHERE kind = 'topic' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].to_string())
        .collect()
}

#[test]
fn a_mark_is_booked_as_registered() {
    if !shipped() {
        return;
    }
    assert!(
        cell_config("intake")["params"]["sections"].is_object(),
        "curator/intake declares the param `sections` (GH #1473)"
    );
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({
            "thread": {"lane": "sidecar", "mark": "topic"},
            "memory": {"lane": "sidecar"}
        }),
    )]);
    let call = call_of(&mut h);

    send(
        &mut h,
        &call,
        "thread",
        json!({"topic": {"movement": "start", "name": "garden"}}),
    );
    assert_eq!(h.routed("sidecar").len(), 1, "{:?} {:?}", h.out, h.stderr);
    let marks = topic_marks(&h);
    assert_eq!(
        marks.len(),
        1,
        "the registered mark is booked once: {marks:?}"
    );
    assert!(marks[0].contains("garden"), "{marks:?}");
    assert!(marks[0].contains("start"), "{marks:?}");

    send(
        &mut h,
        &call,
        "memory",
        json!({"topic": {"movement": "start", "name": "bikes"}, "facts": []}),
    );
    assert_eq!(h.routed("sidecar").len(), 1, "{:?} {:?}", h.out, h.stderr);
    let marks = topic_marks(&h);
    assert!(
        !marks.iter().any(|m| m.contains("bikes")),
        "memory registered without a mark books none: {marks:?}"
    );
}

const COGNY_SPLITTER: &str = "templates/cogny/splitter/config.json";

/// The shipped cogny splitter over one answer whose `memory` names `facts`
/// and a topic, under the splitter's shipped `sidecar_verify`: the section
/// as it leaves the splitter, `hop.sidecar_verify` stamped.
fn split_memory(facts: Value) -> Value {
    let path = repo(COGNY_SPLITTER);
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the cogny splitter ships"),
    )
    .expect("the cogny splitter is JSON");
    let block = json!({"memory": {"facts": facts,
                                  "topic": {"movement": "start", "name": "porto",
                                            "source": "person"}}});
    let doc = json!({
        "header": {"hop": {"finish_reason": "stop"},
                   "context": {"session_id": "s1", "turn_id": "t"}},
        "messages": [said(&format!("Porto is lovely.\n\n```sidecar\n{block}\n```"))],
        "params": {"sidecar_verify": cfg["params"]["sidecar_verify"].clone(),
                   "id_sections": cfg["params"]["id_sections"].clone()},
    });
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(&path.to_string_lossy()),
        &doc,
    );
    out.into_iter()
        .find(|m| m["header"]["section"] == json!("memory"))
        .expect("the splitter hands on the memory section")
}

fn marks_of(h: &Hive, kind: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].to_string())
    .collect()
}

/// Review I1 of GH #1473: a registration without a lane is spent here by
/// its mark and is not checked -- as cogny's `memory` never was before
/// curator 2.0 (cogny pulls no `reject` route from its curator, so every
/// refused fact would have been a dead letter `no_route`). The rule is the
/// registration's, not a name's: no `sidecar_verify` read, no `reject`, no
/// `verify_reject` and no `fact` mark, only the registered `topic`, and the
/// skip said on stderr.
#[test]
fn a_registration_without_a_lane_is_not_checked() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("intake", "sections", json!({"memory": {"mark": "topic"}}))]);
    turn(
        &mut h,
        "s1",
        "r1",
        "My sister Hannah moved to Porto last spring.",
        "That is a big change for her.",
        json!({}),
    );
    let call = h.curate(
        "s1",
        "r2",
        0,
        json!([user(
            "[advice from your reasoning core, consult c-1473]\nAsk whether the new flat has a garden."
        )]),
        mode("Be brief."),
    );
    let msg = split_memory(json!([
        {"subject": "Hannah", "predicate": "lives_in", "claim": "Hannah lives in Porto.",
         "quote": "Hannah moved to Porto last spring", "source": "person"},
        {"subject": "Hannah", "predicate": "lives_in", "claim": "Hannah lives in Lisbon.",
         "quote": "Hannah moved to Lisbon", "source": "person"}
    ]));
    assert!(
        msg["header"]["sidecar_verify"].is_string(),
        "the splitter stamps its rule on the hop: {msg}"
    );
    let mut ctx = call.context.clone();
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
    let mut hop = msg["header"].clone();
    hop.as_object_mut().expect("hop").remove("route");
    let mut body = msg.clone();
    body.as_object_mut().expect("body").remove("header");
    h.out.clear();
    h.lane("in_section", Value::Object(ctx), hop, body);

    assert!(
        h.routed("reject").is_empty(),
        "no reject for a section nobody checks: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        h.routed("sidecar").is_empty(),
        "a registration without a lane leaves on none: {:?}",
        h.out
    );
    assert!(
        marks_of(&h, "verify_reject").is_empty(),
        "no verify_reject mark: {:?}",
        marks_of(&h, "verify_reject")
    );
    assert!(
        marks_of(&h, "fact").is_empty(),
        "no fact mark: {:?}",
        marks_of(&h, "fact")
    );
    let topics = marks_of(&h, "topic");
    assert!(
        topics.iter().any(|m| m.contains("porto")),
        "the registered topic is booked: {topics:?} {:?}",
        h.stderr
    );
    assert!(
        h.stderr.iter().any(|l| l.contains("is not checked here")),
        "the skip is said: {:?}",
        h.stderr
    );
}
