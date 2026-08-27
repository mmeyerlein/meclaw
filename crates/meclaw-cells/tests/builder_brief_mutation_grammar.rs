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

/// The WHOLE body of a briefing that DID retrieve patterns — `briefed()` reads
/// its instructions off exactly this emission, so the two can never disagree.
fn briefed_body() -> Value {
    run_brief(
        json!({"route": "brief", "stage": "briefed", "hits": 3}),
        json!([
            {"origin": "user", "type": "text", "id": "",
             "text": "a collector, an llm summarizer and a store, wired in a chain"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "### config.md -- required_drains (spec) [d-17]\na drain is …"}
        ]),
    )
}

/// A briefing that DID retrieve patterns.
fn briefed() -> String {
    instructions_of(&briefed_body())
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

// ============================================================ THE ENDPOINT SET
//
// Measured on 2026-08-27, 3 of 4 runs
// (`plans/welle-2026-08-27/receipts/builder-agentic-loop-messlauf.md` § 1): every
// draft wrote
//
// ```json
// {"scope": "/os/orgs/acme/apps", "diff": {"add_edges": [{"from": ".", "to": "./research"}]}}
// ```
//
// and the door answered `edge_endpoints/edge_schema .: from='.' unknown`. The
// prompt and the door contradicted each other: `CONNECTIVITY` demanded an edge
// that crosses the new unit's boundary "in the same manifest", and the only
// anchor a declaration names for "outside" is its own scope. But the scope is
// not an endpoint.
//
// The rule, read off `crates/meclaw-colony/src/mutation/validate.rs`:
//   * `scoped_name` (Z. 227) strips a leading `./` and then splits on `/`. `"."`
//     survives as the short name `.`, and no cell or hive is ever called `.` --
//     so `known(".")` is false, always. There is no special case anywhere.
//   * `validate_scope_containment` (Z. 2186) refuses any endpoint starting with
//     `/` or containing a `..` segment BEFORE membership is even looked at, with
//     `scope_out_of_bounds` -- an absolute path is refused even when it points
//     inside the scope. The single exemption is the `to` arm of
//     `/colony/graph|registry|ledger`.
//   * What IS legal: `./x` (a direct child of the scope, or any hive) and
//     `./x/y` (deeper under it), including a node this same diff creates.
//
// So a declaration cannot draw the crossing edge from inside the unit it is
// growing. It has to be scoped one level UP, name the container as `./c` and the
// new cell as `./c/name` -- which is exactly what every shipped example does
// (`examples/organism/grow-org.json` and its four siblings).

#[test]
fn the_briefing_rules_out_the_endpoint_the_door_refuses() {
    let text = briefed();
    assert!(
        text.contains("edge_schema"),
        "the refusal a model must be able to avoid has to be nameable in the \
         prompt -- this is the one 3 of 4 acceptance runs died on"
    );
    assert!(
        text.contains("\".\""),
        "the prompt must say that \".\" -- the scope itself -- is not an edge \
         endpoint; the door has no special case for it and every refused draft \
         used it"
    );
    assert!(
        text.contains("scope_out_of_bounds"),
        "the obvious second guess after \".\" is an absolute path, and that is \
         refused one stage EARLIER, with its own code"
    );
}

#[test]
fn no_example_in_the_briefing_draws_an_edge_from_the_scope() {
    for text in [briefed(), degraded()] {
        assert!(
            !text.contains("\"from\": \".\""),
            "a prompt that rules out an endpoint and then shows it is a prompt \
             that shows it"
        );
    }
}

#[test]
fn the_briefing_shows_how_to_reach_into_a_unit_it_is_growing() {
    let text = briefed();
    assert!(
        text.contains("\"name\": \"c/"),
        "the legal way to place a cell inside a container is a DEEP name in \
         add_nodes, declared at the container's parent -- shown, not described"
    );
    assert!(
        text.contains("\"./c/") && text.contains("\"./c\""),
        "and the two endpoint spellings that go with it: `./c` for the \
         container, `./c/<name>` for what this diff puts in it"
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
    // Same argument for the endpoint set: the corpus cannot be the place a
    // model learns which spellings the door accepts.
    assert!(text.contains("edge_schema") && text.contains("scope_out_of_bounds"));
    assert!(text.contains("\".\""));
}

/// The briefing is a prompt SEEDER now: the corpus is a tool the model may call,
/// so the four tool schemas travel in `system.tools.*` -- separately extracted,
/// never concatenated into the system prompt (`docs/cell-types.md` § llm).
#[test]
fn the_briefing_seeds_four_tools_and_no_others() {
    let out = briefed_body();
    let tools = out["system"]["tools"]
        .as_object()
        .expect("system.tools declared");
    let mut names: Vec<&String> = tools.keys().collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "catalogue_lookup",
            "graph_read",
            "librarian_search",
            "registry_read"
        ],
        "the vocabulary is CLOSED: four eyes, no hand, nothing else"
    );
    for (name, slot) in tools {
        let text = slot["text"].as_str().expect("each tool is one text leaf");
        let fn_obj: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(text).expect("a stringified function object");
        assert_eq!(fn_obj["type"], "function");
        assert_eq!(fn_obj["function"]["name"].as_str(), Some(name.as_str()));
    }
}

/// The retrieval guarantee moved. It used to be an instruction in the corpus
/// arm; agentically that arm is a tool RESULT the model may overrule, so the
/// duty rides in the HEAD -- the same argument that put GRAMMAR there.
#[test]
fn the_head_makes_retrieval_a_duty_rather_than_an_offer() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("catalogue_lookup"),
            "the head names the tool that publishes a template's contract"
        );
        assert!(
            text.contains("before") || text.contains("first"),
            "a model that writes without looking is the new failure class this \
             sentence exists to prevent"
        );
    }
}
