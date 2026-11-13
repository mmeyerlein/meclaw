//! GH #1512 -- a turn note reaches the model that answers the turn, whatever
//! call of the turn came first.
//!
//! Measured in the lab (twice): the crisis note of the router (`route.risk`,
//! priority 0) did not reach the speaker in the first turn of a freshly
//! born colony. The turn's first call was the speculative one (`hop.spec`
//! "1") to `quick_spec`, before the stamp -- so without the turn's notes.
//! `quick_spec` had reported no profile yet, so it was cut on the instance's
//! values (`system` `delta`, `notes_share` above 0) and, as a `delta` call,
//! wrote the turn's system row (`turn_system:<session>:<turn>`, GH #1298)
//! with ITS notes block. The brain's call of the same turn found that row,
//! took the frozen block, and never read `context.turn_notes`: the note was
//! gone.
//!
//! The lock: (a) a spec call neither writes nor reads the turn's row; (b) the
//! row is bound to the target whose call wrote it -- a call to another
//! target builds its own block with every note of its context. GH #1298
//! stays: the tool rounds of the SAME target keep the system of the turn's
//! first call (`gh1298_a_tool_round_keeps_its_system`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const NOTE: &str = "The person may be in danger: answer with care.";

fn turn_note() -> Value {
    json!([{"id": "risk", "source": "route", "text": NOTE, "priority": 0}])
}

fn system_rows(h: &Hive) -> Vec<String> {
    h.rows("SELECT key FROM state WHERE key LIKE 'turn_system:%' ORDER BY key")
        .iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .collect()
}

fn shows_note(call: &Msg) -> bool {
    texts(call).iter().any(|t| t.contains(NOTE))
}

/// A freshly born colony: the brain reported its package, `quick_spec` did
/// not yet; the instance's knobs hold `notes_share` above 0. `spec_target`
/// names the target of the turn's spec call.
fn fresh(spec_target: &str) -> Hive {
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("")),
        ("lead", "spec_target", json!(spec_target)),
    ]);
    h.report_row("talky", "brain", json!({"model": "test/brain"}));
    h.target = Some(MAIN_TARGET.to_string());
    h
}

/// The spec curate of turn `t1` -- before the stamp, so without its notes.
fn spec_call(h: &mut Hive, way: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed(way);
    assert_eq!(calls.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

/// The turn's normal curate after the stamp: path `talk` (the brain), the
/// router's note in its context.
fn brain_call(h: &mut Hive) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": turn_note()});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one brain call: {:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

/// P1 (fix a, the measured case): the spec call to `quick_spec` without a
/// profile leaves no row; the brain's call shows the turn's note.
#[test]
fn the_first_turn_note_reaches_the_brain_after_a_spec_call() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut h = fresh("quick_spec");
    let spec = spec_call(&mut h, "quick");
    assert_eq!(spec.hop["target"], json!("quick_spec"), "{:?}", spec.hop);
    let spec_rows = system_rows(&h);
    let brain = brain_call(&mut h);
    assert!(
        shows_note(&brain),
        "the brain shows the turn's note: {:?}",
        texts(&brain)
    );
    assert!(
        spec_rows.is_empty(),
        "a spec call writes no turn row: {spec_rows:?}"
    );
}

/// P1b (fix a, the read side): a spec call to the very target of the turn's
/// normal call still leaves that call its own block -- the spec call did not
/// write the row, so the target binding alone cannot be what saves it.
#[test]
fn a_spec_call_to_the_same_target_leaves_the_turn_its_notes() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut h = fresh(MAIN_TARGET);
    spec_call(&mut h, "brain");
    let brain = brain_call(&mut h);
    assert!(
        shows_note(&brain),
        "the brain shows the turn's note: {:?}",
        texts(&brain)
    );
}

/// A call of turn `t1` to `target` (none reported a profile: each holds its
/// tree on the instance's values), with `notes` as the turn's notes.
fn staged(h: &mut Hive, target: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "target": target});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on.")], "system": mode("Be brief.")}),
    );
    let calls = h.routed(target);
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

/// P2 (fix b): the row of another target's call is not taken -- the second
/// target builds its own block with every note of its context.
#[test]
fn a_turn_row_of_another_target_is_not_taken() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "notes_share", json!(0.04))]);
    let fast = staged(&mut h, "fast", json!([]));
    assert!(!shows_note(&fast), "{:?}", texts(&fast));
    assert_eq!(system_rows(&h), vec!["turn_system:s1:t1"], "{:?}", h.stderr);
    let strong = staged(&mut h, "strong", turn_note());
    assert!(
        shows_note(&strong),
        "the other target shows the turn's note: {:?}",
        texts(&strong)
    );
}
