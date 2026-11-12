//! D58 followup (GH #1479, contract points 3 and 4) -- an open concern the
//! person voiced reaches the next conversation once.
//!
//! WHY this file exists: in the R039 acceptance call 2 did not ask after the
//! person, who had said in call 1 that they felt unwell (D38 V2, GH #1451).
//! The curator kept that rule until this strand; now the hive `assistant`
//! keeps it. The whole road, on the manifests as they ship:
//! the person says it in a text turn of talky; the model's block carries the
//! fact in its section `memory` with the class `wellbeing` (memory-hive
//! schema, GH #1456); talky's curator forwards the section as `sidecar`
//! (`./intake`, registered section) and talky hands it out at its rim; the
//! edge of `assistant` that branches `sidecar` with `hop.section == 'memory'`
//! brings it to the cell `./followup`; the cell sends ONE note of scope
//! `once` with a `receipt` into every child with a curator (talky,
//! talky-chat, cogny) -- at once into talky-chat, which did not hear it, and
//! into talky, the hearing surface, and cogny, which the hearing session
//! consults, only when the hearing session ends (the `write` talky's curator
//! hands out at the close, OR-d58-2.followup.19, the re-check's I1,
//! OR-d58-2.followup.23, and OR-d58-2.followup.27); talky carries it to its curator
//! on `in_note`; the start
//! window of the next call shows it and answers `note_receipt`; the start
//! window after that shows nothing. The wiring is checked on the shipped
//! edges of `assistant` too: the branch beside every `sidecar` edge of a
//! child, an edge `in_note` into each child, an edge `note_receipt` out of
//! each child into the cell, an edge `write` out of each child with sessions
//! (talky, talky-chat) into the cell and an edge `in_note_held` back into
//! each of them and into cogny, and the cell's own store beside it.
//!
//! Red until the cell exists; behind it red until the note strand (GH #1477,
//! `note_once`) is in the tree.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const CLAIM: &str = "Ada feels unwell and slept badly.";

fn edge_text(e: &Value) -> String {
    format!("{} {}", e["condition"], e["modifier"])
}

#[test]
fn gh_d58_followup_an_open_concern_reaches_the_next_conversation_once() {
    if !shipped() {
        return;
    }
    // The wiring of `assistant`, as it ships.
    let edges = edges_of(&format!("{ASSISTANT}/config.json"));
    let config = child_config(CELL);
    assert_eq!(
        config["cell"]["type"],
        json!("code"),
        "{CELL} is a code cell"
    );
    assert_eq!(
        child_config(STORE)["cell"]["type"],
        json!("store"),
        "{STORE} is the cell's own store"
    );
    let has = |from: &str, to: &str, words: &[&str]| {
        edges.iter().any(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && words.iter().all(|w| edge_text(e).contains(w))
        })
    };
    let sidecar_children: Vec<String> = edges
        .iter()
        .filter(|e| e["to"] == json!(".") && edge_text(e).contains("'sidecar'"))
        .map(|e| e["from"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        ["./talky", "./talky-chat", "./cogny"]
            .iter()
            .all(|c| sidecar_children.iter().any(|s| s == c)),
        "check of the setup: the children hand `sidecar` out: {sidecar_children:?}"
    );
    for child in &sidecar_children {
        assert!(
            has(child, CELL_AT, &["'sidecar'", "hop.section", "'memory'"]),
            "an edge {child} -> {CELL_AT} branches the section memory"
        );
    }
    for child in COMPOSITES {
        assert!(
            has(CELL_AT, child, &["'in_note'"]),
            "an edge {CELL_AT} -> {child} carries in_note"
        );
        assert!(
            has(child, CELL_AT, &["'note_receipt'"]),
            "an edge {child} -> {CELL_AT} brings the receipt home"
        );
    }
    for child in ["./talky", "./talky-chat"] {
        assert!(
            has(child, CELL_AT, &["'write'"]),
            "an edge {child} -> {CELL_AT} brings the end of a session"
        );
        assert!(
            has(CELL_AT, child, &["'in_note_held'", "hop.channel_node"]),
            "an edge {CELL_AT} -> {child} carries the held note to the hearing surface"
        );
    }
    assert!(
        has(CELL_AT, "./cogny", &["'in_note_held'"]),
        "an edge {CELL_AT} -> ./cogny carries the held note to the reasoning core"
    );
    assert!(
        has(CELL_AT, "./cogny", &["'in_note'", "!has(hop.channel_node)"]),
        "an edge {CELL_AT} -> ./cogny carries what goes to every child (a withdrawal)"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == json!(CELL_AT) && e["to"] == json!(STORE_AT)),
        "the cell reaches its store"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == json!(STORE_AT) && e["to"] == json!(CELL_AT)),
        "the store answers the cell"
    );

    // The road.
    let mut a = Assistant::on_at(NOON);
    let mut talky = curator(&[]);
    let section = memory_from_curator(
        &mut talky,
        "./talky",
        "t1",
        json!([
            fact(
                "Ada",
                "moves_to",
                "Elias wants to move to Hamburg.",
                Some(0.9),
                false
            ),
            fact("Ada", "has_symptom", CLAIM, Some(0.9), true)
        ]),
    );
    a.leave("./talky", section);
    for child in ["./talky", "./cogny"] {
        assert!(
            a.notes_into(child).is_empty(),
            "held for {child} while the hearing session runs: {:?}",
            a.into
        );
    }
    assert_eq!(
        live_notes(&a.notes_into("./talky-chat")).len(),
        1,
        "at once into ./talky-chat, which did not hear it: {:?}",
        a.into
    );
    // The hearing session ends; now the note goes there too
    // (OR-d58-2.followup.19, .23).
    session_ends(&mut a, "./talky", "s1");
    assert!(a.emit_violations.is_empty(), "{:?}", a.emit_violations);
    let mut ids = Vec::new();
    for child in COMPOSITES {
        let notes = live_notes(&a.notes_into(child));
        assert_eq!(
            notes.len(),
            1,
            "one note into {child}, all told: {:?}",
            a.into
        );
        let n = &notes[0];
        assert_eq!(n["scope"], json!("once"), "{n}");
        assert!(
            n["receipt"].as_str().is_some_and(|r| !r.is_empty()),
            "a receipt: {n}"
        );
        assert!(
            n["text"].as_str().unwrap_or("").contains(CLAIM),
            "the note names the concern: {n}"
        );
        assert!(
            !n["text"].as_str().unwrap_or("").contains("Hamburg"),
            "an unclassed fact is no concern: {n}"
        );
        ids.push(n["id"].clone());
    }
    assert!(
        ids.windows(2).all(|w| w[0] == w[1]),
        "one note, one id, every curator: {ids:?}"
    );
    assert!(
        a.ops.iter().any(|o| o.is_object()),
        "the cell keeps the concern in its own store: {:?}",
        a.ops
    );

    assert_eq!(
        deliver_notes(&a, "./talky", &mut talky),
        1,
        "talky carries the note to its curator"
    );
    let s2 = seed(&mut talky, "call-2", "start", 0);
    assert_eq!(
        count_in(&s2, CLAIM),
        1,
        "the next call knows it, once: {:?}",
        s2.messages()
    );
    let receipts = talky.routed("note_receipt");
    assert_eq!(receipts.len(), 1, "one receipt: {:?}", talky.out);
    let s3 = seed(&mut talky, "call-3", "start", 0);
    assert_eq!(
        count_in(&s3, CLAIM),
        0,
        "the call after it does not: {:?}",
        s3.messages()
    );
}
