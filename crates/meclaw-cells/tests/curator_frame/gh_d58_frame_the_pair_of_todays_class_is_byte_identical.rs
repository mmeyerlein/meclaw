//! D58 frame (GH #1478, note per stamp class point 5) -- the pair of today's
//! class is byte-identical.
//!
//! WHY this file exists: `talky` offers the route class `clarify` (its route
//! cell's knob `clarify`); the curator learnt that class's pair under fixed
//! strings, which affinity's `brief` reads by the field name. With the class
//! name gone from the curator, the composition sets the entry for its class
//! in `class_notes`: today's text, no tool, the pair under
//! `mx.hints.clarify`. For talky nothing may change: the note (id, source,
//! text), the empty menu, no frozen system part, the state row, and the
//! proposal byte for byte. The strings are pinned here as the curator wrote
//! them before the move.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) with the entry
//! talky's curator node sets, for a target cut with talky's main profile; no
//! model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The turn note of the class, as the curator shipped it before the move.
const TODAYS_TEXT: &str = "The person's words of this turn can be meant in more than one way. \
                           Answer with one short question about how they mean it -- guess \
                           nothing and answer nothing yet.";

/// uuid5 of the policy's namespace over "s1#t1" (python
/// `uuid.uuid5(UUID("5f0c2a8e-8b1d-4b0e-9a43-0c0a7e6f8801"), "s1#t1")`).
const PAIR_UUID: &str = "0812f083-0125-5ee3-a92e-1308f267cb23";

const QUESTION: &str = "Do you mean its concerts or its website?";

fn turn(h: &mut Hive, turn: &str, path: &str, words: &str) -> Msg {
    let calls = curate_for(
        h,
        Some("seat"),
        turn,
        0,
        json!([user(words)]),
        json!({"path": path}),
    );
    the_call(h, &calls)
}

#[test]
fn gh_d58_frame_the_pair_of_todays_class_is_byte_identical() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let want_entry = json!({"clarify": {"text": TODAYS_TEXT, "tools": "none",
                                        "pair": "mx.hints.clarify"}});
    let node = read_json(&repo("templates/talky/curator/config.json"));
    let entry = node["override_params"]["policy"]["class_notes"].clone();
    if entry != want_entry {
        failures.push(format!(
            "talky's curator node sets for its class `clarify` today's text, no tool and the \
             pair: {entry}"
        ));
    }
    let main =
        read_json(&repo("templates/talky/brain/config.json"))["params"]["window_profile"].clone();

    let mut h = Hive::with(&[("policy", "class_notes", want_entry)]);
    announce_profile(&mut h, "seat", main);

    let c = turn(
        &mut h,
        "t1",
        "clarify",
        "the choir in Dresden, search please",
    );
    let marks: Vec<Value> = marks_of_kind(&h, "notes")
        .into_iter()
        .filter(|m| m["call"] == c.hop["curator_call"])
        .collect();
    let note = json!({"id": "clarify", "source": "curator", "scope": "turn"});
    if !marks
        .iter()
        .any(|m| m["notes_in"].as_array().is_some_and(|a| a.contains(&note)))
    {
        failures.push(format!(
            "the notes mark shows {note}: {marks:?} {:?}",
            h.stderr
        ));
    }
    if !c
        .body
        .get("messages")
        .is_some_and(|m| m.to_string().contains(TODAYS_TEXT))
    {
        failures.push("today's text is not in the window".into());
    }
    if c.body.get("tool_scope") != Some(&json!({"allow": []})) {
        failures.push(format!(
            "the menu is not empty: {:?}",
            c.body.get("tool_scope")
        ));
    }
    if !h
        .rows("SELECT key FROM state WHERE key LIKE 'turn_system:s1:t1%'")
        .is_empty()
    {
        failures.push("a frozen system part of the asking turn".into());
    }
    let row: Value = sj::from_str(&h.state("clarify:s1")).unwrap_or(Value::Null);
    if row["turn"] != json!("t1") || row["call"] != c.hop["curator_call"] || !row["at"].is_string()
    {
        failures.push(format!("the row `clarify:s1`: {row}"));
    }
    answer(&mut h, &c, "stop", warm_usage(), json!([said(QUESTION)]));

    let c = turn(&mut h, "t2", "talk", "its concerts");
    let p = h.routed("propose");
    if p.len() != 1 {
        failures.push(format!("one proposal: {:?} {:?}", h.out, h.stderr));
    } else {
        let want_header = json!({"route": "propose", "session_id": "s1", "turn_id": "t2"});
        for (k, v) in want_header.as_object().cloned().unwrap_or_default() {
            if p[0].hop.get(&k) != Some(&v) {
                failures.push(format!(
                    "the proposal's header `{k}`: {:?}",
                    p[0].hop.get(&k)
                ));
            }
        }
        let m = &p[0].messages()[0];
        let want_args = json!({
            "op": "propose",
            "id": format!("clarify:{PAIR_UUID}"),
            "source_ref": "curator:clarify:s1#t1",
            "entity_ref": "member:e",
            "field_path": "mx.hints.clarify",
            "value": {"question": QUESTION, "answer": "its concerts", "class": "talk",
                      "session_id": "s1", "asked_turn": "t1", "answered_turn": "t2"}
        });
        let want_msg = json!({"origin": "assistant", "type": "tool_call", "id": "clarify-t1",
                              "text": canonical(&want_args)});
        if *m != want_msg {
            failures.push(format!(
                "the proposal, byte for byte:\n got {m}\nwant {want_msg}"
            ));
        }
    }
    if !h.state("clarify:s1").is_empty() {
        failures.push("the row `clarify:s1` is not taken".into());
    }
    answer(
        &mut h,
        &c,
        "stop",
        warm_usage(),
        json!([said("They sing on Friday.")]),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
