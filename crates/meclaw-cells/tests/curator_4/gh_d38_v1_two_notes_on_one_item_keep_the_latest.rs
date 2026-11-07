//! GH #1451 (D38 V1, T-3, OR-38-81) -- two notes on one item keep the latest.
//!
//! WHY this file exists: in the R039 acceptance (09:02:21) the companion
//! "corrected its earlier statements" unasked. Its window carried the answered
//! orga question (`hint:question`, 08:59) beside the newer notice
//! (`hint:notify`, 09:01) of the same orga item; the correction repeated the
//! notice almost word for word. A note of the `in_note` form may name its
//! `item`; with `notes_latest_per_item` "1" only the youngest note of one
//! `(source, item)` stands in the notes block (GC class "spent notes",
//! OR-38-13). A note without an item stays as before. Shipped "0".

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask};
use meclaw_core::serde_json::json;

const ASKED: &str = "due: Question on T-2 (T-2); kind: hint:question";
const DONE: &str = "due: T-2 was rescheduled to 18:00 (T-2); kind: hint:notify";
const OTHER: &str = "due: T-5 tomorrow (T-5); kind: hint:due";

fn note(h: &mut Hive, id: &str, item: &str, text: &str) {
    let mut n = json!({"id": id, "source": "orga", "text": text, "scope": "session"});
    if !item.is_empty() {
        n["item"] = json!(item);
    }
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [n]}),
    );
}

fn notes_text(knob: &str, item_a: &str) -> String {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "notes_latest_per_item", json!(knob)),
    ]);
    turn(&mut h, "s1", "t1", "Move it to six.", "Shall I?", json!({}));
    note(&mut h, "on-000000000001", item_a, ASKED);
    note(&mut h, "on-000000000002", item_a, DONE);
    note(&mut h, "on-000000000003", "it-5", OTHER);
    let c = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Elias moves.")]),
        mode("Be brief."),
    );
    texts(&c).last().cloned().unwrap_or_default()
}

/// Pin: the answered question goes, the notice of the same item stays.
#[test]
fn the_younger_note_of_an_item_replaces_the_older() {
    if !shipped() {
        return;
    }
    let text = notes_text("1", "it-2");
    assert!(text.contains(DONE), "the latest note stays: {text}");
    assert!(!text.contains(ASKED), "the spent question goes: {text}");
    assert!(text.contains(OTHER), "another item keeps its note: {text}");
}

/// Lock: notes without an item never collapse; off, the block of before.
#[test]
fn without_an_item_or_off_every_note_stays() {
    if !shipped() {
        return;
    }
    for (knob, item) in [("1", ""), ("0", "it-2")] {
        let text = notes_text(knob, item);
        assert!(
            text.contains(DONE) && text.contains(ASKED),
            "{knob}/{item}: {text}"
        );
    }
}

/// Pin (intake half, Bauer 2): `in_note` keeps the optional `item` in its row,
/// in the bounds of an id; a note without one is the row of before, without
/// the column.
#[test]
fn the_door_keeps_the_item_of_a_note() {
    if !shipped() {
        return;
    }
    let probe = "(globals().__setitem__('AUDIENCE', '[\"member:m\"]'), \
                 [note_of(n, 'T', 's') for n in ARGS])[1]";
    let base = json!({"id": "on-1", "source": "orga", "text": "Call the doctor.",
                      "scope": "session"});
    let mut with = base.clone();
    with["item"] = json!("og-000000000001");
    let mut long = base.clone();
    long["item"] = json!("x".repeat(65));
    let (got, err) = cell_scope("intake", json!({}), probe, json!([with, base, long]));
    assert_eq!(got[0][0]["item"], json!("og-000000000001"), "{got} {err}");
    assert!(
        got[1][0].get("item").is_none(),
        "without an item, the row of before: {got}"
    );
    assert!(
        got[2][0].is_null(),
        "an item over 64 characters is refused: {got}"
    );
}

/// Pin (review o15-Rv1 m2): the same knob in the seed a call starts with --
/// `./history` carries only the youngest note of one item; off, both.
#[test]
fn the_seed_keeps_the_latest_note_of_an_item() {
    if !shipped() {
        return;
    }
    let seed_notes = |knob: &str| -> String {
        let mut h = Hive::new();
        turn(&mut h, "s1", "t1", "Move it to six.", "Shall I?", json!({}));
        note(&mut h, "on-000000000001", "it-2", ASKED);
        note(&mut h, "on-000000000002", "it-2", DONE);
        note(&mut h, "on-000000000003", "it-5", OTHER);
        let p = history_params(&[("notes_latest_per_item", json!(knob))], &["now_utc"]);
        let seed = only(&seed_ask(&h, &p, "call-2"), "seed");
        seed.messages()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .find(|t| t.starts_with("Notes: "))
            .unwrap_or("")
            .to_string()
    };
    let on = seed_notes("1");
    assert!(on.contains(DONE) && on.contains(OTHER), "{on}");
    assert!(
        !on.contains(ASKED),
        "the spent question leaves the seed: {on}"
    );
    let off = seed_notes("0");
    assert!(
        off.contains(DONE) && off.contains(ASKED),
        "off, the seed of before: {off}"
    );
}
