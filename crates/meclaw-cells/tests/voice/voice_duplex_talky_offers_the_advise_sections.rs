//! Welle Live (GH #783) — `talky` offers the three sidecar sections a voice
//! model is advised through: `fact`, `context` and `correction`.
//!
//! WHY A CELL AND NOT A LITERAL. The splitter is section-agnostic
//! (`templates/talky/splitter/config.json`) and the collector merges the
//! `sidecar[]` offers of everyone who answers its menu question into one block
//! contract (`collector/assemble`, GH #606). Whoever is REACHED declares
//! themselves — so the sections this composite asks its own model for are
//! declared by a cell inside the composite, the same shape
//! `templates/memory-hive/schemas/config.json` has, and cost no line in the
//! splitter, in the collector or in `talky`'s own contract.
//!
//! WHY THEY ARE OPTIONAL. A duplex call is the only occasion on which any of
//! them applies: the voice model speaks, `talky` advises it. On every other
//! turn the three sections stay empty, which is why each offer says so in its
//! own words (`in advise mode only`) instead of being switched on and off by a
//! second mechanism nothing could read from the block.
//!
//! The form is `collector_window.rs`: the SHIPPED `params.script_inline` runs
//! under `python3` over a real stdin document, so what is measured here is what
//! ships and nothing is mocked.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};

const TALKY_SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/talky/schemas/config.json"
);

/// The three sections, in the order the cell offers them: what the caller
/// hears, what the model knows, what the model obeys from here on.
const SECTIONS: [&str; 3] = ["fact", "context", "correction"];

/// The shipped answer of `talky/schemas` to one menu question.
///
/// `tools: []` is the request a collector makes that declares no tool name at
/// all — this cell serves none, and its offer is not a response to a name.
fn answer() -> Value {
    let doc = code_stdin(&json!({
        "target": "/main/agent/schemas",
        "header": {"hop": {"route": "in_menu"}, "context": {}},
        "ttl": 64,
        "tools": [],
        "params": {},
    }));
    let out = run_shipped_script(&shipped_script(TALKY_SCHEMAS), &doc.to_string());
    assert!(
        out.status.success(),
        "talky/schemas exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the answer is not one JSON document ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn talky_offers_the_three_advise_sections_and_declares_no_tool() {
    let answer = answer();

    assert_eq!(
        answer["header"]["operation"].as_str(),
        Some("schemas"),
        "the answer travels the menu lane and says so on its hop: {answer}"
    );
    assert_eq!(
        answer["schemas"].as_array().map(Vec::len),
        Some(0),
        "this cell declares NO tool — the tools of a talky are the parent's, \
         and a name declared here would be a name nothing answers: {answer}"
    );

    let offers = answer["sidecar"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer carries a `sidecar` list: {answer}"));
    let named: Vec<&str> = offers
        .iter()
        .map(|o| o["section"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        named, SECTIONS,
        "the three sections, in this order: what the caller HEARS, what the \
         model KNOWS, what it obeys for the REST of the call"
    );

    for offer in offers {
        let section = offer["section"].as_str().unwrap_or_default();
        assert_eq!(
            offer["required"].as_bool(),
            Some(false),
            "`{section}` is optional: outside a duplex call it is empty on \
             every turn, and a required section that is empty on every turn is \
             a contract a model learns to break"
        );
        let instruction = offer["instruction"].as_str().unwrap_or_default();
        assert!(
            instruction.contains("advise mode"),
            "`{section}` says in its own words WHEN it applies — the emptiness \
             outside advise mode is stated in the offer, not enforced by a \
             second mechanism:\n  {instruction}"
        );
    }
}
