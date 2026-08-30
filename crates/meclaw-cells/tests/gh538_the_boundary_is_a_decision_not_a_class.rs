//! GH #538 — the class boundary has to be decidable, not recognisable.
//!
//! An assistant level has two occupants: a conversation surface that answers
//! fast and a reasoning core it can consult. Which questions cross that
//! boundary is stated in exactly ONE place — the `description` of the
//! `consult_cogny` schema, handed out by `templates/cogny/declare` (GH #528),
//! whose own prose calls it *the class boundary*.
//!
//! Measured end to end on a live generation, twice, with the same question
//! (plan three days in a city on a budget, compare two variants with numbers):
//! the first run consulted the core, the second — same chat, the finished plan
//! now standing in the window — ran the surface's own `web_search` and did the
//! work itself. Same agent, same menu, opposite behaviour.
//!
//! The boundary was not wrong; it was not DECIDABLE. It named a kind of work
//! ("synthesis over several sources, a development over time, a multi-step piece
//! of work"), and a reader that has just watched the work being done classifies
//! it as done-able. In-context learning beats a category.
//!
//! So this file judges the SHAPE of the description, off the shipped cell:
//!
//! 1. it puts the boundary as a QUESTION the caller answers by counting, rather
//!    than as a class the caller has to recognise;
//! 2. it says what the caller's OWN lookup tool is for — a boundary that only
//!    describes the far side leaves the near side to taste;
//! 3. it names the window, which is the thing that moved the boundary.
//!
//! # The test of the test
//!
//! The judgement is a function over one string, so it is run twice: once over
//! the shipped description, and once over the description as it stood before
//! this issue — which must FAIL every one of the three. Without that, a check
//! that never says no would pass whatever the file said.

use meclaw_core::serde_json::json;
use meclaw_testing::{emit_all, shipped_script};

const DECLARE_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/cogny/declare/config.json"
);

/// The description as the core hands it out — asked the way a caller asks.
fn shipped_description() -> String {
    let out = emit_all(
        &shipped_script(DECLARE_CELL),
        &json!({
            "target": "/main/cogny/declare",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": ["consult_cogny"],
            "messages": [],
        }),
    );
    let schemas = out
        .first()
        .and_then(|a| a["schemas"].as_array().cloned())
        .unwrap_or_default();
    assert_eq!(schemas.len(), 1, "one errand, one schema: {out:?}");
    assert_eq!(schemas[0]["name"], "consult_cogny");
    schemas[0]["description"]
        .as_str()
        .expect("a declaration carries a description")
        .to_string()
}

/// The three properties, each as its own verdict so a failure names which half
/// of the boundary went missing.
fn verdicts(desc: &str) -> [(&'static str, bool); 3] {
    let lower = desc.to_lowercase();
    [
        (
            "asks a question the caller answers by counting",
            desc.contains('?') && lower.contains("more than one") && lower.contains("comparison"),
        ),
        (
            "says what the caller's own lookup tool is for",
            lower.contains("web_search") && lower.contains("exactly one"),
        ),
        (
            "names the window, so an earlier own attempt is not a precedent",
            lower.contains("window") && lower.contains("precedent"),
        ),
    ]
}

#[test]
fn the_shipped_declaration_states_a_decision() {
    let desc = shipped_description();
    for (what, ok) in verdicts(&desc) {
        assert!(ok, "the declaration no longer {what}:\n{desc}");
    }
}

#[test]
fn the_description_before_this_issue_is_reported() {
    // Byte for byte what the cell handed out before #538 — a boundary stated as
    // a class. Every verdict must say no, or the check above proves nothing.
    const BEFORE: &str = "Consult the agent's reasoning core. It is the PROBLEM \
        SOLVER: send it synthesis over several sources, a development over time, \
        a multi-step piece of work, or anything that has to be researched or \
        worked out with tools. It thinks for as long as that takes and answers \
        as its own later turn, so say what you are doing in the same reply -- \
        that sentence reaches the person immediately. Do NOT send it a quick \
        fact: a question your own memory can answer is one you ask your own \
        memory.";
    for (what, ok) in verdicts(BEFORE) {
        assert!(!ok, "the old description already {what}: {BEFORE}");
    }
}
