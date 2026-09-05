//! GH #464 — the CALLER side: a cell that uses tools declares them, asks the
//! tools hive for exactly those names, and the answer becomes its menu.
//!
//! The hive half shipped with `tools@1.2.0` and is pinned by
//! `gh464_the_hive_hands_out_its_own_declarations.rs`: `in_schemas` in,
//! `tool_schemas` out, provider-neutral, an unknown name named. This file is the
//! other half of the same ruling — *the template is the contract* — measured on
//! the shipped tree:
//!
//! 1. **The declaration is a param of the template.** `talky` declares
//!    `["web_search", "web_fetch"]`, `cogny` declares `["*"]`, and both names of
//!    the first list exist in the hive that answers them. A declaration pointing
//!    at nothing is the defect this lane exists to make visible, so the shipped
//!    one is checked against the shipped table rather than trusted.
//! 2. **The assembler asks, and knows when not to.** A tick with a declaration
//!    produces one `schemas` request and nothing else; a tick with no
//!    declaration, or with a typed `tool_menu`, produces silence — the knob is
//!    the manual override and two writers on `system.tools` would fight.
//! 3. **The answer becomes a provider-native menu.** The hive answers
//!    `{name, description, parameters}`; the caller wraps it, because the caller
//!    is the one that knows its provider. `$replace` on the subtree, one leaf per
//!    tool, the JSON verbatim in `text` — the same shape a typed `tool_menu`
//!    produces, read by the same `fn_of`.
//! 4. **An unknown name is a receipt, not a silence.** It rides in
//!    `hop.menu_unknown` and on stderr, which a `code` cell puts into
//!    `log.jsonl` at warn level; the schemas that WERE found travel beside it.
//! 5. **Nothing usable writes nothing.** `$replace` makes an empty menu a
//!    revocation of the model's whole tool set, so an answer with no usable
//!    declaration in it emits no `menu` message at all.
//! 6. **End to end on a booted colony.** The shipped `talky` beside the shipped
//!    `tools`, wired with the pair the `assistant` level draws: the tick fires by
//!    itself, and exactly the two declared schemas land in the brain's OWN
//!    `cell.db` — the durable signal, never an empty dead-letter queue.
//! 7. **And `["*"]` means all of them.** The shipped `cogny` asks for everything
//!    the hive has, and both of its brains get the same menu.
//!
//! Free of a real provider by construction: the brains talk to a mock OpenAI
//! wire, and on this lane they are expected never to talk at all.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{code_stdin, emit_all, run_shipped_script, shipped_script};
use mock_openai::MockOpenAI;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(name: &str) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    root.join("config.json").exists().then_some(root)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);
const SCHEMAS_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/schemas/config.json"
);

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// GH #277: a `cell.type: "ref"` directory is a REFERENCE — the referenced
/// template's tree belongs in its place.
fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION substitution, and the
/// keeper's night schedule still carries one; the menu clock ships a literal.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000464";
/// Never during a test run.
const NEVER: &str = "0 0 0 1 1 *";

// ═════════════════════════════════════════ 1. the declaration is the template's

/// The two composites declare what they use, and they declare different things
/// on purpose: a channel voice gets a named, small surface, a reasoning core
/// gets everything its surface can reach.
#[test]
fn the_shipped_composites_declare_what_they_use() {
    let Some(talky) = shipped("talky") else {
        return;
    };
    let Some(cogny) = shipped("cogny") else {
        return;
    };

    let talky_tools =
        read_json(&talky.join("collector/config.json"))["override_params"]["assemble"]["tools"]
            .clone();
    assert_eq!(
        talky_tools,
        json!(["web_search", "web_fetch"]),
        "talky's collector ref must carry the composite's own declaration — the tool \
         names the agent uses live where the rest of its contract lives, not in a \
         subscriber table in the hive that answers"
    );

    let cogny_tools =
        read_json(&cogny.join("collector/config.json"))["override_params"]["assemble"]["tools"]
            .clone();
    assert_eq!(
        cogny_tools,
        json!(["*"]),
        "the reasoning core declares EVERYTHING the hive has, and that is a decision: a \
         list typed here would be a second copy of a catalogue that drifts on the first \
         tool added"
    );
}

/// The drift lock over the declaration (`docs/development-rules.md` § 2d): the
/// names `talky` declares are names the shipped hive actually has. A declaration
/// pointing at nothing is exactly the defect the `unknown[]` lane reports, so
/// the shipped one must not be an instance of it.
#[test]
fn every_name_the_shipped_talky_declares_exists_in_the_hive_that_answers() {
    let Some(talky) = shipped("talky") else {
        return;
    };
    let Some(_tools) = shipped("tools") else {
        return;
    };

    let declared: Vec<String> = read_json(&talky.join("collector/config.json"))["override_params"]
        ["assemble"]["tools"]
        .as_array()
        .expect("the declaration is a list")
        .iter()
        .map(|v| v.as_str().expect("a tool name is a string").to_string())
        .collect();

    let answer = ask_the_hive(&json!(declared));
    let unknown = answer["unknown"]
        .as_array()
        .expect("the hive always answers with an unknown list");
    assert!(
        unknown.is_empty(),
        "the shipped talky declares {declared:?}, and the shipped tools hive has no \
         declaration for {unknown:?}. Either the tool left the hive or the name is a \
         typo; both are the failure this lane exists to show, and neither belongs in \
         the shipped tree"
    );
    let names = schema_names(&answer);
    assert_eq!(
        names, declared,
        "the hive answers in the order asked, and every declared name has a schema"
    );
}

/// The drift lock over the PROSE (`docs/development-rules.md` § 2d): the three
/// public template surfaces promise this lane in words, and each promise is
/// grepped here beside the mechanism that carries it. A grep alone pins a
/// string; an assertion alone lets the prose walk away from it.
#[test]
fn the_public_surfaces_say_what_the_lane_does_and_the_lane_does_it() {
    let Some(collector) = shipped("collector") else {
        return;
    };
    let readme = std::fs::read_to_string(collector.join("README.md")).expect("the README ships");
    for needle in [
        "The menu is asked for, not typed",
        "`params.tools` is a list of **names**",
        "one write per change and nothing per turn",
    ] {
        assert!(
            readme.contains(needle),
            "templates/collector/README.md no longer carries `{needle}` — the prose this              lock reads was reworded. Move the lock with it: a lock that silently stops              finding its sentence pins nothing"
        );
    }

    // The mechanism half. The knob exists, the shipped default is silence, and
    // the two lanes the prose names are the two the hive declares.
    let assemble = read_json(&collector.join("assemble/config.json"));
    assert_eq!(
        assemble["params"]["tools"],
        json!([]),
        "the shipped default asks nothing at all"
    );
    assert_eq!(
        assemble["contract"]["settings"]["tools"]["default"],
        json!([]),
        "params and contract.settings.default are one value in two places"
    );
    let cfg = read_json(&collector.join("config.json"));
    let accepts: Vec<&str> = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    let emits: Vec<&str> = cfg["params"]["contract"]["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .filter_map(|e| e["route"].as_str())
        .collect();
    assert!(
        accepts.contains(&"in_menu"),
        "the answer has a door: {accepts:?}"
    );
    assert!(
        emits.contains(&"schemas"),
        "the question has an exit: {emits:?}"
    );
    assert!(emits.contains(&"menu"), "and so has the menu: {emits:?}");
    assert!(
        !accepts.contains(&"in_menu_tick"),
        "the tick is INTERNAL -- the hive's own edge into `./assemble` names it, and a          lane nothing outside can send is not a door. What a caller says is          `mutation_committed` (GH #553): {accepts:?}"
    );
}

/// GH #553 — the ask has a CAUSE now, and the cause is a mutation.
///
/// The cadence used to be a number in two places (`MENU_CRON` in
/// `./menu-clock`, the same number spelled out in the README), and the number
/// was a guess: a poll asks whether anything changed, on a schedule that has no
/// relationship to when anything does. The receipt says it instead — so what is
/// derived here is not a cadence but a CHAIN: the caller's lane, the door it
/// finds, and the internal lane it becomes.
#[test]
fn the_menu_is_asked_on_the_mutation_receipt_and_not_on_a_clock() {
    let Some(collector) = shipped("collector") else {
        return;
    };
    assert!(
        !collector.join("menu-clock").exists(),
        "the poll timer is back in `templates/collector`; the menu follows the \
         mutation receipt since GH #553"
    );
    let cfg = read_json(&collector.join("config.json"));
    let accepts: Vec<&str> = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    assert!(
        accepts.contains(&"mutation_committed"),
        "the receipt needs a door of its own -- it comes from OUTSIDE the hive, \
         which is exactly what the tick never did: {accepts:?}"
    );
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let door = edges
        .iter()
        .find(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'mutation_committed'"))
        })
        .expect("the receipt lane has a door into the hive");
    assert_eq!(door["from"], json!("."), "it enters at the hive path");
    assert_eq!(door["to"], json!("./assemble"), "and reaches the assembler");
    assert_eq!(
        door["modifier"]["set_hop"]["route"],
        json!("'in_menu_tick'"),
        "the door is where the caller's lane becomes the hive's own -- the same \
         shape `./menu-clock -> ./assemble` had, minus the clock"
    );
    // The lane keeps ONE name from the mutation door down to this rim, which is
    // what lets every level in between declare it and be a mandatory hop for it
    // — and it deliberately does NOT start with `in_`, so the generic inbound
    // door cannot carry it as well and hand the assembler the same message twice.
    for composite in ["talky", "cogny"] {
        let Some(root) = shipped(composite) else {
            continue;
        };
        let gen_cfg = read_json(&root.join("config.json"));
        let lanes: Vec<&str> = gen_cfg["params"]["contract"]["accepts"]
            .as_array()
            .expect("accepts")
            .iter()
            .filter_map(|a| a["route"].as_str())
            .collect();
        assert!(
            lanes.contains(&"mutation_committed"),
            "{composite} has to take the receipt at its rim and carry it to its \
             collector; the lane is not renamed on the way: {lanes:?}"
        );
    }
    let readme = std::fs::read_to_string(collector.join("README.md")).expect("the README ships");
    assert!(
        readme.contains("`mutation_committed`"),
        "the README no longer names the lane the menu is asked on"
    );
}

// ══════════════════════════════════ 2. the assembler asks, and knows when not to

/// The tick, over the shipped assembler, run the way the substrate runs it.
fn tick(params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(ASSEMBLE),
        &json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "in_menu_tick"}, "context": {}},
            "ttl": 64,
            "messages": [],
            "params": params,
        }),
    )
}

/// One question at the shipped `tools/schemas` cell, the way its own hive asks it.
fn ask_the_hive(names: &Value) -> Value {
    let out = emit_all(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": names,
            "messages": [],
        }),
    );
    assert_eq!(out.len(), 1, "the hive answers once");
    out.into_iter().next().expect("one answer")
}

fn schema_names(answer: &Value) -> Vec<String> {
    answer["schemas"]
        .as_array()
        .expect("schemas is an array")
        .iter()
        .map(|s| s["name"].as_str().expect("a schema is named").to_string())
        .collect()
}

#[test]
fn a_tick_asks_for_exactly_the_declared_names_and_nothing_else() {
    let out = tick(&json!({"tools": ["web_search", "web_fetch"]}));
    assert_eq!(out.len(), 1, "a tick produces ONE request: {out:#?}");
    let req = &out[0];
    assert_eq!(req["header"]["route"], "schemas");
    assert_eq!(req["header"]["asked_count"], "2");
    assert_eq!(
        req["tools"],
        json!(["web_search", "web_fetch"]),
        "the request body IS the declaration — the hive keeps no record of who asked, \
         so the caller says what it wants every time: {req:#?}"
    );
    assert_eq!(
        req["messages"],
        json!([]),
        "no turn travels with a question about schemas"
    );
}

#[test]
fn a_collector_that_declares_nothing_asks_nothing() {
    assert!(
        tick(&json!({})).is_empty(),
        "the shipped default is an empty declaration, and it has to be silent: a \
         collector in a colony with no tools hive beside it would otherwise fire one \
         dead letter per tick, for ever"
    );
}

#[test]
fn a_typed_menu_is_the_manual_override_and_switches_the_asking_off() {
    let out = tick(&json!({
        "tools": ["web_search"],
        "tool_menu": "[{\"type\":\"function\",\"function\":{\"name\":\"typed\"}}]",
    }));
    assert!(
        out.is_empty(),
        "`tool_menu` and `tools` write the same `system.tools` path, so one of them has \
         to win — and it is the typed one, because a knob somebody set by hand is an \
         override rather than a second source: {out:#?}"
    );
}

// ═══════════════════════════════════ 3.-5. the answer becomes a provider menu

fn run_assembler(doc: &Value) -> (Vec<Value>, String) {
    let out = run_shipped_script(&shipped_script(ASSEMBLE), &doc.to_string());
    assert!(out.status.success(), "the assembler exited non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json out");
    let msgs = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    (msgs, stderr)
}

/// The answer lane, over the shipped assembler, driven with what the shipped
/// hive actually answers — not with a hand-written fixture, so that a change to
/// either side shows up here.
///
/// Since GH #529 the lane is TWO steps, because the menu is a union over
/// answerers and a union needs a memory: the answer is recorded as one row of
/// the collector's own store, and the write is derived from every stored row.
/// This helper drives both with ONE answerer, which is the shape every claim in
/// this file was written against; the merge itself is
/// `gh529_the_menu_merges_every_answerers_declarations.rs`. Both steps' stderr
/// is joined, because which of the two says a thing is a detail of the round
/// trip and not of the promise.
fn menu_of(declared: &Value) -> (Vec<Value>, String) {
    let answer = ask_the_hive(declared);
    let (recorded, mut stderr) = run_assembler(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"}, "context": {}},
        "ttl": 64,
        "messages": [],
        "schemas": answer["schemas"].clone(),
        "unknown": answer["unknown"].clone(),
        "params": {"tools": declared.clone()},
    })));
    if recorded.is_empty() {
        return (recorded, stderr);
    }
    let leg = recorded[0]["messages"]
        .as_array()
        .expect("a bundle of tool_calls")
        .iter()
        .find(|m| m["id"] == json!("c-menu-put"))
        .expect("the bundle inserts this answerer's submenu");
    let args: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().expect("a tool_call text"))
            .expect("the op is json");
    let rows = json!([args["row"].clone()]);
    let (msgs, more) = run_assembler(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": meclaw_core::serde_json::to_string(&rows).unwrap()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": {"tools": declared.clone()},
    })));
    stderr.push_str(&more);
    (msgs, stderr)
}

#[test]
fn the_answer_is_wrapped_in_the_provider_envelope_the_hive_refuses_to_write() {
    let (out, stderr) = menu_of(&json!(["web_search", "web_fetch"]));
    assert_eq!(out.len(), 1, "one menu message: {out:#?}");
    let msg = &out[0];
    assert_eq!(msg["header"]["route"], "menu");
    // Three, not two, since GH #512: the two declared names plus the one the
    // collector answers ITSELF, which no tools hive has a declaration for.
    // `gh512_the_collector_declares_the_tools_it_answers_itself.rs` owns that
    // half; this file keeps measuring the wrapping and the `$replace`. It was
    // four until GH #552 took `memory_recall` to the hive that answers it.
    assert_eq!(msg["header"]["menu_count"], "3");
    assert_eq!(msg["header"]["menu_self"], "thread_recall");
    assert_eq!(msg["header"]["menu_unknown"], "");
    assert!(
        stderr.is_empty(),
        "a clean answer says nothing at all: {stderr}"
    );
    assert!(
        msg.get("messages").is_none(),
        "the menu carries slots and NO turn — that is the shape an `llm` cell upserts \
         without calling a provider: {msg:#?}"
    );

    let tools = &msg["system"]["tools"];
    assert_eq!(
        tools["$replace"],
        json!(true),
        "the subtree is REPLACED, not merged: a menu upserted leaf by leaf would keep \
         every declaration the hive has since dropped, durably"
    );
    for name in ["web_search", "web_fetch"] {
        let raw = tools[name]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` must be a `text` leaf: {tools:#?}"));
        let decl: Value = meclaw_core::serde_json::from_str(raw).expect("a leaf holds json");
        assert_eq!(
            decl["type"], "function",
            "the hive answers provider-NEUTRAL on purpose and the caller wraps, because \
             the caller is the one that knows its provider: {decl:#?}"
        );
        assert_eq!(decl["function"]["name"], name);
        assert!(
            decl["function"]["parameters"].is_object(),
            "a declaration without `parameters` is one no provider accepts: {decl:#?}"
        );
        assert!(
            decl["function"]["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the description travels with the schema: {decl:#?}"
        );
    }
    let leaves: BTreeSet<&str> = tools
        .as_object()
        .expect("the subtree is an object")
        .keys()
        .filter(|k| *k != "$replace")
        .map(String::as_str)
        .collect();
    assert_eq!(
        leaves,
        ["thread_recall", "web_fetch", "web_search"]
            .into_iter()
            .collect(),
        "exactly the declared names and the one this cell serves itself (GH #512), and no \
         menu somebody else typed"
    );
}

#[test]
fn a_name_the_hive_does_not_have_is_reported_and_the_others_still_arrive() {
    let (out, stderr) = menu_of(&json!(["web_search", "telepathy"]));
    assert_eq!(out.len(), 1, "the partial answer is still an answer");
    let msg = &out[0];
    assert_eq!(
        msg["header"]["menu_count"], "2",
        "one found, one served here"
    );
    assert_eq!(
        msg["header"]["menu_unknown"], "telepathy",
        "a declared name nobody has is NAMED on the message: {msg:#?}"
    );
    assert!(
        stderr.contains("telepathy"),
        "and written where a reader finds it without opening a body — a `code` cell puts \
         its script's stderr into `log.jsonl` at warn level and stamps `had_stderr` on \
         the emission. Got: {stderr:?}"
    );
    assert!(
        msg["system"]["tools"]["web_search"].is_object(),
        "the schema that WAS found travels beside the refusal: {msg:#?}"
    );
}

#[test]
fn an_answer_with_nothing_usable_writes_nothing_at_all() {
    let (out, stderr) = menu_of(&json!(["telepathy"]));
    assert!(
        out.is_empty(),
        "`$replace` makes an empty menu a REVOCATION of the model's whole tool set, so \
         an answer that found nothing writes nothing: {out:#?}"
    );
    assert!(
        stderr.contains("telepathy"),
        "and it still says so: {stderr:?}"
    );
}

// ════════════════════════════════════════ the level's own wiring, on the files

/// The `assistant` level draws the pair once per caller and tells the two apart
/// on the way back the same way a tool result is told apart.
#[test]
fn the_assistant_draws_the_pair_for_both_of_its_callers() {
    let Some(assistant) = shipped("assistant") else {
        return;
    };
    let cfg = read_json(&assistant.join("config.json"));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("the level has a graph")
        .clone();

    let find = |from: &str, to: &str, route: &str| -> Value {
        edges
            .iter()
            .find(|e| {
                e["from"] == from
                    && e["to"] == to
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
            })
            .unwrap_or_else(|| {
                panic!("no edge {from} -> {to} on `{route}` in the shipped assistant")
            })
            .clone()
    };

    for caller in ["./talky", "./cogny"] {
        let out = find(caller, "./tools", "schemas");
        assert_eq!(
            out["modifier"]["set_hop"]["route"], "'in_schemas'",
            "the request is renamed onto the tool surface's own declaration lane: {out:#?}"
        );
        assert!(
            out["default"].as_bool() != Some(true),
            "an ORDINARY edge: a second default out of one sender would compete with the \
             tool exit for every message nothing regular carried: {out:#?}"
        );
        let token = out["modifier"]["set_context"]["tool_caller"]
            .as_str()
            .unwrap_or_else(|| panic!("the request stamps who asked: {out:#?}"));

        let back = find("./tools", caller, "tool_schemas");
        assert_eq!(
            back["modifier"]["set_hop"]["route"], "'in_menu'",
            "and the answer is renamed onto the caller's own lane: {back:#?}"
        );
        let guard = back["condition"].as_str().unwrap_or_default();
        assert!(
            guard.contains("context.tool_caller"),
            "the two callers of one tool surface are told apart on the way back by \
             `context.tool_caller` — context, not hop, because the hop decays at the \
             next cell: {back:#?}"
        );
        let _ = token;
    }
}

/// The pair is PAIRED, and the hive that answers refuses half of it.
#[test]
fn the_answering_hive_pairs_the_two_lanes_in_required_drains() {
    let Some(tools) = shipped("tools") else {
        return;
    };
    let cfg = read_json(&tools.join("config.json"));
    let paired = cfg["params"]["required_drains"]
        .as_array()
        .expect("the tools hive declares its pairings")
        .iter()
        .any(|d| d["accepts"] == "in_schemas" && d["emits"] == "tool_schemas");
    assert!(
        paired,
        "a caller that asks what there is and does not subscribe to the answer starts \
         with an empty menu and no way to learn it is empty"
    );
}

// ══════════════════════════════════════════════ 6.-7. the booted colony

/// The wiring the `assistant` level draws, reduced to the one composite under
/// test and the tool surface beside it. `/park` collects everything else so no
/// capture is ever a closed channel.
fn main_config(composite: &str) -> Value {
    let hive = format!("./{composite}");
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": ".", "to": hive.clone(),
         "condition": "has(hop.route) && hop.route == 'mutation_committed'"},
        {"from": hive.clone(), "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'schemas'",
         "modifier": {"set_hop": {"route": "'in_schemas'"},
                      "set_context": {"tool_caller": "'surface'"}}},
        {"from": "./tools", "to": hive.clone(),
         "condition": "has(hop.route) && hop.route == 'tool_schemas'",
         "modifier": {"set_hop": {"route": "'in_menu'"}}},
        {"from": hive, "to": "/park"},
        {"from": "./tools", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'tool_schemas'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, composite: &str, declared: &Value, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nSEARCH_API_KEY=\n",
    )
    .unwrap();
    // GH #553: the menu is asked for on the MUTATION RECEIPT, and the boot is the
    // first receipt (ruling O-0904-2). Opting in here is what makes the first
    // menu happen at all -- the five-minute poll that used to do it is gone.
    std::fs::write(
        root.join("colony.json"),
        r#"{"schema_version": 1, "mutation_receipts": {"to": "/"}}"#,
    )
    .unwrap();
    write(root, "main/config.json", &main_config(composite));
    copy_cells(
        &templates_root().join(composite),
        &root.join(format!("main/{composite}")),
    );
    copy_cells(&templates_root().join("tools"), &root.join("main/tools"));
    // Every occupant of the tools hive whose cell type is not `code` is replaced
    // by a `code` double. The BOOT PLAN refuses a cell type it does not know
    // before it ever asks whether the node is active -- which is why an unwired
    // occupant used to need a factory too, and since `tools@1.3.0` there is no
    // unwired occupant to provide one for (GH #547). This file is about the
    // declaration lane, not about what the tools themselves do. `schemas` is the
    // one cell it must NOT touch: that one is the shipped answer.
    double_the_tool_cells(&root.join("main/tools"));

    let keeper = root.join(format!("main/{composite}/session-keeper/night/config.json"));
    if keeper.exists() {
        patch(
            root,
            &format!("main/{composite}/session-keeper/night/config.json"),
            |v| {
                v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
                v["params"]["schedules"][0]["cron"] = json!(NEVER);
            },
        );
    }
    // Every open generation is a candidate the moment the sweep runs. It was a
    // `KEEPER_IDLE_MS=0` line in the `.env` above until GH #138; the knob is a
    // param of `./close` now, so such a line would be read by NOTHING -- the
    // sweep would keep the shipped two hours and find no candidate.
    if keeper.exists() {
        patch(
            root,
            &format!("main/{composite}/session-keeper/close/config.json"),
            |v| v["params"]["idle_ms"] = json!(0),
        );
    }
    // `copy_cells` follows the `ref` and copies the referenced template, so the
    // ref's own `override_params` — the composite's declaration — does not
    // travel with it. Writing it here is applying that declaration by hand, and
    // `the_shipped_composites_declare_what_they_use` above is what pins the
    // value being applied.
    // ... and so does every other knob the ref sets, `thread_recall` included: a
    // composite that routes no lane for a name switches the tool off there, and a
    // tree that dropped the override would measure a collector nobody ships.
    let overrides = read_json(
        &templates_root()
            .join(composite)
            .join("collector/config.json"),
    )["override_params"]["assemble"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    patch(
        root,
        &format!("main/{composite}/collector/assemble/config.json"),
        |v| {
            for (k, val) in &overrides {
                v["params"][k] = val.clone();
            }
            v["params"]["tools"] = declared.clone();
        },
    );
    for brain in brains_of(composite) {
        patch(
            root,
            &format!("main/{composite}/{brain}/config.json"),
            |v| {
                v["params"]["base_url"] = json!(base_url);
                v["params"]["model"] = json!("gpt-4o-mock");
            },
        );
    }
}

/// A `code` stand-in for a tool occupant this file never calls.
fn tool_double() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": "import sys, json\nsys.stdin.read()\nsys.stdout.write(json.dumps([]))\n",
            "external_timeout_ms": 10000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}},
                      "hop": {"operation": {"type": "string", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for a tool occupant this file never calls.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Replace every non-`code` occupant of a copied tools hive with the double.
fn double_the_tool_cells(tools_root: &std::path::Path) {
    for entry in std::fs::read_dir(tools_root).expect("the tools hive copied") {
        let dir = entry.expect("a directory entry").path();
        if !dir.is_dir() {
            continue;
        }
        let cfg = dir.join("config.json");
        if !cfg.exists() {
            continue;
        }
        if read_json(&cfg)["cell"]["type"] == "code" {
            continue;
        }
        std::fs::write(
            &cfg,
            meclaw_core::serde_json::to_string_pretty(&tool_double()).unwrap(),
        )
        .unwrap();
    }
}

/// Every shipped composite carries exactly one `llm` cell since `cogny@4.4.0`
/// took the core's lookup lane out ([#528](https://github.com/mmeyerlein/meclaw/issues/528)).
/// The indirection stays: the menu write is a fan-out by construction, and a
/// composite that grows a second brain must not need a second test to notice.
fn brains_of(_composite: &str) -> &'static [&'static str] {
    &["brain"]
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
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
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
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
        .expect("bootstrap_from_filesystem must succeed");
    (h, park_rx)
}

/// The brain's OWN durable state: the `system` table of its `cell.db`. This is
/// the honest signal — what an `llm` cell upserted and what it will concatenate
/// into its next prompt — never an empty dead-letter queue.
fn brain_slots(td: &tempfile::TempDir, composite: &str, brain: &str) -> Vec<(String, String)> {
    let p = td.path().join(format!("main/{composite}/{brain}/cell.db"));
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path, value FROM system ORDER BY slot_path")
    else {
        return Vec::new();
    };
    match stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Poll until the menu has landed. 30s is the failure-marker convention; the
/// tick itself is a second away, and the 20ms step only decides how fast a
/// green test finishes.
async fn await_menu(
    td: &tempfile::TempDir,
    composite: &str,
    brain: &str,
    at_least: usize,
) -> Vec<String> {
    for _ in 0..1500 {
        let names: Vec<String> = brain_slots(td, composite, brain)
            .into_iter()
            .filter_map(|(p, _)| p.strip_prefix("tools.").map(str::to_string))
            .collect();
        if names.len() >= at_least {
            return names;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "no menu ever reached {composite}/{brain}'s own cell.db; it holds {:?}",
        brain_slots(td, composite, brain)
    );
}

/// Claim 6. The shipped talky, beside the shipped tools hive, wired with the
/// pair the assistant level draws: the tick fires by itself and exactly the two
/// declared schemas become durable state of the agent's own brain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_talky_asks_for_its_two_tools_and_gets_exactly_those_two() {
    let (Some(_), Some(_)) = (shipped("talky"), shipped("tools")) else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().unwrap();
    build_tree(
        &td,
        "talky",
        &json!(["web_search", "web_fetch"]),
        &mock.base_url,
    );
    let (_h, _park) = boot(&td).await;

    let mut names = await_menu(&td, "talky", "brain", 3).await;
    names.retain(|n| n != "thread_recall");
    names.sort();
    assert_eq!(
        names,
        vec!["web_fetch".to_string(), "web_search".to_string()],
        "a cell that uses tools declares them, and it gets the ones it declared — not \
         the hive's whole catalogue, and not a list somebody typed into its prompt. The \
         one the collector serves itself is held out here and measured by \
         `gh512_the_collector_declares_the_tools_it_answers_itself.rs`"
    );
}

/// Claim 7. `["*"]` means everything the hive has, and the core's brain gets
/// every one of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cogny_asks_for_everything_and_its_brain_gets_it() {
    let (Some(_), Some(_)) = (shipped("cogny"), shipped("tools")) else {
        return;
    };
    let everything = schema_names(&ask_the_hive(&json!(["*"])));
    assert!(
        everything.len() > 2,
        "the hive under test has to carry more than the talky's two, or this claim \
         measures nothing: {everything:?}"
    );

    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().unwrap();
    build_tree(&td, "cogny", &json!(["*"]), &mock.base_url);
    let (_h, _park) = boot(&td).await;

    for brain in brains_of("cogny") {
        let mut names = await_menu(&td, "cogny", brain, everything.len() + 1).await;
        // The one the collector serves ITSELF (GH #512, `collector@3.3.1`): the
        // shipped cogny routes `thread_recall` by name, and no tools hive has a
        // declaration for it. It is held out here and measured by
        // `gh512_the_collector_declares_the_tools_it_answers_itself.rs`.
        // `memory_recall` stood beside it until GH #552 and is the member's
        // memory's own declaration now — a standalone cogny has no member.
        names.retain(|n| n != "thread_recall");
        names.sort();
        let mut want = everything.clone();
        want.sort();
        assert_eq!(
            names, want,
            "`{brain}` must hold every declaration the hive has -- a reasoning core \
             declares `[\"*\"]` precisely so nothing has to be typed twice"
        );
    }
}
