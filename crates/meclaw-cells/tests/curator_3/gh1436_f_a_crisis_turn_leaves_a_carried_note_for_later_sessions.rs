//! GH #1436 (d54g) -- a crisis turn leaves a carried note for later sessions.
//!
//! WHY this file exists: R043 (`konv` A, w3) measured that a crisis sticks
//! across sessions -- not through a note (the router's crisis note is a
//! `turn` note and gone after its turn) but through the recall, the wall and
//! the start windows, which bring the words back in every later window. The
//! recall is not cut by class (R-IG-1). What was missing is the rule R-K:
//! a later session is told that the matter was raised and answered, and not
//! to bring it up unasked. `carry_notes` (curator/policy, shipped `{}`) maps
//! the source of a turn note to `{text, until}`: a turn that carries a note
//! of such a source leaves a note `carry.<source>` of scope `until`, its
//! text the set's, its birth session in `not_session`. This pin: the row is
//! written as said, a window of a later session shows its text, the start
//! window of a later call carries it, and a set can switch the param (the
//! override of an instance passes the door, DISPATCH-BAU § 6).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{self as sj, Value, json};

const NOTE: &str = "The person may be in danger: answer with care.";
const CARRY: &str = "Earlier the person spoke of a crisis and it was answered. Do not bring it up unasked; if they come back to it, stay with it.";

fn carry() -> Value {
    json!({"route.risk": {"text": CARRY, "until": "day"}})
}

fn crisis() -> Value {
    json!([{"id": "route.risk", "source": "route.risk", "text": NOTE, "priority": 0}])
}

/// One answered turn of `session` on the talk way, `notes` as its turn
/// notes; returns the call that left for the model.
fn talk(h: &mut Hive, session: &str, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("I am here with you.")]),
    );
    call
}

fn carried(h: &Hive) -> Vec<Vec<Value>> {
    h.rows(
        "SELECT source, note_id, text, scope, until, not_session, session_id, used_turn \
         FROM notes WHERE source LIKE 'carry.%' ORDER BY source",
    )
}

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

#[test]
fn a_set_override_of_carry_notes_passes_the_door() {
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    let over = json!({"carry_notes": carry()});
    if let Err(why) = check_override_params(&node, Some("policy"), "curator", &over) {
        panic!("a set's {over} on curator/policy must pass the door: {why:?}");
    }
}

#[test]
fn a_crisis_turn_leaves_a_carried_note_for_later_sessions() {
    if !shipped() {
        return;
    }
    // The set's value as the override of an instance, not in the template.
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("policy", "carry_notes", carry()),
    ]);
    let first = talk(&mut h, "s1", "t1", crisis());
    assert!(
        texts(&first).iter().any(|t| t.contains(NOTE)),
        "the crisis turn shows its own note: {:?}",
        texts(&first)
    );
    let rows = carried(&h);
    assert_eq!(rows.len(), 1, "one carried note: {rows:?} {:?}", h.stderr);
    let r = &rows[0];
    assert_eq!(r[0], json!("carry.route.risk"), "{r:?}");
    assert_eq!(r[1], json!("route.risk"), "{r:?}");
    assert_eq!(r[2], json!(CARRY), "the text is the set's: {r:?}");
    assert_eq!(r[3], json!("until"), "{r:?}");
    assert!(
        !r[4].as_str().unwrap_or("").is_empty(),
        "a carried note ends: {r:?}"
    );
    assert_eq!(r[5], json!("s1"), "born in s1: {r:?}");
    assert_eq!(r[6], json!("s1"), "{r:?}");
    assert_eq!(r[7], json!(""), "{r:?}");

    // A later session: its window shows the carried note.
    let later = talk(&mut h, "s2", "t1", json!([]));
    assert!(
        texts(&later).iter().any(|t| t.contains(CARRY)),
        "a later session shows the carried note: {:?}",
        texts(&later)
    );
    assert!(
        !texts(&later).iter().any(|t| t.contains(NOTE)),
        "the crisis note itself stays its turn's: {:?}",
        texts(&later)
    );

    // The start window of a later call carries it as well.
    let seed = only(
        &seed_ask(&h, &history_params(&[], &["now_utc"]), "call-2"),
        "seed",
    );
    let body = sj::to_string(&seed.body).expect("the seed's body");
    assert!(
        body.contains(CARRY),
        "the start window of a later call carries the note: {body}"
    );
}
