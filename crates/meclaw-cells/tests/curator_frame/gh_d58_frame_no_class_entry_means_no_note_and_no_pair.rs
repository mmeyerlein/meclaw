//! D58 frame (GH #1478, note per stamp class points 1 and 4) -- no class
//! entry means no note and no pair.
//!
//! WHY this file exists: the curator names no stamp class of its own. A turn
//! stamped `clarify` on a curator whose `class_notes` hold no entry for it
//! is a turn like any other: no turn note, its target's tool scope, the
//! frozen system part of its turn, no state row and no proposal at the
//! person's next words. That holds for the shipped curator (it ships no
//! entry) and for one whose entries name only other classes.
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

/// A part of the wording the curator shipped for the class before the move.
const SHIPPED_WORDING: &str = "can be meant in more than one way";

fn scope_in() -> Value {
    json!({"allow": ["web", "weather"]})
}

fn curate_class(h: &mut Hive, turn: &str, path: &str, words: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "target": "seat", "path": path});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(words)], "system": system_tree(), "tool_scope": scope_in()}),
    );
    let calls = model_calls(h);
    the_call(h, &calls)
}

/// What a run of a `clarify` turn and the next words did that only a class
/// entry may do.
fn findings(name: &str, mut h: Hive) -> Vec<String> {
    let mut out = Vec::new();
    announce_profile(
        &mut h,
        "seat",
        json!({"system": "delta", "notes_share": 0.02}),
    );
    let c = curate_class(
        &mut h,
        "t1",
        "clarify",
        "the choir in Dresden, search please",
    );
    let notes = marks_of_kind(&h, "notes");
    if notes
        .iter()
        .any(|m| m.to_string().contains("\"id\":\"clarify\""))
    {
        out.push(format!(
            "{name}: the notes mark names a class note: {notes:?}"
        ));
    }
    if c.body
        .get("messages")
        .is_some_and(|m| m.to_string().contains(SHIPPED_WORDING))
    {
        out.push(format!("{name}: a class note in the window"));
    }
    if c.body.get("tool_scope") != Some(&scope_in()) {
        out.push(format!(
            "{name}: the target's scope is not kept: {:?} {:?}",
            c.body.get("tool_scope"),
            h.stderr
        ));
    }
    if h.rows("SELECT key FROM state WHERE key LIKE 'turn_system:s1:t1%'")
        .is_empty()
    {
        out.push(format!("{name}: no frozen system part of the turn"));
    }
    if !h
        .rows("SELECT key FROM state WHERE key LIKE 'clarify:%'")
        .is_empty()
    {
        out.push(format!("{name}: a state row `clarify:`"));
    }
    answer(
        &mut h,
        &c,
        "stop",
        warm_usage(),
        json!([said("Do you mean its concerts or its website?")]),
    );
    curate_class(&mut h, "t2", "talk", "its concerts");
    if !h.routed("propose").is_empty() {
        out.push(format!("{name}: a proposal: {:?}", h.routed("propose")));
    }
    out
}

#[test]
fn gh_d58_frame_no_class_entry_means_no_note_and_no_pair() {
    if !shipped() {
        return;
    }
    let mut failures = findings("shipped", Hive::new());
    failures.extend(findings(
        "other classes only",
        Hive::with(&[(
            "policy",
            "class_notes",
            json!({"vague": {"text": "Ask which one is meant.", "tools": "none",
                             "pair": "mx.hints.vague"}}),
        )]),
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
