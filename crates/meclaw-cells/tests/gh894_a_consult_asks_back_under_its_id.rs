//! GH #894 (R-27-3) -- a consult asks back under its id.
//!
//! A consultation is a COMPLETE ORDER: the surface that hands the reasoning core an
//! errand says what the person wants, what is known, what is ruled out, what shape the
//! answer should have and how long it may be -- the core sees nothing of the
//! conversation but what it is sent. And the exchange is two-way: when the order is
//! not enough the core asks back, the surface answers from its own conversation or
//! asks the person, and the core carries on under the SAME `consult_id`. A later
//! question about the same errand travels under that id too, and the core still holds
//! the first one.
//!
//! Three names carry it, all declared by the core's own `./schemas` cell (OR-KY-Q2):
//!
//! * `consult_cogny` -- the voices' errand, now asking for a complete order and taking
//!   `consult_id` as a follow-up;
//! * `ask_requester` -- on the CORE's menu only: the question goes back to the surface
//!   that consulted, as an advice that reads `the core asks: <question>`;
//! * `reply_to_consult` -- on the VOICES' menu only: the answer goes to the core as its
//!   next errand under the consult's id (OR-KY-Q1).
//!
//! Every one of the three is a HANDOFF in its dispatcher (F-KX-1 of the A1 receipt:
//! no template declared the async class at all -- the list existed only in instances
//! and in tests), so the turn that sends it ends and the answer arrives as an event of
//! its own. A handoff also writes the `depart` row the return is correlated by
//! (GH #728), which is why a reply the person gave AFTER the deadline still comes back
//! under the turn it was given in.
//!
//! What is measured, and where:
//!
//! 1. **The declarations**, off the shipped `cogny/schemas` script, asked the way the
//!    collectors ask it -- by the voices (`context.tool_caller` `talky`/`talky-chat`)
//!    and by the core itself (`hop.tool_caller` `cogny`, its own inner menu edge).
//! 2. **The wiring**, off the shipped files: the dispatcher knobs on both surface
//!    markers and on the core's dispatcher, the two lane pairs of the assistant, the
//!    core's own menu edge and its `./ask` exit.
//! 3. **The road**, in a booted `assistant` with stub providers behind its three brains,
//!    measured at the receivers -- the request bodies the stubs recorded and the hops
//!    the colony logged: consult -> `ask_requester` -> the surface's request carries
//!    "the core asks: ..." under the consult id -> the person answers a turn later,
//!    past the deadline -> `reply_to_consult` -> the core's request carries the answer
//!    beside the first errand -> the core's answer, twenty thousand characters, reaches
//!    the surface whole -> the surface's answer leaves without the block id it quoted
//!    -> a follow-up under the same id reaches a core whose request still carries the
//!    first errand. Once for the spoken surface, once for the chat surface (the
//!    question comes back to the surface that asked).
//! 4. **No cut** (`no_consult_answer_is_cut`): a consult answer of 20 000 characters
//!    reaches the asking surface's provider request whole. No character cap is left on
//!    the consult road -- the length is asked for in the order, never cut.
//! 5. **Ids stay inside** (`a_short_id_never_leaves_the_colony`): a short block id
//!    `[#<12 hex>]` a model quotes in its prose is removed by the surface's splitter,
//!    through which every answer a model of the surface writes reaches its channel (a
//!    peer colony's included); so is one in a section that leaves the surface toward a
//!    reader. Prose without one leaves byte for byte, and an id inside the sidecar
//!    block, inside a tool call's arguments or inside a section the surface routes to
//!    its curator is left alone -- the sections that keep their ids are exactly the
//!    ones the composite routes there (`the_sections_that_keep_their_ids_are_the_ones_
//!    the_curator_takes`, OR-KY-68). The core's question back leaves its `./ask` cell
//!    without one too; the digest of a capped round is the collector's own and is
//!    pinned in `collector_window.rs`. An id is removed in every form a model quotes
//!    it -- bare as a history tool writes it, and the window's forms of a released,
//!    shortened or expired block -- on both surfaces' splitters, while prose that only
//!    looks like one leaves byte for byte (`a_short_id_in_any_of_its_forms_never_leaves_
//!    the_colony`, OR-KY-80).
//! 6. **A reply under an id nobody consulted** (`a_reply_under_an_unknown_consult_id_
//!    comes_back_under_it`): no typed error (OR-KY.Q.7), but no dead letter and no
//!    `error` either -- the core gets the answer as an errand, and its advice comes
//!    back correlated under the id the reply named.
//! 7. **Two consultations open at once** (`a_reply_that_waits_behind_another_consult_
//!    is_answered_under_its_own_id`): the reply to the first question reaches the core
//!    while the core is in the tool round of a second consultation of the same
//!    conversation; it must not wait for good, and it must not come back under the
//!    second consultation's id.
//!
//! Free of a real provider by construction: every `llm` cell of the booted tree is
//! pointed at a local stub. The member's memory and brief legs are switched off on the
//! surfaces (`memory_tier` and `brief_slots` empty) -- no member stands around this
//! assistant, and the recall road is GH #895's lock, not this one. The tool hive is a
//! silent stand-in: nothing in this road calls a tool of it.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, emit_all, override_params_on_disk, shipped_script};
use mock_openai::{
    MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_content_and_tool_calls,
    canned_tool_calls,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ═════════════════════════════════════════════════════════════════ the shipped tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file boots or reads. One missing = skipped (GH #49).
fn shipped() -> bool {
    [
        "assistant",
        "talky",
        "cogny",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn shipped_json(rel: &str) -> Value {
    read_json(&repo(rel))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

// ═══════════════════════════════════════════════════════════ 1. the declarations

const SCHEMAS_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/cogny/schemas/config.json"
);

/// Ask the core's `./schemas` cell what it serves, the way each asker's edge asks it:
/// a voice through the level's edge, which signs `context.tool_caller` with the
/// voice's node name (`None` for an asker that signs nothing -- a standalone core's);
/// the core itself through its composite's inner menu edge, which stamps
/// `hop.tool_caller` 'cogny' and nothing in the context. Returns the whole answer.
fn ask_schemas(caller: Option<&str>, tools: Value) -> Value {
    let (hop, context) = match caller {
        Some("cogny") => (
            json!({"route": "in_schemas", "tool_caller": "cogny"}),
            json!({}),
        ),
        Some(c) => (json!({"route": "in_schemas"}), json!({"tool_caller": c})),
        None => (json!({"route": "in_schemas"}), json!({})),
    };
    let out = emit_all(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/assistant/cogny/schemas",
            "header": {"hop": hop, "context": context},
            "ttl": 64,
            "tools": tools,
            "messages": [],
        }),
    );
    out.first().cloned().unwrap_or(Value::Null)
}

/// The schemas and the unknown names of one ask ([`ask_schemas`]).
fn declared(caller: Option<&str>, tools: Value) -> (Vec<Value>, Vec<String>) {
    let one = ask_schemas(caller, tools);
    (
        one["schemas"].as_array().cloned().unwrap_or_default(),
        strings(&one["unknown"]),
    )
}

fn names_of(schemas: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = schemas
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    v.sort();
    v
}

fn schema<'a>(schemas: &'a [Value], name: &str) -> &'a Value {
    schemas
        .iter()
        .find(|s| s["name"] == json!(name))
        .unwrap_or_else(|| panic!("no `{name}` among {schemas:#?}"))
}

#[test]
fn consult_cogny_asks_for_a_complete_order() {
    let (schemas, unknown) = declared(Some("talky"), json!(["consult_cogny"]));
    assert!(unknown.is_empty(), "{unknown:?}");
    let consult = schema(&schemas, "consult_cogny");
    let desc = consult["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    // The five parts of an order, each named where the caller reads the tool.
    for part in [
        "goal",
        "facts",
        "constraints",
        "form of the answer",
        "how long",
    ] {
        assert!(
            desc.contains(part),
            "a complete order names `{part}` -- the core sees nothing of the conversation \
             but what it is sent:\n{desc}"
        );
    }
    // The length is PROMPTED, never cut: the description carries the wish in words.
    assert!(
        desc.contains("keep the answer under 4000 characters when you can"),
        "the length wish is part of the order, in words:\n{desc}"
    );
    assert!(
        desc.contains("reply_to_consult"),
        "a question the core asked back is answered with `reply_to_consult`, not with a \
         second consultation:\n{desc}"
    );
    // `question` and `context` stay required (the #528 ruling), and `consult_id` is the
    // FOLLOW-UP key now.
    assert_eq!(
        strings(&consult["parameters"]["required"]),
        vec!["question".to_string(), "context".to_string()],
        "{consult:#?}"
    );
    let follow = consult["parameters"]["properties"]["consult_id"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        follow.contains("follow"),
        "`consult_id` continues an earlier consultation: {consult:#?}"
    );
    let context = consult["parameters"]["properties"]["context"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    for part in ["goal", "facts", "constraints", "form", "long"] {
        assert!(
            context.contains(part),
            "the `context` argument is where the order goes, and it says so (`{part}`): \
             {context}"
        );
    }
}

#[test]
fn ask_requester_is_on_the_cores_menu_only() {
    // The core asking itself (its own collector's menu tick, `hop.tool_caller` 'cogny').
    let (core, unknown) = declared(Some("cogny"), json!(["*"]));
    assert_eq!(
        names_of(&core),
        vec!["ask_requester".to_string()],
        "the core's own menu carries the one name it calls and neither of the voices' \
         names -- a core offered `consult_cogny` could consult itself"
    );
    assert!(unknown.is_empty(), "{unknown:?}");
    // The answer names its half on the hop: the composite routes it back into the
    // core's collector on `core` and out of the rim on anything else.
    assert_eq!(
        ask_schemas(Some("cogny"), json!(["*"]))["header"]["audience"],
        "core"
    );
    for caller in [Some("talky"), None] {
        assert_eq!(
            ask_schemas(caller, json!(["*"]))["header"]["audience"],
            "voice",
            "{caller:?}"
        );
    }
    let ask = schema(&core, "ask_requester");
    assert_eq!(
        strings(&ask["parameters"]["required"]),
        vec!["question".to_string()],
        "{ask:#?}"
    );
    let desc = ask["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        desc.contains("ends") && desc.contains("next errand"),
        "the core is told that its turn ends and where the answer comes back: {desc}"
    );

    // A voice never gets it, not by `*` and not by name.
    for caller in [Some("talky"), Some("talky-chat"), None] {
        let (all, _) = declared(caller, json!(["*"]));
        assert!(
            !names_of(&all).contains(&"ask_requester".to_string()),
            "{caller:?} was offered the core's own question tool: {all:#?}"
        );
        let (named, unknown) = declared(caller, json!(["ask_requester"]));
        assert!(named.is_empty(), "{caller:?}: {named:#?}");
        assert_eq!(unknown, vec!["ask_requester".to_string()], "{caller:?}");
    }

    // The core ASKS its own cell: an inner edge on the menu tick, stamped 'cogny' on the
    // HOP -- a context key would ride out of the rim with the answer (gh494) -- and the
    // answer, marked `hop.audience == 'core'`, goes back to the core's collector and
    // never out of the rim.
    let cogny = shipped_json("templates/cogny/config.json");
    let edges = cogny["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ask_own = edges
        .iter()
        .find(|e| {
            e["from"] == json!("./collector")
                && e["to"] == json!("./schemas")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'schemas'"))
        })
        .unwrap_or_else(|| panic!("the core does not ask its own declarations: {edges:#?}"));
    assert_eq!(
        ask_own["modifier"]["set_hop"]["tool_caller"], "'cogny'",
        "{ask_own:#?}"
    );
    assert!(
        ask_own["modifier"]["set_context"].is_null(),
        "no context key: it would outlive the answer and leave the rim with it: {ask_own:#?}"
    );
    let back = edges
        .iter()
        .find(|e| e["from"] == json!("./schemas") && e["to"] == json!("./collector"))
        .unwrap_or_else(|| panic!("the core's own answer has no way back in: {edges:#?}"));
    let cond = back["condition"].as_str().unwrap_or_default();
    assert!(cond.contains("hop.audience == 'core'"), "{back:#?}");
    assert_eq!(
        back["modifier"]["set_hop"]["route"], "'in_menu'",
        "{back:#?}"
    );
    assert_eq!(
        back["modifier"]["set_context"]["tool_answerer"], "'cogny'",
        "{back:#?}"
    );
    let rim = edges
        .iter()
        .find(|e| e["from"] == json!("./schemas") && e["to"] == json!("."))
        .unwrap_or_else(|| panic!("the rim exit of the declarations is gone: {edges:#?}"));
    assert!(
        rim["condition"]
            .as_str()
            .is_some_and(|c| c.contains("!(has(hop.audience) && hop.audience == 'core')")),
        "the core's own answer must not ALSO leave the rim for a voice: {rim:#?}"
    );

    // And it is a handoff in the core's dispatcher: the core's turn ends with it.
    let dispatcher = shipped_json("templates/cogny/dispatcher/config.json");
    let knobs = &dispatcher["override_params"][""];
    for key in ["async_tools", "handoff_tools"] {
        assert!(
            strings(&knobs[key]).contains(&"ask_requester".to_string()),
            "`ask_requester` is missing from the core dispatcher's `{key}`: {dispatcher:#?}"
        );
    }
    assert_eq!(
        knobs["interim"], "",
        "the core still has no channel (GH #539): {dispatcher:#?}"
    );
}

#[test]
fn reply_to_consult_is_on_the_voices_menu() {
    for caller in ["talky", "talky-chat"] {
        let (all, _) = declared(Some(caller), json!(["*"]));
        assert_eq!(
            names_of(&all),
            vec!["consult_cogny".to_string(), "reply_to_consult".to_string()],
            "{caller}: the voices' half of the consult contract"
        );
    }
    let (named, unknown) = declared(Some("talky"), json!(["reply_to_consult"]));
    assert!(unknown.is_empty(), "{unknown:?}");
    let reply = schema(&named, "reply_to_consult");
    assert_eq!(
        strings(&reply["parameters"]["required"]),
        vec!["consult_id".to_string(), "answer".to_string()],
        "{reply:#?}"
    );
    let desc = reply["description"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        desc.contains("the core asks") && desc.contains("ask the person"),
        "the voice is told what it answers and that it may ask the person first: {desc}"
    );
    // The core does not get it.
    let (core, _) = declared(Some("cogny"), json!(["reply_to_consult", "consult_cogny"]));
    assert!(core.is_empty(), "{core:#?}");

    // F-KX-1: the async class is declared by the LEVEL, on both surface markers, and the
    // two markers stay word for word (#709).
    for marker in [
        "templates/assistant/talky/config.json",
        "templates/assistant/talky-chat/config.json",
    ] {
        let m = shipped_json(marker);
        let over = &m["override_params"];
        assert!(
            strings(&over["collector/assemble"]["tools"]).contains(&"reply_to_consult".to_string()),
            "{marker}: the surface declares the name it asks the core for: {over:#?}"
        );
        for key in ["async_tools", "handoff_tools"] {
            let list = strings(&over["dispatcher"][key]);
            for name in ["consult_cogny", "reply_to_consult"] {
                assert!(
                    list.contains(&name.to_string()),
                    "{marker}: `{name}` is missing from `dispatcher.{key}` -- without it the \
                     dispatcher sets no `consult_id` and the fan-in waits for an answer that \
                     comes back as an advice: {over:#?}"
                );
            }
        }
    }
}

/// The two lane pairs of the level, read off the file: the question back from the core
/// to the surface that asked, and the answer from either surface to the core.
#[test]
fn the_ask_and_the_reply_travel_on_edges_of_their_own() {
    let assistant = shipped_json("templates/assistant/config.json");
    let edges = assistant["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // The question back: `./cogny` -> the surface, split on the channel exactly like
    // the advice that answers a consult.
    for (to, split) in [
        (
            "./talky",
            "(!has(context.channel_node) || context.channel_node != 'chat')",
        ),
        (
            "./talky-chat",
            "has(context.channel_node) && context.channel_node == 'chat'",
        ),
    ] {
        let hits: Vec<&Value> = edges
            .iter()
            .filter(|e| {
                e["from"] == json!("./cogny")
                    && e["to"] == json!(to)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.route == 'ask'"))
            })
            .collect();
        assert_eq!(hits.len(), 1, "one question edge to {to}: {hits:#?}");
        let e = hits[0];
        assert!(
            e["condition"].as_str().unwrap_or_default().contains(split),
            "the question goes back to the surface that asked: {e:#?}"
        );
        let m = &e["modifier"];
        assert_eq!(m["set_hop"]["route"], "'in_advice'", "{e:#?}");
        assert_eq!(m["set_context"]["consult_id"], "hop.consult_id", "{e:#?}");
        assert_eq!(m["set_context"]["consult_class"], "'ask'", "{e:#?}");
        assert_eq!(
            m["set_context"]["col_phase"], "''",
            "a message out of another collector's chain arrives mid-assembly unless the \
             phase is cleared: {e:#?}"
        );
        assert_eq!(m["restore_ttl"], json!(true), "{e:#?}");
    }

    // The answer: either surface -> `./cogny`, a named errand like the consult itself.
    for from in ["./talky", "./talky-chat"] {
        let hits: Vec<&Value> = edges
            .iter()
            .filter(|e| {
                e["from"] == json!(from)
                    && e["to"] == json!("./cogny")
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.tool_name == 'reply_to_consult'"))
            })
            .collect();
        assert_eq!(hits.len(), 1, "one reply edge from {from}: {hits:#?}");
        let e = hits[0];
        assert!(
            e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("hop.route == 'tool'"),
            "the lane before the discriminator (W7-R4): {e:#?}"
        );
        assert!(
            e.get("default").is_none() || e["default"] == json!(false),
            "a named errand suppresses the tools default, it does not compete with it: {e:#?}"
        );
        let m = &e["modifier"];
        assert_eq!(m["set_hop"]["route"], "'in_turn'", "{e:#?}");
        assert_eq!(m["set_context"]["consult_id"], "hop.consult_id", "{e:#?}");
        assert_eq!(m["set_context"]["consult_class"], "'reply'", "{e:#?}");
        assert_eq!(m["set_context"]["col_phase"], "''", "{e:#?}");
        let deleted = strings(&m["delete_context"]);
        for key in ["turn_id", "tools_allow", "tools_deny"] {
            assert!(
                deleted.contains(&key.to_string()),
                "the reply drops `{key}` like the consult does (#728, #845): {e:#?}"
            );
        }
    }
    // The tools default is still there for every other tool.
    for from in ["./talky", "./talky-chat"] {
        assert!(
            edges.iter().any(|e| e["from"] == json!(from)
                && e["to"] == json!("./tools")
                && e["default"] == json!(true)),
            "{from} lost its tools default"
        );
    }

    // The core's side: the call leaves through `./ask`, and the rim declares `ask`.
    let cogny = shipped_json("templates/cogny/config.json");
    let inner = cogny["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        inner.iter().any(|e| e["from"] == json!("./dispatcher")
            && e["to"] == json!("./ask")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.tool_name == 'ask_requester'"))),
        "the core's call reaches `./ask`: {inner:#?}"
    );
    assert!(
        inner.iter().any(|e| e["from"] == json!("./ask")
            && e["to"] == json!(".")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'ask'"))),
        "and leaves the rim on `ask`: {inner:#?}"
    );
    let emits: Vec<String> = cogny["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect();
    assert!(emits.contains(&"ask".to_string()), "{emits:?}");
}

/// OR-KY-75: the core's collector opens a round for every errand, the voices keep
/// the telephone model -- read off the shipped refs and both surface markers.
#[test]
fn the_core_opens_a_round_for_every_errand_and_the_voices_do_not() {
    let core = shipped_json("templates/cogny/collector/config.json");
    assert_eq!(
        core["override_params"]["assemble"]["defer_turns"], "0",
        "a core defers no errand behind another: {core:#?}"
    );
    let shipped_default =
        shipped_json("templates/collector/assemble/config.json")["params"]["defer_turns"].clone();
    assert_eq!(
        shipped_default, "1",
        "the collector's own default is today's"
    );
    for rel in [
        "templates/talky/collector/config.json",
        "templates/assistant/talky/config.json",
        "templates/assistant/talky-chat/config.json",
    ] {
        let v = shipped_json(rel);
        for key in ["assemble", "collector/assemble"] {
            assert!(
                v["override_params"][key].get("defer_turns").is_none(),
                "{rel}: a voice keeps one open round per session: {v:#?}"
            );
        }
    }
}

const ASK_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/cogny/ask/config.json"
);

#[test]
fn the_ask_cell_says_what_the_core_asks_under_the_consults_id() {
    let run = |args: &str, ctx: Value| -> Value {
        let out = emit_all(
            &shipped_script(ASK_CELL),
            &json!({
                "target": "/main/assistant/cogny/ask",
                "header": {"hop": {"route": "tool", "tool_name": "ask_requester",
                                   "tool_call_id": "call-a1", "consult_id": "call-a1",
                                   "async": "1"},
                           "context": ctx},
                "ttl": 64,
                "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-a1",
                              "text": args}],
            }),
        );
        assert_eq!(out.len(), 1, "{out:#?}");
        out[0].clone()
    };
    let out = run(
        r#"{"question": "Which city are you travelling to?"}"#,
        json!({"consult_id": "call-c1"}),
    );
    assert_eq!(out["header"]["route"], "ask", "{out:#?}");
    assert_eq!(
        out["header"]["consult_id"], "call-c1",
        "the question travels under the CONSULT's id, the one the surface's departure \
         names -- not under the id of the call that asked it: {out:#?}"
    );
    assert_eq!(
        out["messages"][0]["text"], "the core asks: Which city are you travelling to?",
        "{out:#?}"
    );
    // A call that lost its argument shape is still a question -- never a silence.
    let raw = run("which city?", json!({"consult_id": "call-c1"}));
    assert_eq!(
        raw["messages"][0]["text"], "the core asks: which city?",
        "{raw:#?}"
    );
    // An id of the core's window is a reference into a wall the surface cannot read,
    // and the question goes on to a person: it leaves without one (review m-1).
    let quoted = run(
        r#"{"question": "Is [#0123456789ab] still the city, or [#abcdefABCDEF]?"}"#,
        json!({"consult_id": "call-c1"}),
    );
    assert_eq!(
        quoted["messages"][0]["text"], "the core asks: Is still the city, or?",
        "{quoted:#?}"
    );
    // ...in every form a model quotes one, bare as a history tool writes it included
    // (OR-KY-80).
    let bare = run(
        r#"{"question": "Is #0123456789ab still the city, or [#abcdefabcdef expired]?"}"#,
        json!({"consult_id": "call-c1"}),
    );
    assert_eq!(
        bare["messages"][0]["text"], "the core asks: Is still the city, or?",
        "{bare:#?}"
    );
}

// ═══════════════════════════════════════════════════════ 5. ids stay inside (script)

const SPLITTER_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/talky/splitter/config.json"
);

/// The splitter's knobs as shipped (`nothing_block`, `id_sections`), without the
/// script itself.
fn splitter_params() -> Value {
    let mut p = read_json(std::path::Path::new(SPLITTER_CELL))["params"].clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("script_inline");
    }
    p
}

fn split_with(messages: Value, finish: &str, params: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(SPLITTER_CELL),
        &json!({
            "target": "/main/talky/splitter",
            "header": {"hop": {"finish_reason": finish}, "context": {}},
            "ttl": 64,
            "messages": messages,
            "params": params,
        }),
    )
}

fn split(messages: Value, finish: &str) -> Vec<Value> {
    split_with(messages, finish, splitter_params())
}

/// The message of one section out of a split, by name.
fn section_of<'a>(out: &'a [Value], name: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["section"] == json!(name))
        .unwrap_or_else(|| panic!("the `{name}` section did not leave: {out:#?}"))
}

/// The section names a CEL condition compares `hop.section` with.
fn sections_named(cond: &str) -> Vec<String> {
    let mut v: Vec<String> = cond
        .split("hop.section == '")
        .skip(1)
        .filter_map(|rest| rest.split('\'').next().map(str::to_string))
        .collect();
    v.sort();
    v
}

fn text_turn(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

#[test]
fn a_short_id_never_leaves_the_colony() {
    // Prose that quotes a block id leaves without it.
    let out = split(
        json!([text_turn("As you said [#0123456789ab], Lisbon it is.")]),
        "stop",
    );
    assert_eq!(out.len(), 1, "{out:#?}");
    assert_eq!(
        out[0]["messages"][0]["text"], "As you said, Lisbon it is.",
        "{out:#?}"
    );
    // At the very start, too, without a stray space.
    let out = split(json!([text_turn("[#0123456789AB] Lisbon it is.")]), "stop");
    assert_eq!(out[0]["messages"][0]["text"], "Lisbon it is.", "{out:#?}");

    // Prose without one leaves byte for byte -- including brackets that are no id.
    for text in [
        "Lisbon it is.",
        "See [#note] and [#0123] and [# 0123456789ab] -- none of them is an id.",
        "  two leading spaces, a tab\tand a trailing newline\n",
    ] {
        let out = split(json!([text_turn(text)]), "stop");
        assert_eq!(out[0]["messages"][0]["text"], text, "{out:#?}");
    }

    // The block is the colony's own: an id inside it stays, the prose around it is cut.
    let answered = "Done [#0123456789ab].\n\n```sidecar\n{\"window\": {\"release\": [\"#0123456789ab\"]}}\n```";
    let out = split(json!([text_turn(answered)]), "stop");
    let half = out
        .iter()
        .find(|m| m["header"]["route"].is_null())
        .unwrap_or_else(|| panic!("no answer half: {out:#?}"));
    let prose = half["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(!prose.contains("[#"), "{prose}");
    assert!(prose.starts_with("Done."), "{prose}");
    let window = out
        .iter()
        .find(|m| m["header"]["section"] == json!("window"))
        .unwrap_or_else(|| panic!("the `window` section did not leave: {out:#?}"));
    assert_eq!(
        window["payload"]["release"][0], "#0123456789ab",
        "an id inside the block is the curator's business: {window:#?}"
    );

    // Beside a tool call: the sentence to the person is cleaned, the call is not -- an
    // id is exactly what a history tool takes as its argument.
    let call = json!({"name": "history_read", "arguments": "{\"id\": \"[#0123456789ab]\"}"});
    let out = split(
        json!([
            {"origin": "assistant", "type": "tool_call", "id": "k1", "text": call.to_string()},
            text_turn("Let me look at [#0123456789ab] again.")
        ]),
        "tool_calls",
    );
    assert_eq!(out.len(), 1, "{out:#?}");
    assert_eq!(
        out[0]["messages"][0]["text"],
        call.to_string(),
        "a tool call's arguments are the colony's own: {out:#?}"
    );
    assert_eq!(
        out[0]["messages"][1]["text"], "Let me look at again.",
        "{out:#?}"
    );

    // A section leaves the surface toward a reader -- a duplex voice speaks `fact`,
    // `context` and `correction` aloud -- so its words lose their ids too; the
    // sections the surface routes to its curator keep them (OR-KY-68).
    let block = "Noted [#0123456789ab].\n\n```sidecar\n{\"fact\": \"You said [#0123456789ab] Lisbon.\", \"context\": {\"payload\": \"see [#abcdefabcdef]\", \"more\": [\"a [#abcdefabcdef] b\"]}, \"window\": {\"release\": [\"[#0123456789ab]\"]}, \"memory\": {\"facts\": [\"Lisbon [#0123456789ab]\"]}}\n```";
    let out = split(json!([text_turn(block)]), "stop");
    assert_eq!(
        section_of(&out, "fact")["payload"]["payload"],
        "You said Lisbon.",
        "{out:#?}"
    );
    let context = &section_of(&out, "context")["payload"];
    assert_eq!(context["payload"], "see", "{context:#?}");
    assert_eq!(
        context["more"][0], "a b",
        "nested strings too: {context:#?}"
    );
    assert_eq!(
        section_of(&out, "window")["payload"]["release"][0],
        "[#0123456789ab]",
        "a TRIM names its ids on purpose: {out:#?}"
    );
    assert_eq!(
        section_of(&out, "memory")["payload"]["facts"][0],
        "Lisbon [#0123456789ab]",
        "{out:#?}"
    );
    // A section that was nothing but an id is a section with no words, and is
    // dropped by name like every other one.
    let out = split(
        json!([text_turn(
            "Right.\n\n```sidecar\n{\"correction\": \"[#0123456789ab]\"}\n```"
        )]),
        "stop",
    );
    assert_eq!(out.len(), 1, "{out:#?}");
    assert_eq!(
        out[0]["header"]["sidecar_dropped"], "correction",
        "{out:#?}"
    );
    // Without the knob no section keeps an id -- the safe side of R-27-3.
    let out = split_with(json!([text_turn(block)]), "stop", json!({}));
    assert_eq!(
        section_of(&out, "window")["payload"]["release"][0],
        "",
        "{out:#?}"
    );
}

const COGNY_SPLITTER_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/cogny/splitter/config.json"
);

/// One answer through the splitter `cell`, with that cell's own shipped knobs.
fn split_through(cell: &str, messages: Value, finish: &str) -> Vec<Value> {
    let mut params = read_json(std::path::Path::new(cell))["params"].clone();
    if let Some(o) = params.as_object_mut() {
        o.remove("script_inline");
    }
    emit_all(
        &shipped_script(cell),
        &json!({
            "target": "/main/talky/splitter",
            "header": {"hop": {"finish_reason": finish}, "context": {}},
            "ttl": 64,
            "messages": messages,
            "params": params,
        }),
    )
}

/// OR-KY-80 (review of GH #893, M-3): a short id is not only the bracket the window
/// shows. A history tool answers with the id BARE (`"id": "#<12 hex>"`), and the window
/// shows a released, a shortened and an expired block in forms of their own
/// (`templates/curator/policy`, `RELEASED_FORM`/`SHRUNK_FORM`/`EXPIRED_FORM`). A model
/// quotes whichever it has just read, and each of them names a block of a wall no reader
/// can open. So each of them comes out, on both surfaces (cogny's splitter carries the
/// talky's script, `cognys_splitter_is_talkys`) -- and so does the sixteen-digit form a
/// history tool offers as `candidates` when a short id names more than one block
/// (OR-KY-81: every id form the colony writes, `#` and twelve to sixteen hex digits),
/// and the full block id a `history_read` answer carries as `hash`, sixty-four hex
/// digits with or without `#` (OR-KY-84). Prose that only LOOKS like an id -- a step
/// number, a colour, a hash of another length, a fragment glued to a link, an order
/// number of digits only -- leaves byte for byte, because a free-standing run is an id
/// only with at least one letter a-f in it; inside the window's own brackets any hex
/// run is one.
#[test]
fn a_short_id_in_any_of_its_forms_never_leaves_the_colony() {
    for cell in [SPLITTER_CELL, COGNY_SPLITTER_CELL] {
        let said = |text: &str| -> String {
            let out = split_through(cell, json!([text_turn(text)]), "stop");
            assert_eq!(out.len(), 1, "{cell}: {out:#?}");
            out[0]["messages"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        for (text, want) in [
            ("Block #0123456789ab says Lisbon.", "Block says Lisbon."),
            (
                "#0123456789AB is where you said it.",
                "is where you said it.",
            ),
            ("The note (#abcdefabcdef) says May.", "The note says May."),
            (
                "I let it go [#0123456789ab released \u{2014} history_read(\"#0123456789ab\")] earlier.",
                "I let it go earlier.",
            ),
            (
                "That one is gone [#abcdefabcdef expired].",
                "That one is gone.",
            ),
            (
                "Two blocks match: #0123456789abcdef and #0123456789ab0123.",
                "Two blocks match: and.",
            ),
            (
                "The block 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef says May.",
                "The block says May.",
            ),
            (
                "Read #0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef again.",
                "Read again.",
            ),
            (
                "An id of digits only [#123456789012] is still an id.",
                "An id of digits only is still an id.",
            ),
            (
                "The note [#0123456789ab] Lisbon [shortened \u{2014} history_read(\"#0123456789ab\")] was long.",
                "The note Lisbon was long.",
            ),
        ] {
            assert_eq!(said(text), want, "{cell}");
        }
        for text in [
            "Step #3 of 5, issue #42.",
            "Colours #fff, #1a2b3c and #1a2b3c4d.",
            "The hash #0123456789abcdef0 is seventeen long, #0123456789a eleven.",
            "See https://example.com/page#0123456789ab for it.",
            "C# and F#, ## a heading, and [#note].",
            "Your order #123456789012 has shipped, invoice #2026092900001 is due.",
            "Sixty-five: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0, sixty-four digits: 0123456789012345678901234567890123456789012345678901234567890123.",
        ] {
            assert_eq!(
                said(text),
                text,
                "{cell}: prose that only looks like an id leaves byte for byte"
            );
        }
        // A section toward a reader loses a bare id as well; one the surface routes
        // to its curator keeps it.
        let block = "Noted.\n\n```sidecar\n{\"fact\": \"You said #0123456789ab Lisbon.\", \"window\": {\"release\": [\"#0123456789ab\"]}}\n```";
        let out = split_through(cell, json!([text_turn(block)]), "stop");
        assert_eq!(
            section_of(&out, "fact")["payload"]["payload"],
            "You said Lisbon.",
            "{cell}: {out:#?}"
        );
        assert_eq!(
            section_of(&out, "window")["payload"]["release"][0],
            "#0123456789ab",
            "{cell}: {out:#?}"
        );
    }
}

/// OR-KY-83: the id filter is ONE expression with six copies, one in every cell where
/// a model's words leave toward a reader -- both surfaces' splitters, the digest of a
/// capped round, the core's question back, the enriched push a duplex voice speaks
/// (there it is named `SPOKEN_ID`, beside a `SHORT_ID` that FINDS ids), and the
/// handover block a renewed voice is given. A form one copy learns and another does not
/// is an id that leaves by the other door; so the six compile to the same pattern,
/// byte for byte, read off each script the way Python reads it.
const SHORT_ID_COPIES: &[(&str, &str)] = &[
    ("templates/talky/splitter/config.json", "SHORT_ID"),
    ("templates/cogny/splitter/config.json", "SHORT_ID"),
    ("templates/collector/assemble/config.json", "SHORT_ID"),
    ("templates/cogny/ask/config.json", "SHORT_ID"),
    ("templates/curator/push/config.json", "SPOKEN_ID"),
    ("templates/curator/handover/config.json", "SHORT_ID"),
];

/// The pattern the script of `rel` compiles under `name`, as Python joins its literals.
fn compiled_pattern(rel: &str, name: &str) -> String {
    let path = repo(rel);
    let cfg = read_json(&path);
    let script = cfg["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel}: no script_inline"));
    // The script travels on stdin: one argv string is capped at 128 KiB.
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(concat!(
            "import ast, sys\n",
            "for n in ast.parse(sys.stdin.read()).body:\n",
            "    if isinstance(n, ast.Assign) and any(getattr(t, 'id', '') == sys.argv[1] for t in n.targets):\n",
            "        sys.stdout.write(ast.literal_eval(n.value.args[0]))\n",
        ))
        .arg(name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), script.as_bytes())
        .expect("write the script");
    let out = child.wait_with_output().expect("wait");
    let pattern = String::from_utf8(out.stdout).expect("utf-8");
    assert!(!pattern.is_empty(), "{rel} compiles no `{name}`");
    pattern
}

#[test]
fn every_copy_of_the_short_id_filter_is_one_expression() {
    let (first, first_name) = SHORT_ID_COPIES[0];
    let want = compiled_pattern(first, first_name);
    for (rel, name) in &SHORT_ID_COPIES[1..] {
        assert_eq!(
            compiled_pattern(rel, name),
            want,
            "{rel} `{name}` drifted from {first} `{first_name}` -- an id form one copy \
             learns and another does not leaves by the other door (OR-KY-83)"
        );
    }
}

/// OR-KY-68: the splitter judges no section by name, so the sections whose words keep
/// their ids are named by the composite (`id_sections`) -- and they are exactly the
/// ones the talky routes to its curator, and none of them leaves the rim. The core's
/// copy carries the same list (its params are the talky's, `cognys_splitter_is_talkys`)
/// and routes every section to its curator.
#[test]
fn the_sections_that_keep_their_ids_are_the_ones_the_curator_takes() {
    let mut keep = strings(&splitter_params()["id_sections"]);
    keep.sort();
    assert!(
        !keep.is_empty(),
        "the talky names the sections it keeps ids in"
    );
    let talky = shipped_json("templates/talky/config.json");
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let cond = |to: &str| -> String {
        edges
            .iter()
            .find(|e| {
                e["from"] == json!("./splitter")
                    && e["to"] == json!(to)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.route == 'sidecar'"))
            })
            .and_then(|e| e["condition"].as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("no sidecar edge ./splitter -> {to}: {edges:#?}"))
    };
    assert_eq!(
        sections_named(&cond("./curator")),
        keep,
        "the sections that keep their ids are the ones the curator takes"
    );
    let rim = cond(".");
    assert_eq!(sections_named(&rim), keep, "{rim}");
    assert!(
        rim.contains("!("),
        "the rim takes every section BUT those: {rim}"
    );
    let cogny = shipped_json("templates/cogny/splitter/config.json");
    let mut cogny_keep = strings(&cogny["params"]["id_sections"]);
    cogny_keep.sort();
    assert_eq!(cogny_keep, keep, "the core's copy is the original");
}

// ═══════════════════════════════════════════════════════════════ 3. the road (booted)

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// Left alone after the last expected arrival before "nothing else" is read.
const SETTLE: Duration = Duration::from_secs(2);
/// The deadline of a consult on the surfaces, shortened for this run so the person's
/// answer can arrive PAST it (the case the two-stage park of #728 exists for).
const LATE_AFTER_MS: u64 = 1000;
/// How long the person takes to answer the core's question -- past `LATE_AFTER_MS`.
const THINKING: Duration = Duration::from_millis(1500);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const CHANNEL: &str = "chat:894";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;

const T1: &str = "Plan three days somewhere nice for me.";
const T2: &str = "Lisbon, in May.";
const T3: &str = "And what would those three days cost?";

const CONSULT_ID: &str = "call-c1";
const CONSULT_ARGS: &str = r#"{"question": "Plan three days in a city for the person.", "context": "Goal: a short trip. Facts: none yet. Constraints: none named. Form: a day-by-day plan. Length: keep the answer under 4000 characters when you can."}"#;
const FIRST_ERRAND: &str = "Plan three days in a city for the person.";
const INTERIM_1: &str = "Let me ask my core.";

const ASK_ID: &str = "call-a1";
const QUESTION_BACK: &str = "Which city are you travelling to?";
const ASK_ARGS: &str = r#"{"question": "Which city are you travelling to?"}"#;
const ASKED: &str = "the core asks: Which city are you travelling to?";

const REPLY_ID: &str = "call-r1";
const REPLY_ARGS: &str = r#"{"consult_id": "call-c1", "answer": "Lisbon, in May."}"#;
const INTERIM_2: &str = "Thanks, I have passed that on.";

const FINAL_2: &str = "Here is your plan [#0123456789ab] -- enjoy.";
const FINAL_2_CLEAN: &str = "Here is your plan -- enjoy.";

const FOLLOW_ID: &str = "call-c2";
const FOLLOW_ARGS: &str = r#"{"question": "What would the three days cost?", "context": "The same trip as before; the person asks for the price.", "consult_id": "call-c1"}"#;
const FOLLOW_ERRAND: &str = "What would the three days cost?";
const INTERIM_3: &str = "Let me check that too.";
const BUDGET: &str = "About 900 EUR for the three days.";
const FINAL_3: &str = "About 900 euros, all in.";

/// A consult answer far past any cap that ever stood on this road.
const LONG_CHARS: usize = 20_000;

/// Twenty thousand characters, numbered, so a cut anywhere shows as a missing number.
fn long_answer() -> String {
    let mut s = String::from("PLAN ");
    let mut i = 0usize;
    while s.len() < LONG_CHARS - 4 {
        s.push_str(&format!("w{i:05} "));
        i += 1;
    }
    s.truncate(LONG_CHARS - 4);
    s.push_str(" END");
    assert_eq!(s.chars().count(), LONG_CHARS);
    s
}

/// Which of the two surfaces a run speaks to.
#[derive(Clone, Copy, Debug)]
enum Surface {
    Spoken,
    Chat,
}

impl Surface {
    fn node(self) -> &'static str {
        match self {
            Surface::Spoken => "talky",
            Surface::Chat => "talky-chat",
        }
    }
    fn other(self) -> &'static str {
        match self {
            Surface::Spoken => "talky-chat",
            Surface::Chat => "talky",
        }
    }
}

/// The shipped template, copied the way instantiation lays it out (the reader of
/// `gh889_a_turn_runs_collector_curator_brain.rs`): a ref marker is replaced by the
/// referenced template's tree and its `override_params` are applied to the cells they
/// name, which is what the mutation door does to a staged tree.
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// The local provider stubs of one run: the brain of the surface under test, the
/// core's brain, and everything else.
struct Stubs {
    surface: String,
    core: String,
    background: String,
}

/// Point EVERY `llm` cell of the tree at a stub. Returns the cells it pointed.
fn point_llms_at_stubs(main: &std::path::Path, surface: Surface, stubs: &Stubs) -> Vec<String> {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let surface_brain = format!("assistant/{}/brain", surface.node());
    let mut pointed = Vec::new();
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(main)
            .expect("under main")
            .to_string_lossy()
            .replace('\\', "/");
        let (url, model) = if rel == surface_brain {
            (&stubs.surface, BRAIN_MODEL)
        } else if rel == "assistant/cogny/brain" {
            (&stubs.core, BRAIN_MODEL)
        } else {
            (&stubs.background, BACKGROUND_MODEL)
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
        pointed.push(rel);
    }
    pointed.sort();
    pointed
}

/// Every timer of the tree out of the run's way (swept, never named).
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0894_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// Every `${VAR}` the tree references bound to a dummy, the endpoints to the stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, stubs: &Stubs) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    vars.insert("MEMORY_LLM_BASE_URL".into(), stubs.background.clone());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", stubs.background),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// Every lane the assistant declares at its rim, connect points included: a lane that
/// docks inside still leaves through the rim when nothing draws its v-lane, and an
/// undrained lane is a dead letter the assertion below would then be reading.
fn every_emit() -> Vec<String> {
    shipped_json("templates/assistant/config.json")["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

fn drains(from: &str, lanes: &[String]) -> Vec<Value> {
    lanes
        .iter()
        .map(|lane| {
            let to = if lane == "answer" { "/sink" } else { "/park" };
            json!({"from": from, "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

/// The tool hive's stand-in: it hears and says nothing. Nothing on this road calls a
/// tool of the hive, and its menu half is not what this file measures.
fn silent_tools() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "external_timeout_ms": 10000,
                   "script_inline": "import sys, json\njson.load(sys.stdin)\nsys.stdout.write('[]')\n"},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the tool hive: it answers nothing.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The level, its occupants resolved -- all but `./tools`, whose silent stand-in is
/// written in its place without the tool hive ever being read: it is not part of this
/// road, and not every tree that carries the assistant carries the hive.
fn copy_level(dst: &std::path::Path) {
    let src = repo("templates/assistant");
    std::fs::create_dir_all(dst).expect("mkdir");
    std::fs::copy(src.join("config.json"), dst.join("config.json")).expect("copy");
    for entry in std::fs::read_dir(&src).expect("readable") {
        let entry = entry.expect("entry");
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name == "tools" {
            write_json(&dst.join("tools/config.json"), &silent_tools());
        } else {
            copy_resolved(&entry.path(), &dst.join(name), 0);
        }
    }
}

/// The tool hive's other stand-in: it answers every call after `delay_ms` -- the time
/// a search takes -- with ONE `tool_result` under the call's id, and says nothing to a
/// menu question. It holds a tool round open for as long as the lock needs one.
fn slow_tools(delay_ms: u64) -> Value {
    let script = concat!(
        "import sys, json, time\n",
        "doc = json.load(sys.stdin)\n",
        "hop = ((doc.get('envelope') or {}).get('header') or {}).get('hop') or {}\n",
        "if hop.get('route') != 'tool_call':\n",
        "    sys.stdout.write('[]')\n",
        "    sys.exit(0)\n",
        "calls = [m for m in ((doc.get('body') or {}).get('messages') or [])\n",
        "         if isinstance(m, dict) and m.get('type') == 'tool_call']\n",
        "cid = str(hop.get('tool_call_id') or (calls[0].get('id') if calls else '') or '')\n",
        "time.sleep(DELAY_MS / 1000.0)\n",
        "sys.stdout.write(json.dumps({'header': {'route': 'tool_result',\n",
        "    'operation': str(hop.get('tool_name') or ''), 'tool_call_id': cid},\n",
        "    'messages': [{'origin': 'tool', 'type': 'tool_result', 'id': cid,\n",
        "                  'text': 'Three places near the river.'}]}))\n",
    );
    let mut v = silent_tools();
    v["params"]["script_inline"] = json!(script.replace("DELAY_MS", &delay_ms.to_string()));
    v["description"]["purpose"] =
        json!("Test stand-in for the tool hive: one late result per call.");
    v
}

/// The shipped assistant grown into a parent that drains its rim.
fn build(td: &tempfile::TempDir, surface: Surface, stubs: &Stubs) -> Vec<String> {
    build_with_tools(td, surface, stubs, None)
}

/// [`build`], with another stand-in in the tool hive's place.
fn build_with_tools(
    td: &tempfile::TempDir,
    surface: Surface,
    stubs: &Stubs,
    tools: Option<Value>,
) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_level(&main.join("assistant"));
    if let Some(tools) = tools {
        write_json(&main.join("assistant/tools/config.json"), &tools);
    }
    for node in ["talky", "talky-chat"] {
        override_params_on_disk(
            &main.join(format!("assistant/{node}/collector/assemble")),
            &json!({"memory_tier": "", "brief_slots": [], "late_after_ms": LATE_AFTER_MS}),
        );
    }
    let edges = drains("./assistant", &every_emit());
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, surface, stubs);
    write_env(root, &main, stubs);
    pointed
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every delivery into a
    /// send error.
    park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(512);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped assistant, its three brains and their curators must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// The mutation receipt that makes every collector of the level ask for its menu.
fn menu_tick() -> Message {
    MessageBuilder::new(Path::new("/assistant"))
        .hop(map(json!({"route": "mutation_committed"})))
        .body(Body::Inline(json!({"messages": []})))
        .ttl(400)
        .build()
}

/// A person's turn at the level's door, with the round the channel stamps.
fn person(surface: Surface, turn_id: &str, text: &str) -> Message {
    let mut ctx = json!({"channel": CHANNEL, "audience_set": AUDIENCE, "turn_id": turn_id});
    if matches!(surface, Surface::Chat) {
        ctx["channel_node"] = json!("chat");
    }
    MessageBuilder::new(Path::new("/assistant"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn said(m: &Message) -> String {
    body_of(m)["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One routing hop as the colony logged it.
#[derive(Debug)]
struct Logged {
    from: String,
    to: String,
    hop: Value,
    context: Value,
}

impl Logged {
    fn route(&self) -> String {
        text_of(&self.hop, "route")
    }
}

fn message_log(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, headers)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        Logged {
            from,
            to,
            hop: h["hop"].clone(),
            context: h["context"].clone(),
        }
    })
    .collect()
}

/// `(error_code, sender, resolved_target, hop.route)` of every dead letter the colony
/// recorded. None is expected: the level drains its inner lanes, and this harness
/// drains every lane the level declares.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT error_code, sender_path, resolved_target, message_json \
             FROM dead_letters ORDER BY id",
        )
        .expect("dead_letters");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(code, sender, target, msg)| {
        let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
        (code, sender, target, text_of(&m["headers"]["hop"], "route"))
    })
    .collect()
}

/// The first column of one query against a cell's own `cell.db`; a store that has not
/// woken yet has no table, which is the honest "nothing written yet".
fn column(db: &std::path::Path, sql: &str) -> Vec<String> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    st.query_map([], |r| r.get::<_, Option<String>>(0))
        .map(|rows| -> Vec<String> { rows.filter_map(Result::ok).flatten().collect() })
        .unwrap_or_default()
}

/// Wait until both menus stand in the curators' ledgers: the surface's with the
/// voices' names, the core's with its own. Every brain call after that carries them.
async fn wait_for_menus(root: &std::path::Path, surface: Surface) {
    let surface_db = root.join(format!(
        "main/assistant/{}/curator/ledger/cell.db",
        surface.node()
    ));
    let core_db = root.join("main/assistant/cogny/curator/ledger/cell.db");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let s = column(&surface_db, "SELECT path FROM slots");
        let c = column(&core_db, "SELECT path FROM slots");
        if s.iter().any(|p| p == "tools.reply_to_consult")
            && c.iter().any(|p| p == "tools.ask_requester")
        {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "the menus did not reach the curators within {DEADLINE:?}: surface slots \
                 {s:?}, core slots {c:?}. Dead letters: {:#?}",
                dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The next answer that leaves the level and says `needle`, with every answer seen on
/// the way kept in `seen`.
async fn answer_saying(
    ports: &mut Ports,
    root: &std::path::Path,
    needle: &str,
    seen: &mut Vec<Message>,
) -> Message {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.sink.recv()).await {
            Ok(Some(m)) => {
                let hit = said(&m).contains(needle);
                seen.push(m.clone());
                if hit {
                    return m;
                }
            }
            _ => panic!(
                "no answer saying {needle:?} left the level within {DEADLINE:?}. Answers so \
                 far: {:#?}. Dead letters: {:#?}",
                seen.iter().map(said).collect::<Vec<_>>(),
                dead_letters(root)
            ),
        }
    }
}

fn wire(req: &OpenAiRequestSnapshot) -> String {
    meclaw_core::serde_json::to_string(req.messages().expect("wire messages")).unwrap_or_default()
}

fn tools_offered(req: &OpenAiRequestSnapshot) -> Vec<String> {
    let mut v: Vec<String> = req
        .tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            t["function"]["name"]
                .as_str()
                .or_else(|| t["name"].as_str())
                .map(str::to_string)
        })
        .collect();
    v.sort();
    v
}

fn assert_offers(reqs: &[OpenAiRequestSnapshot], who: &str, must: &[&str], never: &[&str]) {
    for (i, r) in reqs.iter().enumerate() {
        let offered = tools_offered(r);
        for n in must {
            assert!(
                offered.iter().any(|o| o == n),
                "{who} request {i} does not offer `{n}`: {offered:?}"
            );
        }
        for n in never {
            assert!(
                !offered.iter().any(|o| o == n),
                "{who} request {i} offers `{n}`, which is not its to call: {offered:?}"
            );
        }
    }
}

/// consult -> ask back -> the person answers late -> reply -> the core's long answer ->
/// the answer leaves without the id it quoted -> a follow-up under the same id.
async fn a_consult_asks_back_under_its_id(surface: Surface) {
    let long = long_answer();
    let voice = MockOpenAI::start(vec![
        // T1: a sentence for the person and the consult, in one breath (#28).
        canned_content_and_tool_calls(INTERIM_1, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        // The core's question arrives as an advice: the surface asks the person.
        canned_chat_completion(QUESTION_BACK, "stop"),
        // T2: the person answered; the surface replies to the core.
        canned_content_and_tool_calls(INTERIM_2, vec![(REPLY_ID, "reply_to_consult", REPLY_ARGS)]),
        // The core's answer arrives; the surface says it, quoting a block id.
        canned_chat_completion(FINAL_2, "stop"),
        // T3: a follow-up under the same consult id.
        canned_content_and_tool_calls(INTERIM_3, vec![(FOLLOW_ID, "consult_cogny", FOLLOW_ARGS)]),
        canned_chat_completion(FINAL_3, "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![
        canned_tool_calls(vec![(ASK_ID, "ask_requester", ASK_ARGS)]),
        canned_chat_completion(&long, "stop"),
        canned_chat_completion(BUDGET, "stop"),
    ])
    .await;
    let background = MockOpenAI::start(
        (0..16)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build(&td, surface, &stubs);
    for brain in [
        format!("assistant/{}/brain", surface.node()),
        "assistant/cogny/brain".to_string(),
    ] {
        assert!(
            pointed.contains(&brain),
            "{brain} is not an llm cell: {pointed:?}"
        );
    }
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    h.send(menu_tick()).await;
    wait_for_menus(&root, surface).await;

    let mut seen: Vec<Message> = Vec::new();
    h.send(person(surface, "t-1", T1)).await;
    answer_saying(&mut ports, &root, INTERIM_1, &mut seen).await;
    let asked = answer_saying(&mut ports, &root, QUESTION_BACK, &mut seen).await;
    assert_eq!(
        asked.headers.hop.get("turn_id").and_then(Value::as_str),
        Some("t-1"),
        "the core's question is spoken in the turn that consulted: {:?}",
        asked.headers.hop
    );

    // The person thinks -- past the consult's deadline.
    tokio::time::sleep(THINKING).await;
    h.send(person(surface, "t-2", T2)).await;
    answer_saying(&mut ports, &root, INTERIM_2, &mut seen).await;
    let planned = answer_saying(&mut ports, &root, "Here is your plan", &mut seen).await;

    h.send(person(surface, "t-3", T3)).await;
    answer_saying(&mut ports, &root, INTERIM_3, &mut seen).await;
    answer_saying(&mut ports, &root, FINAL_3, &mut seen).await;
    tokio::time::sleep(SETTLE).await;

    let log = message_log(&root);
    let dead = dead_letters(&root);
    let mut parked = Vec::new();
    while let Ok(m) = ports.park.try_recv() {
        parked.push(Value::Object(m.headers.hop.clone()));
    }
    let voice_reqs = voice.recorded_requests().await;
    let core_reqs = core.recorded_requests().await;
    h.shutdown().await;

    // ── the provider side: six calls of the surface, three of the core ──
    assert_eq!(
        voice_reqs.len(),
        6,
        "T1, the question back, T2, the core's answer, T3, the follow-up's answer"
    );
    assert_eq!(core_reqs.len(), 3, "the errand, the reply, the follow-up");

    // The question back reaches the surface's model under the consult's id.
    let back = wire(&voice_reqs[1]);
    assert!(
        back.contains(ASKED),
        "the surface's request does not carry the core's question: {back}"
    );
    assert!(
        back.contains(&format!("consult {CONSULT_ID}")),
        "the question is framed under the consult's id, the id the reply passes back: {back}"
    );

    // The reply reaches the core beside its first errand.
    let replied = wire(&core_reqs[1]);
    assert!(
        replied.contains(T2),
        "the core's request lacks the answer: {replied}"
    );
    assert!(
        replied.contains(FIRST_ERRAND),
        "the core continues its errand -- the first order stands in its window: {replied}"
    );

    // No cut: the core's answer reaches the surface's model whole.
    assert!(
        wire(&voice_reqs[3]).contains(&long),
        "the core's {LONG_CHARS}-character answer did not reach the surface whole"
    );

    // The answer the person reads carries no block id -- and it belongs to the turn
    // the person gave the answer in, although that came past the consult's deadline:
    // the reply is a handoff and leaves a departure of its own (#728).
    assert_eq!(said(&planned), FINAL_2_CLEAN, "{:#?}", body_of(&planned));
    assert_eq!(
        planned.headers.hop.get("turn_id").and_then(Value::as_str),
        Some("t-2"),
        "the core's answer after the reply is spoken in the turn of the reply: {:?}",
        planned.headers.hop
    );
    for m in &seen {
        assert!(
            !said(m).contains("[#"),
            "a block id left the level: {}",
            said(m)
        );
    }

    // The follow-up reaches a core that still holds the first errand.
    let followed = wire(&core_reqs[2]);
    assert!(followed.contains(FOLLOW_ERRAND), "{followed}");
    assert!(
        followed.contains(FIRST_ERRAND),
        "the follow-up's core does not hold the first consultation: {followed}"
    );

    // Each side is offered its own half of the contract.
    assert_offers(
        &voice_reqs,
        surface.node(),
        &["consult_cogny", "reply_to_consult"],
        &["ask_requester"],
    );
    assert_offers(
        &core_reqs,
        "cogny",
        &["ask_requester"],
        &["consult_cogny", "reply_to_consult"],
    );

    // ── the hops, as the colony logged them ──
    let at_surface = format!("/assistant/{}", surface.node());
    let asks: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == at_surface && r.route() == "in_advice")
        .filter(|r| text_of(&r.context, "consult_class") == "ask")
        .collect();
    assert_eq!(asks.len(), 1, "one question back: {asks:#?}");
    assert_eq!(
        text_of(&asks[0].context, "consult_id"),
        CONSULT_ID,
        "{asks:#?}"
    );
    assert_eq!(text_of(&asks[0].context, "col_phase"), "", "{asks:#?}");
    assert!(asks[0].from.starts_with("/assistant/cogny"), "{asks:#?}");
    let other = format!("/assistant/{}", surface.other());
    assert!(
        !log.iter()
            .any(|r| r.to == other && r.route() == "in_advice"),
        "the question went to the surface that did NOT ask"
    );

    let replies: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/assistant/cogny" && r.route() == "in_turn")
        .filter(|r| text_of(&r.hop, "tool_name") == "reply_to_consult")
        .collect();
    assert_eq!(replies.len(), 1, "one reply: {replies:#?}");
    assert_eq!(text_of(&replies[0].context, "consult_id"), CONSULT_ID);
    assert_eq!(text_of(&replies[0].context, "consult_class"), "reply");
    assert!(replies[0].context.get("turn_id").is_none(), "{replies:#?}");

    let consults: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/assistant/cogny" && r.route() == "in_turn")
        .filter(|r| text_of(&r.hop, "tool_name") == "consult_cogny")
        .collect();
    assert_eq!(
        consults.len(),
        2,
        "the errand and its follow-up: {consults:#?}"
    );
    for c in &consults {
        assert_eq!(
            text_of(&c.context, "consult_id"),
            CONSULT_ID,
            "a follow-up travels under the first consult's id: {c:#?}"
        );
    }

    let errors: Vec<&Value> = parked
        .iter()
        .filter(|hop| hop["route"].as_str() == Some("error"))
        .collect();
    assert!(errors.is_empty(), "an error left the level: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_asks_back_under_its_id_on_the_spoken_surface() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    a_consult_asks_back_under_its_id(Surface::Spoken).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consult_asks_back_under_its_id_on_the_chat_surface() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    a_consult_asks_back_under_its_id(Surface::Chat).await;
}

/// 4. No consult answer is cut: 20 000 characters from the core reach the asking
/// surface's model whole. Measured at the receiver -- the request its stub recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_consult_answer_is_cut() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let long = long_answer();
    let voice = MockOpenAI::start(vec![
        canned_content_and_tool_calls(INTERIM_1, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        canned_chat_completion("Here it is.", "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![canned_chat_completion(&long, "stop")]).await;
    let background = MockOpenAI::start(
        (0..16)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, Surface::Spoken, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;

    let mut seen = Vec::new();
    h.send(person(Surface::Spoken, "t-1", T1)).await;
    answer_saying(&mut ports, &root, "Here it is.", &mut seen).await;
    let dead = dead_letters(&root);
    let reqs = voice.recorded_requests().await;
    h.shutdown().await;

    assert_eq!(reqs.len(), 2, "the consult and the advice round");
    let advice = reqs[1]
        .messages()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m["content"].as_str().map(str::to_string))
        .find(|c| c.contains("PLAN "))
        .unwrap_or_else(|| panic!("no advice in the surface's request: {}", wire(&reqs[1])));
    assert!(
        advice.contains(&long),
        "the advice reached the surface cut: {} of {LONG_CHARS} characters",
        advice.chars().count()
    );
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// 6. Review focus (1): a reply under an id nobody consulted. There is no typed error
/// (OR-KY.Q.7 -- only the surface's collector knows which consultations are open, and
/// a check there would be a collector change): the reply is a handoff like any other,
/// it writes its own departure, the core gets the answer as an errand and answers it,
/// and the advice comes back correlated under the id the reply named. Measured at the
/// receivers: no dead letter, no `error`, the core's request carries the answer, the
/// surface's request frames the advice under that id, and the person hears it in the
/// turn that sent the reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_under_an_unknown_consult_id_comes_back_under_it() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const UNKNOWN: &str = "call-x9";
    const REPLY_X9: &str = r#"{"consult_id": "call-x9", "answer": "Lisbon, in May."}"#;
    const SAID: &str = "Passing that on.";
    const THANKS: &str = "Noted, Lisbon in May.";
    const HEARD: &str = "The core has it now.";
    let voice = MockOpenAI::start(vec![
        canned_content_and_tool_calls(SAID, vec![("call-r9", "reply_to_consult", REPLY_X9)]),
        canned_chat_completion(HEARD, "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![canned_chat_completion(THANKS, "stop")]).await;
    let background = MockOpenAI::start(
        (0..16)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, Surface::Spoken, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;
    h.send(menu_tick()).await;
    wait_for_menus(&root, Surface::Spoken).await;

    let mut seen = Vec::new();
    h.send(person(Surface::Spoken, "t-1", T2)).await;
    answer_saying(&mut ports, &root, SAID, &mut seen).await;
    let heard = answer_saying(&mut ports, &root, HEARD, &mut seen).await;
    tokio::time::sleep(SETTLE).await;
    let log = message_log(&root);
    let dead = dead_letters(&root);
    let mut parked = Vec::new();
    while let Ok(m) = ports.park.try_recv() {
        parked.push(Value::Object(m.headers.hop.clone()));
    }
    let voice_reqs = voice.recorded_requests().await;
    let core_reqs = core.recorded_requests().await;
    h.shutdown().await;

    assert_eq!(core_reqs.len(), 1, "the reply is one errand for the core");
    assert!(
        wire(&core_reqs[0]).contains(T2),
        "the core's request lacks the answer: {}",
        wire(&core_reqs[0])
    );
    assert_eq!(voice_reqs.len(), 2, "the reply's turn and the advice's");
    assert!(
        wire(&voice_reqs[1]).contains(&format!("consult {UNKNOWN}")),
        "the advice comes back under the id the reply named: {}",
        wire(&voice_reqs[1])
    );
    assert_eq!(
        heard.headers.hop.get("turn_id").and_then(Value::as_str),
        Some("t-1"),
        "the reply's own departure keys the advice to its turn: {:?}",
        heard.headers.hop
    );
    let advices: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/assistant/talky" && r.route() == "in_advice")
        .collect();
    assert_eq!(advices.len(), 1, "{advices:#?}");
    assert_eq!(text_of(&advices[0].context, "consult_id"), UNKNOWN);
    let errors: Vec<&Value> = parked
        .iter()
        .filter(|hop| hop["route"].as_str() == Some("error"))
        .collect();
    assert!(errors.is_empty(), "an error left the level: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}

/// 7. Review focus (2): two consultations open at once, in ONE conversation -- which
/// is the normal case on this road, not a corner: the reply waits on a person's clock,
/// and the core works on the next errand meanwhile. All consultations of a
/// conversation share the core's session (the consult edge promotes the surface's
/// `session_id`), and a turn that reaches a collector while a tool round of that
/// session is open is DEFERRED (GH #103) -- it waited for a round nobody opened, and
/// rode the next errand's round under that errand's `consult_id`. Since OR-KY-75 the
/// core's collector ships `defer_turns` "0": such a turn opens its own round beside
/// the open one.
///
/// The run: c1 asks back; the person gives the surface a second errand, c2, and the
/// core's c2 round calls a tool that takes its time; while that round is open the
/// person answers c1's question and the reply reaches the core. What must hold, at
/// the receivers: the reply does not wait for good -- a core request carries it, beside
/// the first errand -- and the core's answer to it comes back under c1, never under c2
/// (the correlation rides in the context of whatever opens the round it is answered
/// in); exactly one advice per consultation, no dead letter, no `error`. Two rounds of
/// one core session run side by side here, so the curator's window, pair guard and
/// wall are measured under that too: the core's stub provider refuses a request whose
/// tool calls are not answered in order (`MockOpenAI`), and a refused call is an
/// `error` this lock reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_that_waits_behind_another_consult_is_answered_under_its_own_id() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const SECOND: &str = "And find me a place to eat tonight.";
    const C2: &str = "call-c2";
    const C2_ARGS: &str = r#"{"question": "Find a place to eat tonight.", "context": "Goal: dinner tonight. Facts: none. Constraints: none. Form: one place. Length: one sentence."}"#;
    const SAID_2: &str = "I will ask about that too.";
    const PASSED: &str = "Passed on.";
    /// Long enough for the person's reply to reach the core inside the tool round.
    const TOOL_DELAY_MS: u64 = 4000;
    let voice = MockOpenAI::start(vec![
        canned_content_and_tool_calls(INTERIM_1, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        canned_chat_completion(QUESTION_BACK, "stop"),
        canned_content_and_tool_calls(SAID_2, vec![(C2, "consult_cogny", C2_ARGS)]),
        canned_content_and_tool_calls(INTERIM_2, vec![(REPLY_ID, "reply_to_consult", REPLY_ARGS)]),
        // The two advices, in whichever order they come: the surface says the same.
        canned_chat_completion(PASSED, "stop"),
        canned_chat_completion(PASSED, "stop"),
    ])
    .await;
    let core = MockOpenAI::start(vec![
        canned_tool_calls(vec![(ASK_ID, "ask_requester", ASK_ARGS)]),
        canned_tool_calls(vec![(
            "call-k1",
            "web_search",
            r#"{"query": "dinner tonight"}"#,
        )]),
        canned_chat_completion("A place by the river.", "stop"),
        canned_chat_completion("Three days in Lisbon, planned.", "stop"),
    ])
    .await;
    let background = MockOpenAI::start(
        (0..16)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let stubs = Stubs {
        surface: voice.base_url.clone(),
        core: core.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build_with_tools(
        &td,
        Surface::Spoken,
        &stubs,
        Some(slow_tools(TOOL_DELAY_MS)),
    );
    let root = td.path().to_path_buf();
    let (h, mut ports) = boot(&td).await;
    h.send(menu_tick()).await;
    wait_for_menus(&root, Surface::Spoken).await;

    let mut seen = Vec::new();
    h.send(person(Surface::Spoken, "t-1", T1)).await;
    answer_saying(&mut ports, &root, INTERIM_1, &mut seen).await;
    answer_saying(&mut ports, &root, QUESTION_BACK, &mut seen).await;

    // The second errand, and its round's tool call out at the (slow) tool hive.
    h.send(person(Surface::Spoken, "t-2", SECOND)).await;
    answer_saying(&mut ports, &root, SAID_2, &mut seen).await;
    let deadline = Instant::now() + DEADLINE;
    while !message_log(&root)
        .iter()
        .any(|r| r.to == "/assistant/tools" && r.route() == "tool_call")
    {
        assert!(
            Instant::now() < deadline,
            "the core's second round never called its tool"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The person answers the FIRST question while that round is open.
    h.send(person(Surface::Spoken, "t-3", T2)).await;
    answer_saying(&mut ports, &root, INTERIM_2, &mut seen).await;

    // The reply must reach the core's model -- not wait for an errand that never comes.
    let deadline = Instant::now() + DEADLINE;
    let replied = loop {
        let reqs = core.recorded_requests().await;
        if let Some(r) = reqs.iter().find(|r| wire(r).contains(T2)) {
            break wire(r);
        }
        if Instant::now() >= deadline {
            let log = message_log(&root);
            let reply_at = log.iter().position(|r| {
                r.to == "/assistant/cogny" && text_of(&r.hop, "tool_name") == "reply_to_consult"
            });
            let result_at = log
                .iter()
                .position(|r| r.from == "/assistant/tools" && r.to == "/assistant/cogny");
            panic!(
                "the reply to the core's question never reached the core's model within \
                 {DEADLINE:?}: it arrived while the second consultation's tool round was \
                 open (reply at log row {reply_at:?}, the tool's result at {result_at:?}). \
                 A collector that defers it (`defer_turns` not \"0\") opens no round for it \
                 after that round answers -- it waits for the NEXT errand of the \
                 conversation and is answered under that errand's consult_id. Core \
                 requests: {}. Dead letters: {:#?}",
                reqs.len(),
                dead_letters(&root)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        replied.contains(FIRST_ERRAND),
        "the core answers the reply with the first errand in its window: {replied}"
    );
    answer_saying(&mut ports, &root, PASSED, &mut seen).await;
    answer_saying(&mut ports, &root, PASSED, &mut seen).await;
    tokio::time::sleep(SETTLE).await;
    let log = message_log(&root);
    let dead = dead_letters(&root);
    let mut parked = Vec::new();
    while let Ok(m) = ports.park.try_recv() {
        parked.push(Value::Object(m.headers.hop.clone()));
    }
    h.shutdown().await;

    // The precondition this lock exists for: the reply reached the core BEFORE the
    // tool's result did, i.e. inside the open round.
    let reply_at = log
        .iter()
        .position(|r| {
            r.to == "/assistant/cogny" && text_of(&r.hop, "tool_name") == "reply_to_consult"
        })
        .expect("the reply reached the core");
    let result_at = log
        .iter()
        .position(|r| r.from == "/assistant/tools" && r.to == "/assistant/cogny")
        .expect("the tool answered");
    assert!(
        reply_at < result_at,
        "the reply came after the tool round closed -- this run measured nothing \
         (reply at {reply_at}, result at {result_at})"
    );

    // One advice per consultation, each under its own id.
    let answers: Vec<&Logged> = log
        .iter()
        .filter(|r| r.to == "/assistant/talky" && r.route() == "in_advice")
        .filter(|r| r.from.starts_with("/assistant/cogny"))
        .filter(|r| text_of(&r.context, "consult_class") != "ask")
        .collect();
    let mut ids: Vec<String> = answers
        .iter()
        .map(|r| text_of(&r.context, "consult_id"))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![CONSULT_ID.to_string(), C2.to_string()],
        "one answer per consultation, each under its own id: {answers:#?}"
    );
    for r in &answers {
        if text_of(&r.context, "consult_class") == "reply" {
            assert_eq!(
                text_of(&r.context, "consult_id"),
                CONSULT_ID,
                "the answer to the reply came back under another consultation: {r:#?}"
            );
        }
    }
    let errors: Vec<&Value> = parked
        .iter()
        .filter(|hop| hop["route"].as_str() == Some("error"))
        .collect();
    assert!(errors.is_empty(), "an error left the level: {errors:#?}");
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}
