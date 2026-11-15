//! GH #1517 -- a slot the curator writes itself that arrives or changes
//! between two calls keeps the prefix.
//!
//! WHY this file exists: in a run without a model the second call of a
//! freshly reset session broke the prefix without a rebuild (two sessions,
//! call 2 `prefix_break` 1). The colony's message log showed the cause: the
//! slot `instructions.hygiene`, which `./push` writes in the bundle of the
//! first turn (blocks + slots, owner curator), came home between the first and
//! the second call; the first call went out without it, the second carried it
//! in its system part (every other leaf of the two calls the same). The slot
//! can change again later (a second set of hygiene words), and a changed slot
//! breaks the prefix the same way. A slot that arrives or changes after the target's
//! window was first cut on a state of the plan now takes effect with the next
//! change of plan -- the one place a prefix may change -- and orders it (mark
//! `catalog`, no model asked), as a catalogue row does since GH #1497.
//!
//! Only a slot whose row names the curator as its `owner` is held back so: a
//! slot of any other owner (a pack, a collector) is a contract over content
//! and stands in the very next call as before -- the line between the two
//! runs on the column `owner`, never on a slot's name.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The slot `./push` writes once (`HYGIENE`), and a text in its form.
const SLOT: &str = "instructions.hygiene";
const SLOT_TEXT: &str = "If you are not sure what was said, ask for the block by its id.";

/// The ledger rows `./push` books for its slot: the block of the leaf and
/// the slot row naming it, owner curator.
fn push_writes_the_slot(h: &Hive) {
    writes_the_slot(h, "curator");
}

/// The block of the leaf and the slot row naming it, booked for `owner`.
fn writes_the_slot(h: &Hive, owner: &str) {
    let leaf = json!({"path": SLOT, "text": SLOT_TEXT});
    let body = canonical(&leaf);
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'system', ?2, ?3, 'x')",
        rusqlite::params![hash, SLOT_TEXT.len() as i64, body],
    )
    .expect("the leaf's block");
    h.db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, ?3, 'x')",
        [SLOT, hash.as_str(), owner],
    )
    .expect("the leaf's slot");
}

/// Whether the system tree the model holds for `call` (its snapshot row)
/// holds the slot.
fn slot_sent(h: &Hive, call: &Msg) -> bool {
    sent_of(h, call)
        .map(|s| sent_window(h, &s).leaves)
        .unwrap_or_default()
        .iter()
        .any(|(path, _)| path == SLOT)
}

/// The figure `kind` of the hive's own `stats` door over the last day and
/// the next.
fn figure(t: &mut Talk, kind: &str) -> Value {
    let now = chrono::Utc::now();
    let day = chrono::Duration::days(1);
    t.h.out.clear();
    t.h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "prefix:1",
               "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_kinds": format!("[\"{kind}\"]")}),
        json!({"messages": []}),
    );
    assert_eq!(t.h.out.len(), 1, "one answer: {:?}", t.h.out);
    let m = &t.h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", t.h.stderr);
    m.body["stats"]["counts"][kind].clone()
}

/// A conversation on the form whose first turn goes out without the slot;
/// `between` runs after it, then the second turn -- no rebuild between them.
fn two_turns(between: impl Fn(&Hive)) -> (Talk, Msg, Msg) {
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    assert!(
        t.h.rows(&format!("SELECT hash FROM slots WHERE path = '{SLOT}'"))
            .is_empty()
            && !slot_sent(&t.h, &c1),
        "check of the setup: the first call went out before the slot"
    );
    between(&t.h);
    let c2 = t.turn("t2", "The paper of yesterday lies on the table.", "I see.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    (t, c1, c2)
}

/// Pin: the slot that came home between the two calls is no part of the
/// second window -- the system part is the same tree, the watch counts no
/// prefix break -- and the second call orders the change of plan that takes
/// it on; after that change the slot stands in the window, and the watch
/// still counts no break.
#[test]
fn gh_d58_prefix_a_slot_that_arrives_between_two_calls_keeps_the_prefix() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (mut t, _, c2) = two_turns(push_writes_the_slot);
    assert!(
        t.breaks().is_empty(),
        "the second call begins with the window of the first: {:?}",
        t.breaks()
    );
    assert!(
        !slot_sent(&t.h, &c2),
        "the system part holds no slot the first call did not hold"
    );
    let order =
        t.h.clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("an order at the clock: {:?}", t.h.clock));
    assert_eq!(
        order.body["emit_body"]["reason"],
        json!("catalog"),
        "the arrived slot orders the change of plan: {:?}",
        order.body
    );
    t.h.fire(&order);
    assert!(
        t.h.summ.is_empty(),
        "the change of plan only cuts anew -- no summarizer is asked: {:?}",
        t.h.summ
    );
    let c3 = t.turn("t3", "And now?", "Now this.");
    assert_ne!(
        cut_epoch(&t.h, &c3),
        cut_epoch(&t.h, &c2),
        "the next turn is cut on the new state of the plan"
    );
    assert!(
        slot_sent(&t.h, &c3),
        "after the change of plan the slot stands in the system part"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["n"], json!(3), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "the change of plan is the one place the prefix may change: {w}"
    );
    let r = figure(&mut t, "rebuild");
    assert_eq!(
        r["by_mark"]["catalog"],
        json!(1),
        "the order counts as a rebuild: {r}"
    );
}

/// Lock: without a slot that arrives the window stands as before -- the
/// same system part, no break, no change of plan ordered.
#[test]
fn without_a_new_slot_nothing_is_ordered() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (mut t, _, c2) = two_turns(|_| {});
    assert!(t.breaks().is_empty(), "{:?}", t.breaks());
    assert!(!slot_sent(&t.h, &c2));
    let reason =
        t.h.clock_order_for(ROUND_E)
            .map(|o| o.body["emit_body"]["reason"].clone());
    assert_ne!(
        reason,
        Some(json!("catalog")),
        "no slot moved, no change of plan for it: {reason:?}"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}

/// Counter-pin: the same leaf under the same path, booked by another owner
/// (a pack), is a contract over content -- it stands in the very next call,
/// as before the hold, and orders no change of plan.
#[test]
fn a_slot_of_another_owner_stands_in_the_next_call() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (t, _, c2) = two_turns(|h| writes_the_slot(h, "pack"));
    assert!(
        slot_sent(&t.h, &c2),
        "a slot the curator does not own reaches the next call"
    );
    let reason =
        t.h.clock_order_for(ROUND_E)
            .map(|o| o.body["emit_body"]["reason"].clone());
    assert_ne!(
        reason,
        Some(json!("catalog")),
        "a slot of another owner orders no change of plan: {reason:?}"
    );
}

/// The leaf's text `text` under `SLOT`, booked by the curator: its block and
/// the slot row naming it (one row per path, so a change replaces the row).
fn sets_the_slot(h: &Hive, text: &str) {
    let leaf = json!({"path": SLOT, "text": text});
    let body = canonical(&leaf);
    let hash = sha256_hex(&body);
    h.db.execute(
        "INSERT OR IGNORE INTO blocks (hash, kind, chars, body, first_seen) \
         VALUES (?1, 'system', ?2, ?3, 'x')",
        rusqlite::params![hash, text.len() as i64, body],
    )
    .expect("the leaf's block");
    h.db.execute("DELETE FROM slots WHERE path = ?1", [SLOT])
        .expect("the old slot row");
    h.db.execute(
        "INSERT INTO slots (path, hash, owner, at) VALUES (?1, ?2, 'curator', 'x')",
        [SLOT, hash.as_str()],
    )
    .expect("the leaf's slot");
}

/// The text of the slot in the system tree the model holds for `call`.
fn slot_text(h: &Hive, call: &Msg) -> Option<String> {
    sent_of(h, call)
        .map(|s| sent_window(h, &s).leaves)
        .unwrap_or_default()
        .into_iter()
        .find(|(path, _)| path == SLOT)
        .map(|(_, text)| text)
}

const CHANGED_TEXT: &str = "If you are unsure what was said, fetch the block by its short id.";

/// Pin (a CHANGED slot): the slot stood in the first call; it changes between
/// the first and the second call. The second call still sends the text it was
/// first cut with -- the prefix stays byte for byte -- and orders the change
/// of plan; after that change the new text stands, and the watch counts no
/// prefix break. The old block is no slot row any more (one row per path),
/// so the old text must come from what the first cut kept, not from the
/// read of today's slots.
#[test]
fn a_slot_of_the_curator_that_changes_between_two_calls_keeps_the_prefix() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    sets_the_slot(&t.h, SLOT_TEXT);
    let c1 = t.turn("t1", "Good morning.", "Good morning to you.");
    assert_eq!(
        slot_text(&t.h, &c1).as_deref(),
        Some(SLOT_TEXT),
        "check of the setup: the first call carries the slot"
    );
    sets_the_slot(&t.h, CHANGED_TEXT);
    let c2 = t.turn("t2", "The paper of yesterday lies on the table.", "I see.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    assert!(
        t.breaks().is_empty(),
        "the second call begins with the window of the first: {:?}",
        t.breaks()
    );
    assert_eq!(
        slot_text(&t.h, &c2).as_deref(),
        Some(SLOT_TEXT),
        "the second call sends the text the first was cut with"
    );
    let order =
        t.h.clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("an order at the clock: {:?}", t.h.clock));
    assert_eq!(
        order.body["emit_body"]["reason"],
        json!("catalog"),
        "the changed slot orders the change of plan: {:?}",
        order.body
    );
    t.h.fire(&order);
    let c3 = t.turn("t3", "And now?", "Now this.");
    assert_ne!(
        cut_epoch(&t.h, &c3),
        cut_epoch(&t.h, &c2),
        "the next turn is cut on the new state of the plan"
    );
    assert_eq!(
        slot_text(&t.h, &c3).as_deref(),
        Some(CHANGED_TEXT),
        "after the change of plan the new text stands"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["n"], json!(3), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "the change of plan is the one place the prefix may change: {w}"
    );
}
