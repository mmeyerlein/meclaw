//! The briefing carries the ENTRY SHAPE of a mutation, not only its vocabulary.
//!
//! Measured, not supposed. A hosted model walked S12 three times
//! (`plans/welle-2026-08-27/receipts/s12-luna-run.md`): it designed the right
//! topology every time — three cells plus the chain of edges the sentence asked
//! for — and every time the mutation door refused declaration 1 with `schema`:
//!
//! ```text
//! add_nodes[0]: add_nodes[].name missing
//! add_nodes[1]: add_nodes[].template missing
//! ```
//!
//! It addressed its cells with `path` and named the cell type with `kind` or
//! `type`, because nothing it was handed ever said what an entry looks like.
//! The briefing listed the diff KEYS that exist (`add_nodes`, `move_nodes`, …)
//! and stopped there — which is the vocabulary of the language without its
//! grammar, and a model fills a missing grammar with the shape it has seen most
//! often elsewhere.
//!
//! Two of the three drafts would also have grown an ISLAND had they parsed:
//! every edge stayed inside the new unit, and per GH #265 an edge between a
//! hive and its own child is internal and connects nothing. That constraint was
//! named nowhere in the prompt either.
//!
//! So the grammar lives in the briefing HEAD, not in the retrieved corpus: it
//! must survive a corpus outage, because a degraded briefing is exactly when a
//! model has the least to lean on. The corpus gained `docs/rewiring.md` in the
//! same pass (`workshop/tools/build_librarian_seed.py`) — that is the depth;
//! this is the floor.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

fn run_brief(hop: Value, messages: Value) -> Value {
    emit_one(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": hop, "context": {}},
            "ttl": 64,
            "messages": messages,
        }),
    )
}

fn instructions_of(out: &Value) -> String {
    out["system"]["instructions"]["text"]
        .as_str()
        .expect("system.instructions.text — the shape builder-hive/brief ships")
        .to_string()
}

/// A briefing that DID retrieve patterns.
fn briefed() -> String {
    instructions_of(&run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 3}),
        json!([
            {"origin": "user", "type": "text", "id": "",
             "text": "a collector, an llm summarizer and a store, wired in a chain"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "### config.md -- required_drains (spec) [d-17]\na drain is …"}
        ]),
    ))
}

/// The same request with the corpus down.
fn degraded() -> String {
    instructions_of(&run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 0, "degraded": true}),
        json!([
            {"origin": "user", "type": "text", "id": "",
             "text": "a collector, an llm summarizer and a store, wired in a chain"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "(retrieval unavailable: query_timeout)"}
        ]),
    ))
}

#[test]
fn the_briefing_names_the_two_required_keys_of_an_add_nodes_entry() {
    let text = briefed();
    assert!(
        text.contains("\"name\"") && text.contains("\"template\""),
        "the two keys the door demands are the two the prompt never named"
    );
    assert!(
        text.contains("REQUIRED"),
        "naming a key is not the same as saying it may not be left out — the \
         refused drafts carried `template`-shaped information under other names"
    );
}

#[test]
fn the_briefing_rules_out_the_keys_the_refused_drafts_invented() {
    let text = briefed();
    for invented in ["\"path\"", "\"kind\"", "\"type\""] {
        assert!(
            text.contains(invented),
            "the prompt must say that {invented} is not a key of an add_nodes \
             entry; all three were observed in a refused draft"
        );
    }
}

#[test]
fn the_briefing_states_the_crossing_edge_rule() {
    let text = briefed();
    assert!(
        text.contains("add_edges") && text.contains("crosses"),
        "an add_nodes with no crossing edge grows a registered, wired, INACTIVE \
         subtree (GH #265) — a manifest that commits and delivers nothing"
    );
    assert!(
        text.contains("already exist") || text.contains("this manifest creates"),
        "one refused draft pointed its last edge at a node nobody had created; \
         the endpoint rule belongs in the prompt"
    );
}

#[test]
fn the_briefing_shows_one_worked_declaration() {
    let text = briefed();
    assert!(
        text.contains("\"scope\"") && text.contains("\"diff\"") && text.contains("\"add_edges\""),
        "a grammar without an example is a grammar a model has to guess the \
         nesting of"
    );
}

#[test]
fn the_briefing_says_how_a_templates_contract_is_met() {
    // The run AFTER the grammar block landed: every entry encoded correctly,
    // and the door refused one level higher with `requirement_missing` — the
    // model had named `cogny` and passed it an empty `ctx`. The prompt said
    // what an entry looks like and never that a template may demand keys.
    let text = briefed();
    assert!(
        text.contains("requirement_missing"),
        "the refusal a model must be able to avoid has to be nameable in the \
         prompt; it is the one the second S12 run died on"
    );
    assert!(
        text.contains("CONTRACT"),
        "the prompt must point at the surface the requirement is READABLE on — \
         the catalogue row's contract line — or the rule is unactionable"
    );
    assert!(
        text.contains("override_params"),
        "the plausible wrong channel has to be closed by name: a template's \
         `requires.ctx` is met by the declaration's own `ctx` block, never by \
         `override_params`, which sets a cell's params"
    );
    assert!(
        text.contains("\"ctx\": {\"model\""),
        "a rule about a block needs the block shown filled; the EMPTY ctx of \
         the other example is precisely the shape that was refused"
    );
}

#[test]
fn the_grammar_survives_a_corpus_outage() {
    // A degraded briefing is when the model has the LEAST to lean on. If the
    // entry shape rode in on the retrieved patterns it would vanish exactly
    // then — which is why it lives in the head and not in the corpus arm.
    let text = degraded();
    assert!(text.contains("\"name\"") && text.contains("\"template\""));
    assert!(text.contains("REQUIRED"));
    assert!(text.contains("crosses"));
    // The contract rule rides in the head for the same reason: with the corpus
    // down there is no catalogue row to read it off, and a model that does not
    // know a template can demand keys will not think to ask.
    assert!(text.contains("requirement_missing"));
}
