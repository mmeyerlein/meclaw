//! The briefing carries the ENTRY SHAPE of a mutation, not only its vocabulary.
//!
//! Measured, not supposed. A hosted model walked S12 three times
//! (the S12 build runs, `CHANGELOG.md` § 0.26.0): it designed the right
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
use meclaw_testing::{emit_all, emit_one, shipped_script};

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

/// The leg of the brief that reaches `./compose`. Since GH #477 the cell is a
/// multi-send: the store leg that parks the question and the instructions in
/// the round table travels first, the briefing itself second.
fn compose_leg(all: Vec<Value>) -> Value {
    all.into_iter()
        .find(|m| m["header"]["route"] == "compose")
        .expect("the brief's leg to the composer")
}

fn run_brief(hop: Value, messages: Value) -> Value {
    compose_leg(emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": hop, "context": {}},
            "ttl": 64,
            "messages": messages,
        }),
    ))
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
// (the builder agentic-loop run series, § 1): every
// draft wrote
//
// ```json
// {"scope": "/os/orgs/acme/apps", "diff": {"add_edges": [{"from": ".", "to": "./research"}]}}
// ```
//
// and the door answered `edge_endpoints/edge_schema .: from='.' unknown`. The
// prompt and the door contradicted each other: `CONNECTIVITY` demanded an edge
// that crosses the new unit's boundary "in the same manifest", and the only
// anchor a declaration names for "outside" is its own scope.
//
// GH #487 decided that contradiction the other way round: the DOOR was wrong,
// not the drafts. `.` names the declaration's own scope root — the reading
// `Path::resolve` and the boot path always had, and the spelling 277 of the
// shipped template edges use — and only `mutation::validate::scoped_name`
// classified it as a short name nothing is called. Since then the scope IS an
// endpoint, and since GH #503 it is the endpoint every level is grown through:
// a level declares itself AT its container and draws `.` ↔ `./<child>`.
//
// The rule as it now stands, read off `crates/meclaw-colony/src/mutation/validate.rs`:
//   * `scoped_name` resolves `.` and `./` to the scope itself (`ScopedName::Deep`).
//     A scope that is no node — `/` — still fails the membership test, so this
//     widened the vocabulary without inventing a node.
//   * `validate_scope_containment` refuses any endpoint starting with `/` or
//     containing a `..` segment BEFORE membership is even looked at, with
//     `scope_out_of_bounds` — an absolute path is refused even when it points
//     inside the scope. The single exemption is the `to` arm of
//     `/colony/graph|registry|ledger`.
//   * What IS legal: `.` (the scope root), `./x` (a direct child of the scope,
//     or any hive) and `./x/y` (deeper under it), including a node this same
//     diff creates.
//
// What survives of the old rule is the part that was never about `.`: a
// SIBLING of the scope is unreachable, because reaching one needs `..` or an
// absolute path. That is why the identity door — and only it — is still
// declared one storey up (see the path-depth test below).

#[test]
fn the_briefing_publishes_the_three_endpoint_spellings() {
    let text = briefed();
    assert!(
        text.contains("scope_out_of_bounds"),
        "the guess after a relative path is an absolute one, and that is \
         refused one stage EARLIER, with its own code"
    );
    assert!(
        text.contains("SCOPE ROOT") && text.contains("GH #487"),
        "`.` is an endpoint since GH #487 and is the one every level is grown \
         through since GH #503; a briefing that omits it teaches the wide form"
    );
    assert!(
        !text.contains("\".\" is NOT an endpoint"),
        "the briefing still carries the pre-#487 refusal, so it contradicts \
         both the door and every shipped grow-<level>.json"
    );
}

/// The half of the old endpoint rule that GH #487 did not touch: a sibling of
/// the scope needs `..` or an absolute path, and the door refuses both.
#[test]
fn the_briefing_still_rules_out_the_endpoint_the_door_refuses() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("edge_schema"),
            "the refusal a model must be able to avoid has to be nameable in \
             the prompt -- this is the one 3 of 4 acceptance runs died on"
        );
        assert!(
            text.contains("SIBLING") && text.contains("\"..\""),
            "the reason the identity door is declared one storey up is that a \
             sibling of the scope is unreachable; without it the exception \
             reads as a style choice"
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

// --- GH #466: four more blocks, four more measured refusals -----------------
//
// Each of the four tests below is a DRIFT LOCK in the sense of
// `docs/development-rules.md` § 2d: it greps the sentence AND asserts the
// mechanism the sentence describes. Grepping alone pins a string; asserting the
// mechanism alone lets the prose walk away from it.

use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

fn shipped_json(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// `override_params` was forbidden by the grammar and admitted by the door, and
/// the shape a model reached for when it used it anyway was FLAT. It is
/// addressed: the key is a cell path inside the template, `""` is the node.
#[test]
fn the_briefing_admits_override_params_in_its_addressed_form() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("OVERRIDES"),
            "the block that admits the key has to be in the head — an optional \
             key documented only in a corpus row is one a degraded run invents"
        );
        assert!(
            text.contains("override_params"),
            "the grammar still refuses to name the key it admits"
        );
        assert!(
            text.contains("cogny/brain"),
            "the addressed form is shown by example, because 'addressed' is the \
             word every flat draft would also have agreed with"
        );
    }
    // The mechanism: the path the example addresses is a REAL cell of a real
    // shipped template, one level down from the node. An example addressing
    // nothing would teach the shape and refuse on the tree.
    if repo("templates/assistant/cogny").is_dir() {
        assert!(
            repo("templates/cogny/brain/config.json").is_file(),
            "the briefing's addressed example names `cogny/brain`, and the \
             assistant's `cogny` ref resolves to a template whose `brain` cell \
             is exactly one segment down — if that stops being true the example \
             teaches a refusal"
        );
    }
}

/// `ref` was in no grammar at all, and it is where the catalogue's `CONTRACT —`
/// line gets its keys from: a composite declares no `requires` of its own and
/// demands them anyway, because its refs do.
#[test]
fn the_briefing_teaches_ref_as_a_declaration_form() {
    for text in [briefed(), degraded()] {
        assert!(text.contains("REFS"), "the block is missing from the head");
        assert!(
            text.contains("\"ref\"") || text.contains("ref\\\"") || text.contains("type\": \"ref"),
            "the grammar has to name the form, not gesture at it"
        );
        assert!(
            text.contains("UNION") || text.contains("union"),
            "the whole point of teaching refs here is where a composite's \
             required ctx keys come from"
        );
    }
    // The mechanism, on the tree: `assistant` does not declare `ctx.model`
    // anywhere in its own descriptor and is nevertheless refused without it —
    // because `cogny`, reached through a ref, declares one.
    //
    // GH #516: the level DOES carry a `requires` block since it grew a model key
    // of its own (`model_surface`, which no ref can own — it is the level that
    // decides which of its two brains gets which model). That does not weaken
    // the union argument, it sharpens it: a composite's demand is its own
    // declaration PLUS its refs', and `model` is still only ever written down in
    // `cogny`. Asking whether the block exists would measure the wrong thing.
    let (Some(assistant), Some(refd), Some(cogny)) = (
        shipped_json("templates/assistant/template.json"),
        shipped_json("templates/assistant/cogny/config.json"),
        shipped_json("templates/cogny/template.json"),
    ) else {
        return;
    };
    let own_ctx = assistant["requires"]["ctx"].as_object();
    assert!(
        own_ctx.is_none_or(|m| !m.contains_key("model")),
        "`model` is written down in `cogny` and nowhere else — the moment the \
         level restates it, the union is no longer the reason the door refuses, \
         and GH #292's own rule (restating IS the drift) is broken with it"
    );
    assert_eq!(refd["cell"]["type"], json!("ref"));
    assert!(
        refd["cell"]["template"]
            .as_str()
            .is_some_and(|t| t.contains('@')),
        "the grammar tells the model to pin a ref's version; the tree has to \
         mean it"
    );
    assert_eq!(cogny["requires"]["ctx"]["model"]["required"], json!(true));
}

/// Every example the grammar had was ONE segment deep, and a level is never one
/// segment deep. The address rule lived only in two template READMEs.
///
/// GH #503 moved every level into the NARROW form: the declaration stands in
/// the container, so a level's name is a BLANK name and not a path. The path
/// form survives for the one declaration the narrow form cannot express — the
/// identity door, which has to name `./affinity`, a sibling of the container.
/// The briefing has to teach both, and has to say which is which.
#[test]
fn the_briefing_carries_the_address_rule_and_an_example_two_segments_deep() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("context.assistant"),
            "the guard the whole addressing scheme turns on is not named"
        );
        assert!(
            text.to_lowercase().contains("static") && text.contains("Edge.to"),
            "the REASON is the rule: an edge target is static, so there is no \
             edge meaning 'send it wherever the context says'"
        );
        assert!(
            text.contains("sum, never the cross product"),
            "the consequence — N + M edges, not N x M — is the part a model \
             gets wrong when it only knows the guard"
        );
        // The narrow form is the default, and it is shown as a whole
        // declaration: scope IS the container, the name is bare.
        assert!(
            text.contains("NARROW form"),
            "the form a level is actually grown in is not named"
        );
        assert!(
            text.contains("\"scope\": \"/os/orgs/acme/members/alex/assistants\"")
                && text.contains("\"name\": \"scribe\"")
                && text.contains("\"to\": \"./scribe\""),
            "the worked example is not in the narrow form the fast lane renders"
        );
        // …and the exception is taught as an exception, with its reason.
        assert!(
            text.contains("assistants/scribe"),
            "the path form is gone, so a model drawing an identity door has no \
             shape to copy"
        );
        assert!(
            text.contains("./affinity") && text.contains("IDENTITY DOOR"),
            "the path form without its REASON is a second style to pick from, \
             which is exactly how the two forms get mixed"
        );
    }
    // The mechanism: the shipped example really is the narrow form, and really
    // does guard the way the briefing says.
    let Some(grown) = shipped_json("examples/organism/grow-assistant.json") else {
        return;
    };
    assert_eq!(
        grown["scope"],
        json!("/os/orgs/acme/members/alex/assistants"),
        "the level's declaration no longer stands in its container, so the \
         narrow form the briefing teaches is not the one the tree renders"
    );
    assert_eq!(
        grown["diff"]["add_nodes"][0]["name"],
        json!("scribe"),
        "a level's name is blank in the narrow form; a path here means the \
         briefing and the worked example disagree again"
    );
    let edges = grown["diff"]["add_edges"].as_array().expect("add_edges");
    assert!(
        edges.iter().all(|e| {
            // GH #562: a v-lane names its lane and ends on an occupant of the
            // child (`./scribe/talky`, `./scribe/cogny`) — still spelled from
            // inside the container, one segment deeper, and permitted by the
            // child template's own `at` rather than by this spelling.
            let deep = e.get("lane").and_then(|l| l.as_str()).is_some();
            let ok = |p: &str| p == "." || p == "./scribe" || (deep && p.starts_with("./scribe/"));
            ok(e["from"].as_str().unwrap_or_default()) && ok(e["to"].as_str().unwrap_or_default())
        }),
        "an endpoint in the shipped example is not spelled from inside the \
         container"
    );
    let guarded = edges
        .iter()
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("context.assistant"))
        })
        .count();
    assert!(
        guarded >= 2,
        "the example stopped guarding on context.assistant, so the rule the \
         briefing states is no longer the rule the tree follows"
    );
}

/// The most expensive kind of wrong: a manifest that validates, applies and
/// boots against an endpoint that does not exist. A measured run wrote its
/// `ctx.model` as an invented literal.
#[test]
fn the_briefing_makes_the_model_id_a_question_rather_than_a_guess() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("NOT YOURS TO INVENT") || text.contains("not yours to invent"),
            "the block is missing from the head — and the head is where it has \
             to be, because a corpus outage does not make a model id knowable"
        );
        assert!(
            text.contains("requires.ctx.model"),
            "the demand has to be named the way the catalogue names it"
        );
        assert!(
            text.contains("{\"question\""),
            "an instruction to 'ask' with no shape to answer in is an \
             instruction to improvise"
        );
    }
    // The mechanism: `normalise` really turns that answer into a named stop on
    // the error lane, and not into the generic 'your answer was not a list'.
    let normalise = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/builder/normalise/config.json"
    );
    let out = emit_one(
        &shipped_script(normalise),
        &json!({
            "target": "/os/builder/normalise",
            "header": {"hop": {"finish_reason": "stop"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "text", "id": "",
                          "text": "{\"question\": \"which model should the brain infer with?\"}"}],
        }),
    );
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "a composer that asks instead of guessing must come back under a code \
         of its own: 'you did not tell me the model' and 'your answer was not a \
         list' call for opposite repairs"
    );
    assert!(
        out["manifest"].is_null(),
        "no manifest slot on a refusal, question or not"
    );
    let payload: Value =
        meclaw_core::serde_json::from_str(out["messages"][0]["text"].as_str().expect("payload"))
            .expect("json payload");
    assert!(
        payload["reason"]
            .as_str()
            .is_some_and(|r| r.contains("which model")),
        "the question travels verbatim — a refusal a human cannot read is one \
         they cannot answer"
    );
    // and the ordinary refusal is untouched
    let plain = emit_one(
        &shipped_script(normalise),
        &json!({
            "target": "/os/builder/normalise",
            "header": {"hop": {"finish_reason": "stop"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "text", "id": "",
                          "text": "{\"declarations\": []}"}],
        }),
    );
    assert_eq!(
        plain["header"]["error_code"],
        json!("declarations_not_a_list")
    );
}

// --- GH #482: the one diff key whose shape was never published ---------------
//
// Measured on 2026-08-29 on a throwaway colony with a real hosted model as
// `MODEL_BUILDER`, on the first wish no recipe covers: *"a feed cell under the
// researcher that fetches three RSS feeds every ten minutes and emits one
// headline document per new item"*.
//
// The composer spent all seven rounds and every one of them well. It never lost
// the thread and it never wrote nonsense. It spent the whole budget looking for
// four templates that do not exist:
//
// ```text
// iter 0  catalogue_lookup "rss feed fetch poll" | graph_read <scope>
// iter 1  "RSS feed cell template" | "timer cron schedule cell"
// iter 2  "web-fetch-tool http" | "code cell template generic"
// ...
// iter 6  "code-cell template contract single cell" | "store-cell template ..."
// ```
//
// Then the iteration bound fired and the build ended on `no_manifest_in_answer`
// — paid for, and nothing delivered. Prompt 5 557 → 50 220 tokens.
//
// It searched because the head is correct and complete about instantiation:
// "there is no way to ask for a bare cell type — every node is an instance of a
// template that exists, so name one." A feed is a `timer` plus a `web_fetch`
// plus a `code` cell plus a `store`, and `templates/_cell-types/README.md`
// deliberately ships no single-cell template for any of the four.
//
// What it never tried is `add_templates`. The key is named in the head's list of
// eight diff keys and its FORM is published nowhere — so the one key that
// answers "the template I need does not exist" was the one key the composer
// could not use, while `seed_rows` got ROWS, `override_params` got OVERRIDES and
// `birth` got BIRTH.

#[test]
fn the_briefing_publishes_the_form_of_an_add_templates_entry() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("TEMPLATES --"),
            "the block is missing from the head — and the head is where it has \
             to be, because a corpus outage does not make a missing class exist"
        );
        assert!(
            text.contains("\"files\""),
            "an entry is {{name, files}}; naming the key without its second \
             half is the state that cost seven rounds"
        );
        assert!(
            text.contains("template.json"),
            "the one file the door demands has to be named"
        );
        assert!(
            text.contains("^[a-z][a-z0-9-]{1,63}$"),
            "the name pattern is a pre-destructive refusal; a composer that \
             cannot read it writes one and learns by being refused"
        );
        assert!(
            text.contains("local/"),
            "where the class lands is built by the colony and never taken from \
             the body — a composer that thinks it chooses the path writes one"
        );
        for refusal in ["invalid_template_name", "template_name_taken"] {
            assert!(
                text.contains(refusal),
                "the refusal {refusal} must be legible before it is earned"
            );
        }
        assert!(
            text.contains("FIRST operation"),
            "add_templates runs first in its diff, so an add_nodes of the SAME \
             diff resolves the class — that is why a build out of an own design \
             is ONE manifest and not two"
        );
    }
}

#[test]
fn the_briefing_says_the_four_types_have_no_template_to_name() {
    for text in [briefed(), degraded()] {
        for cell_type in ["code", "store", "timer", "web_fetch"] {
            assert!(
                text.contains(cell_type),
                "the head must say that {cell_type} has no single-cell template \
                 — that sentence is the one that ends the search loop"
            );
        }
        assert!(
            text.contains("BLANK single-cell template"),
            "the catalogue answers neighbours rather than 'not found', so the \
             absence has to be stated where the search would otherwise start \
             — and stated exactly: five templates DO hold a single `code` cell, \
             each with a script and a purpose of its own"
        );
        for named in ["door", "terminal", "retry", "dispatcher", "archive-bridge"] {
            assert!(
                text.contains(named),
                "{named} is a one-cell `code` template that exists; a head \
                 claiming there is none would be refuted by the catalogue on \
                 the first lookup, and a prompt a model can disprove is one it \
                 stops believing"
            );
        }
    }
}

#[test]
fn the_briefing_names_the_second_capability_question_as_the_colonys_decision() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("code.author"),
            "a manifest carrying executable behaviour asks a SECOND capability \
             question at the submitter, and it is off by default"
        );
        assert!(
            text.contains("code_author_denied"),
            "the denial has a verdict class of its own — the string a caller \
             greps for"
        );
        assert!(
            text.contains("script_inline"),
            "the derivation fires on an override_params carrying a script too, \
             not only on add_templates"
        );
    }
    // and it must not read as a prohibition: the colony decides, not the model.
    let text = briefed();
    let tail = &text[text.find("TEMPLATES --").expect("the block")..];
    assert!(
        tail.contains("not a formal defect") || tail.contains("not that your manifest"),
        "a composer that reads the denial as a form error repairs a manifest \
         that was never malformed — and the repair budget is separate and small"
    );
}

/// The submitter really derives that second question from an `add_templates` at
/// all — the prompt sentence above is a claim about the tree, so it is compared
/// against the tree rather than believed.
#[test]
fn the_submitter_really_asks_code_author_for_an_add_templates_manifest() {
    let gate = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/submit/gate/config.json"
    ))
    .expect("the shipped gate");
    let gate: Value = meclaw_core::serde_json::from_str(&gate).expect("parses");
    let script = gate["params"]["script_inline"].as_str().expect("script");
    assert!(
        script.contains("add_templates"),
        "the gate derives `code.author` from what the manifest CARRIES; if it \
         stopped, the head would be publishing a price that is not charged"
    );
    assert!(
        script.contains("code_author_denied"),
        "the code the head names has to be the code the gate emits"
    );
}

/// The one worked entry in the block is JSON inside JSON inside a python
/// literal inside a config file — four layers of escaping, and a model copies
/// what it is shown. So it is PARSED rather than eyeballed: the entry, and both
/// files it carries.
#[test]
fn the_add_templates_example_in_the_head_is_a_valid_entry() {
    let text = briefed();
    let start = text
        .find("{\"name\": \"rss-poll\"")
        .expect("the worked add_templates entry");
    let line = &text[start..start + text[start..].find('\n').expect("one line")];
    let entry: Value = meclaw_core::serde_json::from_str(line).expect("the entry parses");
    assert!(entry["name"].is_string(), "an entry names its class");
    let files = entry["files"].as_object().expect("files is a map");
    assert!(
        files.contains_key("template.json"),
        "template.json is the one file the door demands"
    );
    for (name, contents) in files {
        let inner: Value = meclaw_core::serde_json::from_str(contents.as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("{name} in the example is not readable json: {e}"));
        assert!(
            inner.is_object(),
            "{name} has to be the file's CONTENTS, as a string"
        );
    }
    assert!(
        files["config.json"].is_string(),
        "the file value is the contents as a STRING, not an inlined object — \
         an example that inlined it would teach a shape the colony writes to \
         disk verbatim"
    );
}

// --- GH #510: the two REMOVING keys were named and shaped nowhere -----------
//
// GRAMMAR gives the entry shape of `add_nodes` and `add_edges` and says which
// keys do not exist, because those were the refusals that were measured. The
// two removing keys were named in the head's key list — the same list that
// named `add_templates` before GH #482 gave it a block — and shaped nowhere,
// while a wish asking for something to be taken away is among the commonest
// there is.
//
// Measured, one wish, eight rounds, one draft, no repair budget left:
//
// ```text
// wish:   "the editor's assistant may only use web_search"
// scope:  .../assistants/editor/tools     (correct — a tool is a change
//                                          INSIDE the hive, as the brief says)
// diff:   remove_edges: [{"from": "./edit", "to": "."}, …]   -- correct
//         remove_nodes: ["./edit", "./web_fetch", …]         -- refused
//
// post_state_addresses/schema remove_nodes[0]: remove_nodes[].match.name missing
// (… once per entry, eight of them)
// ```
//
// Everything else in that draft was right: it read the tools hive, it used `.`
// as the scope root (GH #487), it drew the removals before anything else, and
// it kept `web_search`. The one thing it had no way to know is that a
// `remove_nodes` entry is `{"match": {"name": "<name>"}}` and not the path
// string every other diff key in the same manifest takes.
//
// The form published is the one `docs/rewiring.md` writes up and the one the
// door reads, and every claim below is checked against the door rather than
// against the sentence.

use meclaw_colony::mutation::MutationError;
use meclaw_colony::mutation::validate::{
    DIFF_OPERATIONS, EdgeMatchView, remove_edges_pattern_hits, validate_remove_edges,
};

/// The diff keys the head enumerates, read out of its own sentence.
fn head_diff_keys(text: &str) -> Vec<String> {
    let start = text
        .find("The diff keys you may use are ")
        .expect("the head enumerates the vocabulary")
        + "The diff keys you may use are ".len();
    let rest = &text[start..];
    let end = rest.find(" -- no others exist").expect("the list closes");
    rest[..end]
        .replace(" and ", ", ")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The head's list is the door's vocabulary — all eight of it, and nothing
/// else. `DIFF_OPERATIONS` is the door's own list ("this list IS the diff
/// vocabulary"), so an operation added there without reaching the briefing is
/// an operation the composer cannot use, and a word in the briefing that the
/// door does not read is a refusal waiting to happen.
#[test]
fn the_head_enumerates_exactly_the_eight_operations_the_door_reads() {
    for text in [briefed(), degraded()] {
        let mut named = head_diff_keys(&text);
        named.sort();
        let mut door: Vec<String> = DIFF_OPERATIONS.iter().map(|s| (*s).to_string()).collect();
        door.sort();
        assert_eq!(
            named, door,
            "the briefing's vocabulary and the door's have drifted apart"
        );
    }
}

#[test]
fn the_briefing_publishes_the_form_of_both_removing_keys() {
    for text in [briefed(), degraded()] {
        assert!(
            text.contains("REMOVING --"),
            "the block is missing from the head — and it has to be in the head, \
             because a corpus outage does not make a removal wish rarer"
        );
        let block = &text[text.find("REMOVING --").expect("the block")..];
        assert!(
            block.contains("{\"match\": {\"name\": \"<the node>\"}}"),
            "a remove_nodes entry is a MATCH OBJECT around a name; the measured \
             draft wrote the path string every other diff key takes and was \
             refused once per entry"
        );
        assert!(
            block.contains("{\"match\": {\"from\": \"./x\", \"to\": \"./y\"}}"),
            "a remove_edges entry names the two ENDS, inside a match of its own"
        );
        for narrowing in ["\"condition\"", "\"modifier\"", "\"default\""] {
            assert!(
                block.contains(narrowing),
                "{narrowing} narrows a remove_edges pattern, and a composer that \
                 does not know it cannot take one edge of a pair"
            );
        }
        assert!(
            block.contains("UNCONSTRAINED"),
            "the absent key is the trap: a pattern on the pair alone takes EVERY \
             edge between the two ends"
        );
        assert!(
            block.contains("match_no_hit"),
            "the refusal has to be legible before it is earned — a pattern that \
             hits nothing fails the WHOLE manifest"
        );
    }
}

/// The mechanism half of the same lock, in three parts, each driven through the
/// door's own validator rather than believed.
#[test]
fn the_door_really_reads_a_removing_entry_the_way_the_briefing_says() {
    let scope = "/os/a";
    let edges = vec![
        EdgeMatchView {
            from: "/os/a/x".into(),
            to: "/os/a/y".into(),
            condition_source: Some("has(hop.route)".into()),
            modifier_source: None,
            is_default: false,
        },
        EdgeMatchView {
            from: "/os/a/x".into(),
            to: "/os/a/y".into(),
            condition_source: None,
            modifier_source: None,
            is_default: true,
        },
    ];

    // 1 — the entry shape. A pattern without `from` is a schema refusal, and the
    //     door names the very key the briefing publishes.
    let err = validate_remove_edges(
        &json!({"remove_edges": [{"match": {"to": "./y"}}]}),
        scope,
        &edges,
    )
    .expect_err("a pattern without `from` is refused");
    assert!(
        matches!(&err, MutationError::Schema(s) if s.contains("remove_edges[].match.from")),
        "the door refuses the missing END by name: {err:?}"
    );

    // 2 — absent means unconstrained, and it reaches BOTH routing phases. This
    //     is the sentence the briefing prints in capitals, so it is the one
    //     worth measuring: one pattern, two edges, both hit.
    let hits = edges
        .iter()
        .filter(|e| remove_edges_pattern_hits(e, "/os/a/x", "/os/a/y", None, None, None))
        .count();
    assert_eq!(
        hits, 2,
        "a pattern on the pair alone must take every edge between those ends, \
         the default-phase one included"
    );
    let narrowed = edges
        .iter()
        .filter(|e| {
            remove_edges_pattern_hits(e, "/os/a/x", "/os/a/y", Some("has(hop.route)"), None, None)
        })
        .count();
    assert_eq!(
        narrowed, 1,
        "and naming the condition narrows it to one — otherwise `condition` in \
         the briefing is a key that changes nothing"
    );

    // 3 — a pattern that hits nothing fails, rather than passing as a no-op.
    let err = validate_remove_edges(
        &json!({"remove_edges": [{"match": {"from": "./x", "to": "./nowhere"}}]}),
        scope,
        &edges,
    )
    .expect_err("a pattern that hits nothing is refused");
    assert!(
        matches!(err, MutationError::MatchNoHit(_)),
        "match_no_hit is what the briefing promises: {err:?}"
    );
}

/// The other half of a tool restriction. The measured draft removed the cells
/// and their name edges and never touched `params.tools` — which leaves the
/// caller asking the hive for names that are gone. The briefing already says a
/// tool is two declarations on the way IN (§ TOOLS); GH #510 is the same
/// sentence on the way out.
#[test]
fn the_briefing_says_a_removal_does_not_take_the_name_out_of_the_caller() {
    for text in [briefed(), degraded()] {
        let block = &text[text.find("REMOVING --").expect("the block")..];
        assert!(
            block.contains("params.tools"),
            "removing the cells alone is half a tool restriction, and the half \
             that is missing is the one nothing refuses"
        );
        assert!(
            block.contains("override_params"),
            "the second half is an override_params in the SAME manifest — a \
             composer told only what to remove writes one declaration and stops"
        );
    }
}
