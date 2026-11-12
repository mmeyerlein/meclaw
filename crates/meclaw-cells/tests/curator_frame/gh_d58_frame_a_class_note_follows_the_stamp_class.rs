//! D58 frame (GH #1478, note per stamp class points 1 and 2) -- a class
//! note follows the stamp class.
//!
//! WHY this file exists: the curator knew one stamp class by name
//! (`clarify`) and gave its turns one fixed note (the knob `clarify_note`).
//! Which class a turn has is the composition's word (its route model); what
//! the curator does for a class is the composition's word too: the param
//! `class_notes` maps a class of the stamp to `{text, tools?, pair?}`. A turn
//! of a class with an entry gets the entry's `text` as a turn note (id the
//! class, source the curator); a turn of another class gets none, and the
//! turn that answers an errand (`consult_id` on the curate) gets none either,
//! whatever its stamp says. The name `clarify` is a class like any other:
//! its entry's text wins over the shipped wording.
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

const NOTE_VAGUE: &str = "Ask which of the two walks the person means.";
const NOTE_ASK: &str = "Ask one short question back before you answer.";

/// One curate of `turn` stamped `path` for the target `seat`, with further
/// hop keys `extra`; the one call.
fn curate_class(h: &mut Hive, turn: &str, path: &str, words: &str, extra: Value) -> Msg {
    let mut hop = json!({"path": path});
    for (k, v) in obj(extra) {
        hop[k] = v;
    }
    let calls = curate_for(h, Some("seat"), turn, 0, json!([user(words)]), hop);
    the_call(h, &calls)
}

/// The notes the `notes` mark of `call` names, shown and dropped.
fn notes_of(h: &Hive, call: &Msg) -> Vec<Value> {
    marks_of_kind(h, "notes")
        .into_iter()
        .filter(|m| m["call"] == call.hop["curator_call"])
        .flat_map(|m| {
            let mut all = m["notes_in"].as_array().cloned().unwrap_or_default();
            all.extend(m["notes_dropped"].as_array().cloned().unwrap_or_default());
            all
        })
        .collect()
}

fn in_window(call: &Msg, text: &str) -> bool {
    call.body
        .get("messages")
        .is_some_and(|m| m.to_string().contains(text))
}

#[test]
fn gh_d58_frame_a_class_note_follows_the_stamp_class() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[(
        "policy",
        "class_notes",
        json!({"vague": {"text": NOTE_VAGUE}, "clarify": {"text": NOTE_ASK}}),
    )]);
    announce_profile(&mut h, "seat", json!({"notes_share": 0.02, "speaks": true}));
    let mut failures = Vec::new();

    // A turn of the class `vague`: its note, under the class's name.
    let c = curate_class(&mut h, "t1", "vague", "The walk tomorrow.", json!({}));
    if !in_window(&c, NOTE_VAGUE) {
        failures.push(format!(
            "vague: the class note is not in the window: {:?}",
            h.stderr
        ));
    }
    let want = json!({"id": "vague", "source": "curator", "scope": "turn"});
    if !notes_of(&h, &c).contains(&want) {
        failures.push(format!(
            "vague: the notes mark does not name {want}: {:?}",
            notes_of(&h, &c)
        ));
    }
    answer(
        &mut h,
        &c,
        "stop",
        warm_usage(),
        json!([said("Which walk do you mean?")]),
    );

    // The class `clarify` takes its entry's text, not a shipped one.
    let c = curate_class(&mut h, "t2", "clarify", "That one.", json!({}));
    if !in_window(&c, NOTE_ASK) {
        failures.push("clarify: the entry's text is not in the window".into());
    }
    let shipped_text = "can be meant in more than one way";
    if in_window(&c, shipped_text) {
        failures.push("clarify: the shipped wording stands beside the entry's".into());
    }
    answer(
        &mut h,
        &c,
        "stop",
        warm_usage(),
        json!([said("Do you mean the river?")]),
    );

    // A class without an entry: no note.
    let c = curate_class(&mut h, "t3", "talk", "Yes, the river.", json!({}));
    if in_window(&c, NOTE_VAGUE) || in_window(&c, NOTE_ASK) {
        failures.push("talk: a class note in a turn of a class without an entry".into());
    }
    let ids: Vec<Value> = notes_of(&h, &c).iter().map(|n| n["id"].clone()).collect();
    if ids.contains(&json!("vague")) || ids.contains(&json!("clarify")) {
        failures.push(format!("talk: the notes mark names a class note: {ids:?}"));
    }
    answer(&mut h, &c, "stop", warm_usage(), json!([said("Fine.")]));

    // The turn that answers an errand: no class note, whatever the stamp.
    let c = curate_class(
        &mut h,
        "t4",
        "vague",
        "And the result?",
        json!({"consult_id": "k-errand-1"}),
    );
    if in_window(&c, NOTE_VAGUE) {
        failures.push("answered: the class note in the turn that answers an errand".into());
    }
    if notes_of(&h, &c).iter().any(|n| n["id"] == json!("vague")) {
        failures.push("answered: the notes mark names the class note".into());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
