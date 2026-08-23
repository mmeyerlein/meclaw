//! 0.2.0 P1 -- the extraction lane mints CANONICAL predicates (rulings Q1, Q2, Q4).
//!
//! The measured defect: every turn opened its own axis. `favorite editor`,
//! `Lieblingseditor` and `favorite_editor` are three axes for one relation, so the
//! version chain never fires (5 percent chain-fire rate) and a knowledge update
//! reads as two unrelated facts.
//!
//! Two guarantees are pinned here. Both used to be read out of the batched
//! extractor's rendered prompt; per-turn extraction (GitHub #298) retired that
//! prompt, and wave 5 task 9 re-pointed them at the surface that carries them
//! now -- the SHIPPED contract block, `templates/memory-hive/inline-contract.md`.
//! The party that mints the facts is the front model, and what it is handed is
//! that block, not a prompt a cell writes.
//!
//! 1. The block carries the curated core list WITH its cardinality split, byte
//!    for byte the one in `predicate-core.json` -- that file is the authority and
//!    this is its drift lock (a persona cannot import a JSON file at prompt time).
//! 2. The block carries the entity-fidelity rule: predicates are translated into
//!    canonical English, subjects, objects, values and proper names never are.
//!
//! A third guarantee died with the batch lane rather than moving: the lane used to
//! READ the axes this memory already carried and render them into the prompt it
//! built. There is no prompt left to render them into, so the vocabulary read and
//! its four cases were deleted with the mechanism (wave 5 task 7) instead of being
//! re-pointed at nothing.
//!
//! The shape half of the contract -- the obligation, both parts, the forms the
//! ingress parses, the length bound -- is
//! `crates/meclaw-cells/tests/gh299_the_contract_asks_for_both_parts.rs`. This
//! file keeps the two P1 guarantees, which is where they were pinned before the
//! surface moved.

const CORE_LIST: &str = "../../templates/memory-hive/predicate-core.json";
const INLINE_CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";

fn core_list() -> serde_json::Value {
    let raw = std::fs::read_to_string(CORE_LIST).expect("predicate-core.json");
    serde_json::from_str(&raw).expect("core list json")
}

/// Predicates of one cardinality group, as the authority file declares them.
fn core_group(kind: &str) -> Vec<String> {
    let list = core_list();
    let mut out: Vec<String> = list["predicates"]
        .as_array()
        .expect("predicates array")
        .iter()
        .filter(|p| p["cardinality"] == kind)
        .map(|p| p["predicate"].as_str().expect("predicate").to_string())
        .collect();
    out.sort();
    out
}

/// The block a persona actually carries: the fenced `text` section of the shipped
/// contract. Prose ABOUT a rule is not the rule, so both assertions below read the
/// block and never the page around it.
fn contract_block() -> String {
    let raw = std::fs::read_to_string(INLINE_CONTRACT).unwrap_or_else(|e| {
        panic!(
            "the hive ships no inline extraction contract ({INLINE_CONTRACT}): {e}. \
             Since GitHub #298 it is the ONLY thing the extracting model is told."
        )
    });
    let (_, tail) = raw
        .split_once("```text\n")
        .expect("the contract file carries the persona block in a ```text fence");
    let (block, _) = tail
        .split_once("\n```")
        .expect("the persona block's fence is closed");
    block.to_string()
}

/// Every run of whitespace collapsed to one space -- the block is wrapped to a
/// column, and a rule that had to survive a re-wrap intact would be a rule nobody
/// dares reformat.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The predicates of one cardinality group, as the BLOCK lists them.
///
/// The list runs on past its own line, so it is read as a token run rather than
/// as a line: everything after the group's colon that still looks like a
/// predicate key, up to and including the token the next sentence is glued to.
fn block_group(kind: &str) -> Vec<String> {
    let flat = flat(&contract_block());
    let marker = format!("{kind} (");
    let at = flat
        .find(&marker)
        .unwrap_or_else(|| panic!("the block names no {kind:?} group:\n{flat}"));
    let (_, tail) = flat[at..]
        .split_once("): ")
        .unwrap_or_else(|| panic!("the {kind:?} group opens no list:\n{flat}"));
    let mut out = Vec::new();
    for token in tail.split(',') {
        let token = token.trim();
        let key: String = token
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if key.is_empty() {
            break;
        }
        let ended = key.len() != token.len();
        out.push(key);
        if ended {
            // The last predicate of the group carries the next sentence behind it.
            break;
        }
    }
    out.sort();
    out
}

#[test]
fn the_contract_carries_the_core_list_split_by_cardinality() {
    // The drift lock: `predicate-core.json` is the authority, the literal in the
    // shipped block is its copy. A persona cannot import at prompt time, so this
    // comparison is the only thing that keeps the two from diverging silently --
    // and the cardinality split is what P2 derives the chain rules from (ruling
    // Q4): `single` replaces, `multi` enumerates, and a relation on the wrong side
    // of that line either loses values or keeps dead ones.
    assert_eq!(
        block_group("single"),
        core_group("single"),
        "the contract's single-valued group drifted from predicate-core.json"
    );
    assert_eq!(
        block_group("multi"),
        core_group("multi"),
        "the contract's multivalued group drifted from predicate-core.json"
    );
    assert!(
        contract_block().contains("snake_case"),
        "the style rule itself has to be in the contract, not only its examples"
    );
}

#[test]
fn the_contract_forbids_translating_entities() {
    // Q2 entity-fidelity rule: the small closed class of relation patterns becomes
    // canonical English, everything a turn NAMES stays byte-faithful. A model that
    // "corrects" an unfamiliar village into a familiar one destroys the fact, and
    // no later pass can tell that it happened -- the corrected name is a perfectly
    // plausible one.
    let block = flat(&contract_block());
    assert!(
        block.contains(
            "ENTITIES ARE VERBATIM: names and values are copied byte for byte, \
             never translated or corrected."
        ),
        "the entity-fidelity rule left the shipped contract:\n{block}"
    );
}
