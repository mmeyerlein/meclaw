//! GH #1471 (D58 roundpin, contract point 2) -- a later call of a tool round
//! costs no extra step at the store.
//!
//! WHY this file exists: a tool round keeps the plan of its first call, and
//! every later call of the round reads it. Each message to the store is a
//! round trip in the middle of a call that a person waits for; the plan the
//! round keeps must ride in the messages a call sends to the store anyway.
//! Counted here at the store, message by message (sender and step): a later
//! call of a tool round takes the same steps whether a newer plan stands
//! or not, and the same steps as a call without the form. The same holds
//! for a turn that keeps all a turn can keep: the summary leaf and the
//! handover leaf of its first call, with the actions of a rebuild waiting.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ID: &str = "call_tool_rpsteps_0001";

fn talk(form: &str) -> Talk {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!(form)),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(2)),
    ]);
    for i in 1..=6 {
        t.turn(
            &format!("t{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    t
}

/// The messages the store answered while the first and while the second
/// call of a tool round were built, `(sender, step)` each. With
/// `rebuild`, a rebuild writes its plan between the two calls.
fn steps_of_a_tool_round(form: &str, rebuild: bool) -> [Vec<(String, String)>; 2] {
    clear_of_midnight();
    let mut t = talk(form);
    let at = t.h.ledger_steps.len();
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    let first = t.h.ledger_steps[at..].to_vec();
    t.answer(&c0, "tool_calls", json!([tool_call(ID, "read_file")]));
    if rebuild {
        t.strike("- The person told items 1 to 4.");
        assert!(
            t.cover() > 0,
            "the plan of the rebuild is written while the tool round runs: {:?}",
            t.h.stderr
        );
    }
    let at = t.h.ledger_steps.len();
    t.call(
        "t7",
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
    );
    let later = t.h.ledger_steps[at..].to_vec();
    assert!(
        !first.is_empty() && !later.is_empty(),
        "check of the setup: the calls were built at the store"
    );
    [first, later]
}

/// Pin: the later call reads the plan its tool round keeps in the messages
/// it sends anyway -- with a newer plan standing it takes the very steps it
/// takes without one.
#[test]
fn gh_d58_roundpin_a_later_call_costs_no_extra_store_step() {
    if !shipped() {
        return;
    }
    let [_, plain] = steps_of_a_tool_round("1", false);
    let [_, kept] = steps_of_a_tool_round("1", true);
    assert_eq!(
        kept.len(),
        plain.len(),
        "a later call under a newer plan sends as many messages to the store: {kept:?} / {plain:?}"
    );
    assert_eq!(kept, plain, "and the same ones, step by step");
}

/// Lock: what a tool round keeps costs no message of its own -- the first
/// and the later call take the steps of a call without the form.
#[test]
fn the_form_adds_no_store_step_to_a_call() {
    if !shipped() {
        return;
    }
    let [first_off, later_off] = steps_of_a_tool_round("0", false);
    let [first_on, later_on] = steps_of_a_tool_round("1", false);
    assert_eq!(first_on, first_off, "the first call of a tool round");
    assert_eq!(later_on, later_off, "a later call of a tool round");
}

const NOTE: &str = "What the session before left open: the file.";

/// The handover leaf as `./handover` lays it beside its binding: the block,
/// the slot, and `handover_audience` bound to the block's hash. Returns the
/// hash.
fn lay_the_handover_leaf(t: &Talk) -> String {
    let leaf = json!({"path": "history.handover", "text": NOTE});
    let hash = sha256_hex(&canonical(&leaf));
    let db = &t.h.db;
    db.execute(
        "INSERT OR IGNORE INTO blocks (hash, kind, chars, body, first_seen) \
         VALUES (?1, 'system', ?2, ?3, 'x')",
        rusqlite::params![hash, NOTE.len() as i64, canonical(&leaf)],
    )
    .expect("the block of the leaf");
    db.execute("DELETE FROM slots WHERE path = 'history.handover'", [])
        .expect("no older leaf");
    db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES ('history.handover', ?1, 'curator', 'x')",
        [&hash],
    )
    .expect("the slot of the leaf");
    let bound = canonical(&json!({"audience": round_canon(ROUND_E), "hash": hash}));
    db.execute("DELETE FROM state WHERE key = 'handover_audience'", [])
        .expect("no older binding");
    db.execute(
        "INSERT INTO state (key, value) VALUES ('handover_audience', ?1)",
        [&bound],
    )
    .expect("the binding");
    hash
}

fn slot_hash(t: &Talk, path: &str) -> Value {
    t.h.rows(&format!("SELECT hash FROM slots WHERE path = '{path}'"))
        .first()
        .map(|r| r[0].clone())
        .unwrap_or(Value::Null)
}

/// Pin (the condition of the plan): a turn that keeps the summary leaf and
/// the handover leaf of its first call and finds the actions of a rebuild
/// waiting -- before its first call and again before a later one, after a
/// rebuild in its middle replaced the one leaf and took the other down --
/// sends the store the very messages of a turn that keeps none of them,
/// call by call.
#[test]
fn a_turn_that_keeps_both_leaves_and_finds_actions_takes_the_same_steps() {
    if !shipped() {
        return;
    }
    let [first_plain, later_plain] = steps_of_a_tool_round("1", false);
    clear_of_midnight();
    let mut t = talk("1");

    // A rebuild before the turn: its summary leaf stands, and what it says
    // to the next call waits. The handover leaf stands beside it.
    t.strike("- The person told items 1 to 4.");
    assert!(
        t.cover() > 0,
        "check of the setup: a rebuild before the turn: {:?}",
        t.h.stderr
    );
    let handover = lay_the_handover_leaf(&t);
    let summary = slot_hash(&t, &summary_slot(ROUND_E));
    assert!(
        summary.is_string(),
        "check of the setup: the summary leaf stands"
    );
    let waiting = t.h.state_in("actions_pending", ROUND_E);
    assert!(
        waiting.contains("rebuild"),
        "check of the setup: the actions of the rebuild wait for the first call: {waiting}"
    );

    let at = t.h.ledger_steps.len();
    let c0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    let first = t.h.ledger_steps[at..].to_vec();
    let kept = turn_cut_row(&t.h, ROUND_E, "s1", "t7");
    assert_eq!(
        kept["leaves"]["history.summary"][0], summary,
        "the turn keeps the summary leaf of its first call: {kept}"
    );
    assert_eq!(
        kept["leaves"]["history.handover"][0],
        json!(handover),
        "the turn keeps the handover leaf of its first call: {kept}"
    );
    assert_eq!(
        t.h.state_in("actions_pending", ROUND_E),
        "[]",
        "the first call of the turn took the actions that waited"
    );
    t.answer(&c0, "tool_calls", json!([tool_call(ID, "read_file")]));

    // A rebuild in the middle of the turn: another summary leaf, the
    // handover leaf taken down, actions waiting again.
    t.strike("- The person told items 1 to 5.");
    assert_ne!(
        slot_hash(&t, &summary_slot(ROUND_E)),
        summary,
        "check of the setup: the rebuild replaced the summary leaf: {:?}",
        t.h.stderr
    );
    assert_eq!(
        slot_hash(&t, "history.handover"),
        Value::Null,
        "check of the setup: the rebuild took the handover leaf down: {:?}",
        t.h.stderr
    );
    let waiting = t.h.state_in("actions_pending", ROUND_E);
    assert!(
        waiting.contains("rebuild"),
        "check of the setup: the actions of the rebuild wait: {waiting}"
    );

    let at = t.h.ledger_steps.len();
    t.call(
        "t7",
        1,
        json!([tool_call(ID, "read_file"), tool_result(ID, "file 1: abc")]),
    );
    let later = t.h.ledger_steps[at..].to_vec();
    assert_eq!(
        first, first_plain,
        "the first call of the turn sends the store the messages of a first call"
    );
    assert_eq!(
        later, later_plain,
        "the later call of the turn sends the store the messages of a later call"
    );
    // What the steps bought: the later call begins with the whole window
    // of the first, system part and messages, and leaves the actions.
    let n = t.sent.len();
    assert_eq!(
        begins_with(&t.h, &t.sent[n - 2], &t.sent[n - 1]),
        Ok(()),
        "the later call is cut by what the turn keeps: {:?}",
        t.h.stderr
    );
    assert_eq!(
        t.h.state_in("actions_pending", ROUND_E),
        waiting,
        "the later call leaves the actions to the first call of the next turn"
    );
}
