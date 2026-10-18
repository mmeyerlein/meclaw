//! GH #1267 -- `templates/npc`: one identity, one brain, a curator and a
//! memory for a non-player character.
//!
//! The template is a world adapter (`./world`, the one resident code cell) in
//! front of the tool round of `cogny` (collector -> curator -> brain ->
//! splitter -> dispatcher), a memory hive behind the curator, and its own
//! `./access` with the grants of its seven model consumers. Its rim is the
//! world's contract: `obs` in, `intent` out, `night`, `digest`, `hp`,
//! `hp_reply`, `identity`, `mandate`, `interview`, `in_model`.
//!
//! Two layers, cheapest first:
//!
//! 1. **The file** (no colony): the lanes, the seal, the drains the memory
//!    hive insists on, the identity/mandate leaves the adapter writes, exactly
//!    one thinking `llm`, and the app attribution of every model cell.
//! 2. **The character** (a colony of the instantiated template with stub
//!    models): a move ends in one intent after one brain call, the identity
//!    reaches the brain with its AI disclosure, a night closes the day with a
//!    model call, and the handset round sees the world's move in its window.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry the template or one of its refs is skipped, never judged.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::required_drains::{check_required_drains, collect_required_drains};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::TemplatesRegistry;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::deadline::HANG;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{
    MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_content_and_tool_calls,
    canned_tool_calls,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

const NPC: &str = "/npc";
const VAULT_CELL: &str = "/main/npc/access/vault";
const VAULT_PASSPHRASE: &[u8] = b"npc-template-test-passphrase";
const BRAIN_MODEL: &str = "brain-stub";
const NAME: &str = "Ada Stone";
const OBS_TEXT: &str = "The baker Tom stands at your door with a basket of rolls.";
const HP_TEXT: &str = "Did Tom bring the rolls already?";
/// The rule every writing model of the memory carries in its prompt.
const LANGUAGE_RULE: &str = "Write every statement in the language";

/// The model cells of the tree and the grant handle each presents.
const MODEL_CELLS: [&str; 7] = [
    "brain",
    "curator/summarizer",
    "memory-hive/closer",
    "memory-hive/dialectic",
    "memory-hive/dreamer",
    "memory-hive/judge",
    "memory-hive/embed",
];

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "npc",
        "collector",
        "curator",
        "dispatcher",
        "memory-hive",
        "access",
        "cogny",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

fn npc_config() -> Value {
    read_json(&repo("templates/npc/config.json"))
}

/// The template laid out the way instantiation lays it out: a `ref` directory
/// is replaced by the referenced template, its `override_params` applied
/// (GH #140, #277), and what the ref directory carries beside its marker (the
/// grants seeded under `access/store/seed`) laid over it.
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(depth < 8, "ref chain does not end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
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
            overlay_seed(src, dst);
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
        } else if name == "config.json" || is_seed_row_file(src, &from) {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn is_seed_row_file(dir: &std::path::Path, file: &std::path::Path) -> bool {
    dir.file_name().is_some_and(|d| d == "seed") && file.extension().is_some_and(|e| e == "jsonl")
}

fn overlay_seed(src: &std::path::Path, dst: &std::path::Path) {
    for entry in std::fs::read_dir(src).expect("readable") {
        let from = entry.expect("entry").path();
        if from.is_dir() {
            let to = dst.join(from.file_name().expect("a name"));
            std::fs::create_dir_all(&to).expect("mkdir");
            overlay_seed(&from, &to);
        } else if is_seed_row_file(src, &from) {
            std::fs::copy(&from, dst.join(from.file_name().expect("a name"))).expect("copy");
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

/// `(path relative to the npc, config)` of every cell of the instantiated tree.
fn cells_of(npc: &std::path::Path) -> Vec<(String, Value)> {
    let mut files = Vec::new();
    configs_under(npc, &mut files);
    let mut out: Vec<(String, Value)> = files
        .into_iter()
        .map(|f| {
            let rel = f
                .parent()
                .expect("a cell directory")
                .strip_prefix(npc)
                .expect("under the npc")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, read_json(&f))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn instantiated() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let npc = td.path().join("main/npc");
    copy_resolved(&repo("templates/npc"), &npc, 0);
    (td, npc)
}

fn routes(lanes: &Value) -> Vec<String> {
    lanes
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The single-quoted routes a condition names in `hop.route == '<r>'`.
fn named_routes(condition: &str) -> Vec<String> {
    condition
        .split("hop.route == '")
        .skip(1)
        .filter_map(|s| s.split('\'').next().map(str::to_string))
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn table_of(base: &str, specs: &[EdgeSpec]) -> EdgeTable {
    let mut t = EdgeTable::new();
    for spec in specs {
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

/// One message through the adapter script itself, as its runner feeds it.
fn adapter(hop: Value, body: Value) -> Vec<Value> {
    adapter_with(json!({}), hop, body)
}

/// [`adapter`] with `overlay` over the shipped params (an instance's
/// `override_params`).
fn adapter_with(overlay: Value, hop: Value, body: Value) -> Vec<Value> {
    adapter_full(overlay, json!({}), hop, body)
}

/// [`adapter`] with `ctx` as the message's context (the round's
/// `npc_kind`, `turn_id`).
fn adapter_in(ctx: Value, hop: Value, body: Value) -> Vec<Value> {
    adapter_full(json!({}), ctx, hop, body)
}

fn adapter_full(overlay: Value, ctx: Value, hop: Value, body: Value) -> Vec<Value> {
    let world = read_json(&repo("templates/npc/world/config.json"));
    let script = world["params"]["script_inline"]
        .as_str()
        .expect("an inline script");
    let td = tempfile::TempDir::new().expect("tempdir");
    let file = td.path().join("world.py");
    std::fs::write(&file, script).expect("write the script");
    let mut params = world["params"].clone();
    params
        .as_object_mut()
        .expect("params")
        .remove("script_inline");
    for (k, v) in as_map(&overlay) {
        params[k] = v;
    }
    let input = json!({"envelope": {"header": {"hop": hop, "context": ctx}}, "body": body,
                       "params": params});
    let mut child = std::process::Command::new("python3")
        .arg("-I")
        .arg(&file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python3 runs");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.to_string().as_bytes())
            .expect("feed the adapter");
    }
    let out = child.wait_with_output().expect("the adapter ends");
    assert!(out.status.success(), "the adapter failed: {out:?}");
    meclaw_core::serde_json::from_slice::<Vec<Value>>(&out.stdout)
        .unwrap_or_else(|e| panic!("the adapter's output is a JSON array: {e}"))
}

// ═══════════════════════════════════════════════════════ 1. the file

#[test]
fn npc_hive_declares_its_lanes() {
    if !shipped() {
        return;
    }
    let cfg = npc_config();
    let params = &cfg["params"];
    assert_eq!(params["ports"], json!([]), "the npc is sealed");
    let accepts = routes(&params["contract"]["accepts"]);
    let emits = routes(&params["contract"]["emits"]);
    for lane in [
        "obs",
        "night",
        "digest",
        "hp",
        "hp_reply",
        "identity",
        "mandate",
        "interview",
        "in_model",
    ] {
        assert!(
            accepts.contains(&lane.to_string()),
            "accepts {lane}: {accepts:?}"
        );
    }
    for lane in [
        "intent",
        "night",
        "hp",
        "hp_reply",
        "interview",
        "model_refused",
        "error",
    ] {
        assert!(emits.contains(&lane.to_string()), "emits {lane}: {emits:?}");
    }
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(params.clone()).expect("the npc's params parse");
    let mut doors = 0;
    let mut exits = 0;
    for e in &hp.graph.edges {
        let named = named_routes(e.condition.as_deref().unwrap_or(""));
        if e.from == "." {
            doors += 1;
            for r in &named {
                assert!(
                    accepts.contains(r),
                    "the door {} -> {} opens the undeclared lane {r}",
                    e.from,
                    e.to
                );
            }
        }
        if e.to == "." {
            exits += 1;
            let leaves: Vec<String> = e
                .modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(|r| vec![r.trim_matches('\'').to_string()])
                .unwrap_or(named);
            for r in &leaves {
                assert!(
                    emits.contains(r),
                    "the exit {} -> . carries the undeclared lane {r}",
                    e.from
                );
            }
        }
    }
    assert!(doors >= 9 && exits >= 7, "doors {doors}, exits {exits}");

    // Every lane the npc draws into its memory hive is drained.
    let (td, _npc) = instantiated();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"}}"#,
    )
    .expect("root");
    let hives = [Path::new("/npc/memory-hive")];
    let reqs = collect_required_drains(td.path(), hives.iter());
    assert!(!reqs.is_empty(), "the memory hive declares its drains");
    let table = table_of(NPC, &hp.graph.edges);
    if let Err(e) = check_required_drains(&reqs, &table) {
        panic!("the npc leaves a memory lane undrained: {e:?}");
    }

    // `identity` replaces the leaf `persona.current`, `mandate` the leaf
    // `instructions.mandate` of the curator's pack.
    let out = adapter(
        json!({"route": "identity"}),
        json!({"identity": {"facts": ["married to Tom"], "since": "day 3"}, "messages": []}),
    );
    let pack = out
        .iter()
        .find(|d| d["header"]["route"] == "in_pack")
        .expect("identity leaves as a pack");
    assert!(
        pack["system"]["persona"]["current"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("married to Tom")),
        "identity -> persona.current: {pack}"
    );
    let out = adapter(
        json!({"route": "mandate"}),
        json!({"mandate": {"action": "start", "note": "sell the bread"}, "messages": []}),
    );
    let pack = out
        .iter()
        .find(|d| d["header"]["route"] == "in_pack")
        .expect("mandate leaves as a pack");
    assert!(
        pack["system"]["instructions"]["mandate"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("sell the bread")),
        "mandate -> instructions.mandate: {pack}"
    );
}

#[test]
fn npc_has_exactly_one_thinking_llm() {
    if !shipped() {
        return;
    }
    let (_td, npc) = instantiated();
    let thinking: Vec<String> = cells_of(&npc)
        .into_iter()
        .filter(|(rel, cfg)| {
            cfg["cell"]["type"] == "llm"
                && !rel.starts_with("curator/")
                && !rel.starts_with("memory-hive/")
        })
        .map(|(rel, _)| rel)
        .collect();
    assert_eq!(
        thinking,
        vec!["brain".to_string()],
        "one brain, nothing else thinks"
    );
}

#[test]
fn npc_every_model_cell_names_the_app() {
    if !shipped() {
        return;
    }
    let (_td, npc) = instantiated();
    let cells = cells_of(&npc);
    let llms: Vec<&(String, Value)> = cells
        .iter()
        .filter(|(_, c)| c["cell"]["type"] == "llm")
        .collect();
    assert!(
        llms.len() >= 6,
        "the brain, the summarizer, four memory cells: {}",
        llms.len()
    );
    let mut wrong = Vec::new();
    for (rel, cfg) in &llms {
        for key in ["x_title", "http_referer"] {
            if !cfg["params"][key].as_str().is_some_and(|v| !v.is_empty()) {
                wrong.push(format!("{rel}: {key} = {}", cfg["params"][key]));
            }
        }
    }
    let embed = cells
        .iter()
        .find(|(rel, _)| rel == "memory-hive/embed")
        .expect("the memory's embedder");
    let script = embed.1["params"]["script_inline"].as_str().unwrap_or("");
    for header in ["\"X-Title\"", "\"HTTP-Referer\""] {
        if !script.contains(header) {
            wrong.push(format!("memory-hive/embed sends no {header}"));
        }
    }
    // Every model cell presents its own grant, and the grant is seeded.
    let grants = std::fs::read_to_string(npc.join("access/store/seed/grants.jsonl"))
        .expect("the npc seeds its grants");
    for cell in MODEL_CELLS {
        let (_, cfg) = cells
            .iter()
            .find(|(rel, _)| rel == cell)
            .unwrap_or_else(|| panic!("no cell {cell}"));
        let grant = cfg["params"]["credential_grant_id"].as_str().unwrap_or("");
        if grant.is_empty() || !grants.contains(&format!("\"grant_id\": \"{grant}\"")) {
            wrong.push(format!("{cell}: grant {grant:?} not seeded"));
        }
    }
    assert!(
        wrong.is_empty(),
        "attribution and grants:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn npc_start_memories_reach_a_character_born_late() {
    if !shipped() {
        return;
    }
    // A character instantiated on day 5 (a birth in the world, a player's
    // colony that joins later) is seeded like one born on day 1.
    let out = adapter_with(
        json!({"identity": {"name": NAME,
                            "start_memories": [{"what": "learned to weave", "days_ago": 400}]}}),
        json!({"route": "obs"}),
        json!({"obs": {"turn_id": "t-1", "kind": "event", "text": OBS_TEXT, "day": 5, "tools": []},
               "messages": []}),
    );
    let memories = out
        .iter()
        .filter(|d| {
            d["header"]["route"] == "in_episode"
                && d["messages"][0]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("learned to weave"))
        })
        .count();
    assert_eq!(
        memories, 1,
        "the start memory reaches a character born on day 5, once: {out:?}"
    );
}

#[test]
fn npc_obs_without_a_day_is_refused_loudly() {
    if !shipped() {
        return;
    }
    let out = adapter(
        json!({"route": "obs"}),
        json!({"obs": {"turn_id": 9, "kind": "event", "text": OBS_TEXT, "tools": []},
               "messages": []}),
    );
    let on = |route: &str| {
        out.iter()
            .find(|d| d["header"]["route"] == route)
            .unwrap_or_else(|| panic!("`{route}` on the rim: {out:?}"))
            .clone()
    };
    let error = on("error");
    assert_eq!(error["error"]["error_code"], "obs_without_day", "{error}");
    assert_eq!(error["error"]["turn_id"], 9, "the world's own id: {error}");
    let intent = on("intent");
    assert_eq!(
        intent["intent"],
        json!({"turn_id": 9, "never": true}),
        "the move is answered `never`: {intent}"
    );
    assert!(
        out.iter().all(|d| d["header"]["route"] != "in_turn"),
        "no move runs without its day: {out:?}"
    );
}

#[test]
fn npc_brain_is_a_fast_speaking_model() {
    if !shipped() {
        return;
    }
    let brain = read_json(&repo("templates/npc/brain/config.json"));
    let row = brain["contract"]["settings"]["external_timeout_ms_row"]["default"].clone();
    let talky = repo("templates/talky/brain/config.json");
    let voice = if talky.is_file() {
        read_json(&talky)["contract"]["settings"]["external_timeout_ms_row"]["default"].clone()
    } else {
        json!("openai/gpt-6-luna")
    };
    assert_eq!(
        row, voice,
        "R-NPC-1: the brain is born on the row of a speaking voice"
    );
    assert!(
        brain["params"]["max_tokens"]
            .as_u64()
            .is_some_and(|n| n >= 16_384),
        "the brain's own budget carries the longest move: {}",
        brain["params"]["max_tokens"]
    );
    // Its own budget (the GH #1037 floor, not the row's whole output) bounds
    // a hung provider within minutes, not the 51 of a reasoning core.
    let backstop = brain["cell"]["message_timeout"]
        .as_u64()
        .unwrap_or(u64::MAX);
    assert!(backstop <= 190_000, "message_timeout {backstop}");
}

#[test]
fn npc_names_no_private_template() {
    if !shipped() {
        return;
    }
    for rel in ["templates/npc/template.json", "templates/npc/README.md"] {
        let text = std::fs::read_to_string(repo(rel)).expect("readable");
        assert!(
            !text.to_lowercase().contains("egon"),
            "{rel} names a template the export does not carry"
        );
    }
}

#[test]
fn npc_the_door_admits_a_reasoning_effort() {
    // Under 0.66.0 `override_params brain.reasoning_effort` was refused: "names
    // no param of llm at 'brain' in template 'npc'" (the door admits a key only
    // when the cell's params carry it, GH #294). It ships null: no thinking
    // (R-NPC-1), the request byte for byte the one without the declaration.
    if !shipped() {
        return;
    }
    let brain = read_json(&repo("templates/npc/brain/config.json"));
    assert_eq!(
        brain["params"].get("reasoning_effort"),
        Some(&Value::Null),
        "declared, unset"
    );
    assert_eq!(
        brain["contract"]["settings"]["reasoning_effort"]["default"],
        Value::Null,
        "a setting that ships unset"
    );
    // The tree as instantiation lays it out (every ref resolved against the
    // templates the npc names, all of them public), so the parse needs no
    // registry of the whole `templates/` directory, which the export does not
    // carry as a root.
    let (_td, npc) = instantiated();
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&npc, &registry).expect("the npc parses");
    let cell = parsed
        .cells
        .iter()
        .find(|c| c.rel_path == "brain")
        .expect("a brain");
    let version = read_json(&repo("templates/npc/template.json"))["version"]
        .as_str()
        .expect("a version")
        .to_string();
    check_override_params(
        cell,
        Some("brain"),
        &format!("npc@{version}"),
        &json!({"reasoning_effort": "low"}),
    )
    .unwrap_or_else(|why| panic!("the door admits brain.reasoning_effort: {why:?}"));
}

/// An assistant turn of the round as the dispatcher hands it to `./world`:
/// `text` (empty = none) and the calls `(id, name, arguments)` beside it.
fn calls_body(text: &str, calls: &[(&str, &str, Value)]) -> Value {
    let mut turns = Vec::new();
    if !text.is_empty() {
        turns.push(json!({"origin": "assistant", "type": "text", "text": text}));
    }
    for (id, name, args) in calls {
        turns.push(json!({"origin": "assistant", "type": "tool_call", "id": id,
                          "text": json!({"name": name, "arguments": args.to_string()}).to_string()}));
    }
    json!({ "messages": turns })
}

fn on_route<'a>(out: &'a [Value], route: &str) -> Vec<&'a Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == route)
        .collect()
}

#[test]
fn npc_text_beside_a_world_tool_is_its_says() {
    if !shipped() {
        return;
    }
    let out = adapter(
        json!({"route": "calls"}),
        calls_body(
            "Here you go, Tom.",
            &[("call-1", "say", json!({"text": "Rolls!"}))],
        ),
    );
    let intents = on_route(&out, "intent");
    assert_eq!(intents.len(), 1, "one intent: {out:?}");
    let first = &intents[0]["intent"]["intents"][0];
    assert_eq!(first["tool"], "say", "{out:?}");
    assert_eq!(
        first["says"], "Here you go, Tom.",
        "the text is the call's says: {out:?}"
    );
}

#[test]
fn npc_an_empty_answer_is_never() {
    if !shipped() {
        return;
    }
    let out = adapter(json!({"route": "answer"}), json!({"messages": []}));
    let intents = on_route(&out, "intent");
    assert_eq!(intents.len(), 1, "one intent: {out:?}");
    assert_eq!(intents[0]["intent"]["never"], true, "{out:?}");
    assert!(
        intents[0]["intent"].get("intents").is_none(),
        "a move without text and call says nothing: {out:?}"
    );
}

#[test]
fn npc_hp_text_beside_any_call_is_the_reply() {
    // Every call the handset round makes is a handoff and ends the round
    // without an `answer`: the text beside it is final, so it is the reply --
    // with a tool the handset does not offer too (the head of the hp prompt
    // names the last move's world tool). Without text such a call is an
    // error, never a silent round.
    if !shipped() {
        return;
    }
    let ctx = json!({"npc_kind": "hp", "turn_id": "hp-1"});
    let out = adapter_in(
        ctx.clone(),
        json!({"route": "calls"}),
        calls_body("Yes, he did.", &[("call-1", "say", json!({"text": "Hi"}))]),
    );
    let replies = on_route(&out, "hp_reply");
    assert_eq!(replies.len(), 1, "one reply: {out:?}");
    assert_eq!(replies[0]["hp_reply"]["text"], "Yes, he did.", "{out:?}");
    assert!(on_route(&out, "error").is_empty(), "{out:?}");
    let out = adapter_in(
        ctx,
        json!({"route": "calls"}),
        calls_body("", &[("call-1", "say", json!({"text": "Hi"}))]),
    );
    let errors = on_route(&out, "error");
    assert_eq!(errors.len(), 1, "a round never ends silent: {out:?}");
    assert_eq!(
        errors[0]["error"]["error_code"], "tool_not_offered",
        "{out:?}"
    );
    assert_eq!(errors[0]["error"]["kind"], "hp", "{out:?}");
    assert!(on_route(&out, "hp_reply").is_empty(), "{out:?}");
}

#[test]
fn npc_a_move_with_only_a_phone_call_has_its_intent() {
    // The handoff of a `phone_call` ends the move without an `answer`: the
    // world still gets its intent -- the text beside the call as speech, else
    // `never` -- and the handset message leaves on `hp`.
    if !shipped() {
        return;
    }
    let ctx = json!({"npc_kind": "contact", "turn_id": "t-1"});
    let call = [(
        "call-p",
        "phone_call",
        json!({"to": "Tom", "text": "Rolls?"}),
    )];
    let out = adapter_in(
        ctx.clone(),
        json!({"route": "calls"}),
        calls_body("I will call Tom.", &call),
    );
    assert_eq!(on_route(&out, "hp").len(), 1, "the call leaves: {out:?}");
    let intents = on_route(&out, "intent");
    assert_eq!(intents.len(), 1, "one intent: {out:?}");
    assert_eq!(intents[0]["intent"]["turn_id"], "t-1", "{out:?}");
    assert_eq!(
        intents[0]["intent"]["intents"],
        json!([{"says": "I will call Tom."}]),
        "{out:?}"
    );
    let out = adapter_in(ctx, json!({"route": "calls"}), calls_body("", &call));
    assert_eq!(on_route(&out, "hp").len(), 1, "the call leaves: {out:?}");
    let intents = on_route(&out, "intent");
    assert_eq!(intents.len(), 1, "one intent: {out:?}");
    assert_eq!(intents[0]["intent"]["never"], true, "{out:?}");
}

#[test]
fn npc_a_failed_interview_is_answered() {
    // The interview's edge sets no `round_error`: a failed call was dropped
    // and the question never answered. It is the error and an answer `null`.
    if !shipped() {
        return;
    }
    let out = adapter_in(
        json!({"npc_kind": "interview", "turn_id": "q-1"}),
        json!({"route": "answer", "finish_reason": "error", "error_code": "upstream_error"}),
        json!({"messages": []}),
    );
    let answers = on_route(&out, "interview");
    assert_eq!(answers.len(), 1, "one answer: {out:?}");
    assert_eq!(answers[0]["interview"]["question_id"], "q-1", "{out:?}");
    assert_eq!(answers[0]["interview"]["answer"], Value::Null, "{out:?}");
    assert_eq!(
        answers[0]["interview"]["error"], "upstream_error",
        "{out:?}"
    );
    assert_eq!(on_route(&out, "error").len(), 1, "{out:?}");
}

#[test]
fn npc_text_beside_a_recall_is_no_move() {
    // The round's own tools (`memory_recall`, `history_*`) run on: the text
    // beside them is an interim, never the move -- the move comes after.
    if !shipped() {
        return;
    }
    for tool in ["memory_recall", "history_search"] {
        let out = adapter(
            json!({"route": "calls"}),
            calls_body(
                "One moment, let me think.",
                &[("call-r", tool, json!({"query": "Tom"}))],
            ),
        );
        assert!(
            on_route(&out, "intent").is_empty() && on_route(&out, "hp_reply").is_empty(),
            "{tool}: an interim leaves nothing on the rim: {out:?}"
        );
        assert_eq!(
            on_route(&out, "in_calls").len(),
            1,
            "{tool}: the round runs on: {out:?}"
        );
    }
}

// ═══════════════════════════════════════════════ 2. the character (colony)

struct Stubs {
    brain: MockOpenAI,
    background: MockOpenAI,
}

/// Every `llm` cell at a stub (the brain at the scripted one), every model of
/// the tree named, every timer out of the run's way, the env file and the
/// provider key deposited in the npc's own vault -- the grants stay the
/// template's.
fn build(td: &tempfile::TempDir, stubs: &Stubs) {
    let root = td.path();
    let main = root.join("main");
    let npc = main.join("npc");
    copy_resolved(&repo("templates/npc"), &npc, 0);
    override_params_on_disk(
        &npc.join("world"),
        &json!({"npc_id": "n1", "world_start": "2026-01-01T00:00:00Z",
                "identity": {"name": NAME, "core": "a weaver who keeps to herself",
                             "origin": "the hill farm", "life_goal": "a workshop of her own",
                             "facts": ["unmarried"], "voice": "short, dry sentences",
                             "start_memories": [{"what": "learned to weave", "with": "her aunt",
                                                 "days_ago": 400}]}}),
    );
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    for f in &files {
        let mut cfg = read_json(f);
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(&npc)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        match cfg["cell"]["type"].as_str() {
            Some("llm") => {
                let (url, model) = if rel == "brain" {
                    (&stubs.brain.base_url, BRAIN_MODEL.to_string())
                } else {
                    (
                        &stubs.background.base_url,
                        format!("stub-{}", rel.replace('/', "-")),
                    )
                };
                cfg["params"]["base_url"] = json!(format!("{url}/v1"));
                cfg["params"]["model"] = json!(model);
            }
            Some("timer") => {
                if let Some(schedules) = cfg
                    .pointer_mut("/params/schedules")
                    .and_then(Value::as_array_mut)
                {
                    for s in schedules.iter_mut() {
                        if s.get("cron").is_some() {
                            s["cron"] = json!(NEVER_CRON);
                        }
                    }
                }
            }
            _ => {}
        }
        write_json(f, &cfg);
    }
    // No armed night inside the run: the night comes on the lane.
    override_params_on_disk(
        &npc.join("memory-hive/writer"),
        &json!({"night_at_utc": ""}),
    );
    let edges = json!([
        {"from": "./npc", "to": "/sink", "condition": "has(hop.npc_rim)"},
        {"from": "./npc", "to": "/park",
         "condition": "!has(hop.npc_rim) && has(hop.route) && (hop.route == 'model_refused' || hop.route == 'error')"}
    ]);
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let key_file = root.join("vault.key");
    std::fs::write(&key_file, VAULT_PASSPHRASE).expect("key file");
    // The operator's half of a grown vault (as `examples/` seed it): it opens
    // itself from a key file named in the environment.
    override_params_on_disk(
        &npc.join("access/vault"),
        &json!({"key_source": "plainfile", "key_file": key_file.display().to_string()}),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).expect("0600");
    }
    meclaw_cells::vault::user_channel::add(
        root,
        VAULT_CELL,
        "cred:openrouter",
        b"test-key",
        VAULT_PASSPHRASE,
    )
    .expect("the cold deposit into the npc's vault");
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in &files {
        let raw = std::fs::read_to_string(f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            // A token with a default keeps it; only a bare one needs a value.
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
    vars.insert(
        "MECLAW_VAULT_KEY_FILE".into(),
        key_file.display().to_string(),
    );
    vars.insert(
        "MEMORY_LLM_BASE_URL".into(),
        format!("{}/v1", stubs.background.base_url),
    );
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", stubs.background.base_url),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("the env file");
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    #[allow(dead_code)]
    park: mpsc::Receiver<Message>,
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        (
            "vault".to_string(),
            Arc::new(meclaw_cells::vault::VaultCellFactory),
        ),
    ]
}

async fn boot(
    brain: Vec<meclaw_testing::mock_http::MockResponse>,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    let stubs = Stubs {
        brain: MockOpenAI::start(brain).await,
        // One answer for every background model: an empty verdict is a close
        // pass that changes nothing, and a summary all the same.
        background: MockOpenAI::start(vec![canned_chat_completion("{}", "stop")]).await,
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs);
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
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
        .expect("the instantiated npc boots");
    (
        td,
        stubs,
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

fn lane(route: &str, slot: Value) -> Message {
    let mut body = json!({"messages": []});
    body[route] = slot;
    MessageBuilder::new(Path::new(NPC))
        .hop(as_map(&json!({"route": route})))
        .context(Map::new())
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

fn obs(turn_id: impl Into<Value>, day: u64) -> Message {
    let turn_id: Value = turn_id.into();
    lane(
        "obs",
        json!({"turn_id": turn_id, "kind": "contact", "text": OBS_TEXT, "day": day,
               "tools": [{"name": "say", "description": "Say something to who is in front of you.",
                          "parameters": {"type": "object",
                                         "properties": {"text": {"type": "string"}},
                                         "required": ["text"]}}]}),
    )
}

fn body_of(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("an inline body on the rim, got {other:?}"),
    }
}

fn route_of(m: &Message) -> String {
    m.headers
        .hop
        .get("route")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// The next rim message on `route`; others on the way are kept. A run that
/// never brings it fails at the hang mark with the road the messages took.
async fn next_on(
    h: &ColonyHandle,
    ports: &mut Ports,
    route: &str,
    seen: &mut Vec<Message>,
) -> Message {
    let wait = async {
        loop {
            let m = ports.sink.recv().await.expect("the sink is open");
            if route_of(&m) == route {
                return m;
            }
            seen.push(m);
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(m) => m,
        Err(_) => panic!(
            "HANG: the npc never emitted `{route}`; the road:\n{}",
            road(h).await
        ),
    }
}

/// The first rim message on each of `routes`, whatever their order; others
/// on the way are kept. A run that never brings one fails at the hang mark.
async fn first_of(
    h: &ColonyHandle,
    ports: &mut Ports,
    routes: &[&str],
    seen: &mut Vec<Message>,
) -> BTreeMap<String, Message> {
    let mut got = BTreeMap::new();
    let wait = async {
        while got.len() < routes.len() {
            let m = ports.sink.recv().await.expect("the sink is open");
            let r = route_of(&m);
            if routes.contains(&r.as_str()) && !got.contains_key(&r) {
                got.insert(r, m);
            } else {
                seen.push(m);
            }
        }
    };
    let outcome = tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await;
    match outcome {
        Ok(()) => got,
        Err(_) => panic!(
            "HANG: the npc never emitted all of {routes:?}; the road:\n{}",
            road(h).await
        ),
    }
}

/// `from -> to route` of the last logged messages (the store's own traffic
/// left out), oldest first.
async fn road(h: &ColonyHandle) -> String {
    let mut rows = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.retain(|r| {
        !r.to_path.ends_with("store")
            && !r.from_path.ends_with("store")
            && !r.to_path.ends_with("ledger")
            && !r.from_path.ends_with("ledger")
    });
    let skip = rows.len().saturating_sub(120);
    rows.iter()
        .skip(skip)
        .map(|r| {
            let hdr: Value = meclaw_core::serde_json::from_str(&r.headers_json).unwrap_or_default();
            let hop = &hdr["hop"];
            format!(
                "{} -> {} {} {}",
                r.from_path,
                r.to_path,
                hop["route"].as_str().unwrap_or("-"),
                [
                    "error_code",
                    "reject_reason",
                    "store_error",
                    "finish_reason",
                    "phase"
                ]
                .iter()
                .filter_map(|k| hop[*k].as_str().map(|v| format!("{k}={v}")))
                .collect::<Vec<_>>()
                .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Waits until the message log holds a message from `from` to `to`:
/// event-driven over the writer's commit counter, the hang mark as the
/// deadline. The dream runs two seconds after its trigger (memory-hive
/// README, the night now), so a quiet colony says nothing about it; the
/// messages of its run do.
async fn wait_sent(h: &ColonyHandle, from: &str, to: &str) {
    let mut seq = h.subscribe_log_seq();
    // A closed log channel is no sighting: it ends the wait as a failure with
    // the road, never as if the message had gone (it let the night's pin pass
    // without the dream's messages).
    let wait = async {
        loop {
            seq.borrow_and_update();
            let sent = h
                .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
                .await
                .iter()
                .any(|r| r.from_path == from && r.to_path == to);
            if sent {
                return true;
            }
            if seq.changed().await.is_err() {
                return false;
            }
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(true) => {}
        Ok(false) => panic!(
            "the message log closed before anything went from {from} to {to}; the road:\n{}",
            road(h).await
        ),
        Err(_) => panic!(
            "HANG: nothing went from {from} to {to}; the road:\n{}",
            road(h).await
        ),
    }
}

/// Whether a message on `route` went from `from` to `to`.
async fn logged(h: &ColonyHandle, from: &str, to: &str, route: &str) -> bool {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .any(|r| {
            r.from_path == from
                && r.to_path == to
                && meclaw_core::serde_json::from_str::<Value>(&r.headers_json)
                    .is_ok_and(|h| h["hop"]["route"] == route)
        })
}

fn wire_text(req: &OpenAiRequestSnapshot) -> String {
    req.messages()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn system_text(req: &OpenAiRequestSnapshot) -> String {
    req.messages()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn say_call() -> meclaw_testing::mock_http::MockResponse {
    say_call_as("call-say-1")
}

fn say_call_as(id: &str) -> meclaw_testing::mock_http::MockResponse {
    canned_tool_calls(vec![(id, "say", r#"{"text":"Thank you, Tom."}"#)])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_obs_ends_in_one_intent() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![say_call(), say_call_as("call-say-2")]).await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], "t-1", "the turn id comes back: {slot}");
    assert_eq!(slot["intents"][0]["tool"], "say", "{slot}");
    assert_eq!(
        slot["intents"][0]["args"]["text"], "Thank you, Tom.",
        "{slot}"
    );
    assert_eq!(slot["intents"].as_array().map(Vec::len), Some(1), "{slot}");
    let first = stubs.brain.recorded_requests().await;
    let tools: Vec<String> = first[0]
        .tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        tools.contains(&"say".to_string()),
        "the move's menu is the round's: {tools:?}"
    );
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        1,
        "a world call ends the move: exactly one brain call"
    );
    // The move closed its round: the next move is one call again, its window
    // is well-formed (the stub refuses an open tool call, as the provider
    // does), and it carries what the character did on the first move.
    // A world that counts its moves sends an int and gets the int back.
    h.send(obs(2, 1)).await;
    let second = next_on(&h, &mut ports, "intent", &mut seen).await;
    assert_eq!(
        body_of(&second)["intent"]["turn_id"],
        json!(2),
        "the turn id comes back unchanged, an int as an int: {}",
        body_of(&second)
    );
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one brain call per move");
    let window = wire_text(&reqs[1]);
    assert!(
        window.contains("Thank you, Tom."),
        "the next move sees what the character did: {window}"
    );
    while let Ok(m) = ports.sink.try_recv() {
        seen.push(m);
    }
    assert!(
        seen.iter().all(|m| route_of(m) != "intent"),
        "one intent per move: {:?}",
        seen.iter().map(body_of).collect::<Vec<_>>()
    );
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_identity_is_seeded_not_human() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![say_call()]).await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "intent", &mut seen).await;
    let reqs = stubs.brain.recorded_requests().await;
    let system = system_text(&reqs[0]);
    assert!(
        system.contains(NAME),
        "the identity reaches the brain: {system}"
    );
    assert!(
        system.contains("You are an AI") && system.contains("you say truthfully that you are"),
        "the frame discloses the AI: {system}"
    );
    assert!(
        !system.to_lowercase().contains("human being"),
        "never a human being: {system}"
    );
    let frame_at = system.find("You are an AI").unwrap_or(usize::MAX);
    let name_at = system.find(NAME).unwrap_or(0);
    assert!(
        frame_at < name_at,
        "the frame stands before the person it frames: {system}"
    );
    assert!(
        seen.iter().all(|m| route_of(m) != "error"),
        "the pack is acknowledged without an error: {:?}",
        seen.iter().map(body_of).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_night_closes_with_a_model_call() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![say_call()]).await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.quiesce().await;
    let before = stubs.background.recorded_requests().await.len();
    h.send(lane("night", json!({"day": 1}))).await;
    let night = next_on(&h, &mut ports, "night", &mut seen).await;
    let slot = body_of(&night)["night"].clone();
    assert_eq!(slot["day"], 1, "{slot}");
    assert_eq!(
        slot["ok"],
        true,
        "the close pass reported: {slot}\n{}",
        road(&h).await
    );
    let closer: Vec<String> = stubs.background.recorded_requests().await[before..]
        .iter()
        .filter_map(|r| r.model().map(str::to_string))
        .collect();
    assert!(
        closer.iter().any(|m| m == "stub-memory-hive-closer"),
        "the night calls the memory's closer: {closer:?}"
    );
    // The night's model call names its app (GH #1251) and writes in the
    // language of the day, never in a default English (the first run with a
    // real model filed German episodes as English facts).
    let night_reqs = stubs.background.recorded_requests().await;
    let call = night_reqs[before..]
        .iter()
        .find(|r| r.model() == Some("stub-memory-hive-closer"))
        .expect("the closer's request");
    for header in ["x-title", "http-referer"] {
        assert!(
            call.headers.get(header).is_some_and(|v| !v.is_empty()),
            "the closer's call carries {header}: {:?}",
            call.headers
        );
    }
    assert!(
        system_text(call).contains(LANGUAGE_RULE),
        "the closer's prompt carries the language rule: {}",
        system_text(call)
    );
    let dream = read_json(&repo("templates/memory-hive/dream-glue/config.json"));
    assert!(
        dream["params"]["script_inline"]
            .as_str()
            .is_some_and(|s| s.contains(LANGUAGE_RULE)),
        "the dreamer's prompt carries the language rule as well"
    );
    h.quiesce().await;
    assert!(
        logged(&h, "/npc/world", "/npc/memory-hive", "clock_op").await,
        "the night triggers the memory's dream:\n{}",
        road(&h).await
    );
    // The dream ran: its booked run reached the entity night and the recur
    // pass -- the two messages the `dream-glue` no_route of GH #1266 dropped --
    // and no message on the way became a dead letter. (Its dreamer is bought
    // only by a fact that moved, GH #857: stub verdicts that add nothing buy
    // none.)
    wait_sent(
        &h,
        "/npc/memory-hive/dream-glue",
        "/npc/memory-hive/entity-glue",
    )
    .await;
    wait_sent(&h, "/npc/memory-hive/dream-glue", "/npc/memory-hive/recur").await;
    h.quiesce().await;
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_hp_round_shares_the_window() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![
        say_call(),
        canned_chat_completion("Yes, he was here this morning.", "stop"),
    ])
    .await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.quiesce().await;
    h.send(lane(
        "hp",
        json!({"text": HP_TEXT, "at": "2026-01-01T09:00:00Z", "from": "Mara"}),
    ))
    .await;
    let reply = next_on(&h, &mut ports, "hp_reply", &mut seen).await;
    let slot = body_of(&reply)["hp_reply"].clone();
    assert!(
        slot["text"]
            .as_str()
            .is_some_and(|t| t.contains("he was here")),
        "the handset's answer: {slot}"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one call per round");
    let menu: Vec<String> = reqs[1]
        .tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        menu.contains(&"phone_call".to_string()) && !menu.contains(&"say".to_string()),
        "the hp round offers the phone, never the last move's world tools: {menu:?}"
    );
    let window = wire_text(&reqs[1]);
    assert!(window.contains(HP_TEXT), "the hp round asks: {window}");
    assert!(
        window.contains(OBS_TEXT),
        "the hp round sees the world's move in its window: {window}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_a_repeated_turn_is_refused_loudly() {
    // A world that sends one turn twice (seen: a boot sent turn 1 three times)
    // parked the collector's window leg twice and the character hung. The
    // second copy is refused on the error lane; the first runs on.
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![say_call(), say_call_as("call-say-2")]).await;
    h.send(obs("t-1", 1)).await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    let error = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&error)["error"].clone();
    assert_eq!(slot["error_code"], "duplicate_turn", "{slot}");
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    assert_eq!(body_of(&intent)["intent"]["intents"][0]["tool"], "say");
    h.quiesce().await;
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        1,
        "one brain call for the one turn"
    );
    while let Ok(m) = ports.sink.try_recv() {
        seen.push(m);
    }
    let routes: Vec<String> = seen.iter().map(route_of).collect();
    assert!(
        routes.iter().all(|r| r != "intent" && r != "error"),
        "one intent, one error: {routes:?}"
    );
    // No hang: the next turn runs.
    h.send(obs("t-2", 1)).await;
    let next = next_on(&h, &mut ports, "intent", &mut seen).await;
    assert_eq!(body_of(&next)["intent"]["turn_id"], "t-2");
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_speech_without_a_tool_reaches_the_world() {
    // A move answered with text alone was `intents: []` and the speech was
    // lost. It is one intent without a tool: pure speech.
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(vec![canned_chat_completion(
        "Lunch break! I treat myself to a coffee.",
        "stop",
    )])
    .await;
    h.send(obs("t-1", 1)).await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    assert!(slot.get("never").is_none(), "{slot}");
    let intents = slot["intents"].as_array().cloned().unwrap_or_default();
    assert_eq!(intents.len(), 1, "one intent: {slot}");
    assert!(
        intents[0].get("tool").is_none(),
        "speech has no tool: {slot}"
    );
    assert_eq!(
        intents[0]["says"], "Lunch break! I treat myself to a coffee.",
        "{slot}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_hp_text_beside_a_call_is_the_reply() {
    // On the handset the text beside a `phone_call` is the reply; the call
    // leaves as the character's own message.
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(vec![canned_content_and_tool_calls(
        "I will ask him.",
        vec![(
            "call-phone-1",
            "phone_call",
            r#"{"to":"Tom","text":"Did you bring rolls?"}"#,
        )],
    )])
    .await;
    h.send(lane(
        "hp",
        json!({"text": HP_TEXT, "at": "2026-01-01T09:00:00Z", "from": "Mara"}),
    ))
    .await;
    let mut seen = Vec::new();
    let reply = next_on(&h, &mut ports, "hp_reply", &mut seen).await;
    assert_eq!(
        body_of(&reply)["hp_reply"]["text"],
        "I will ask him.",
        "{}",
        body_of(&reply)
    );
    h.quiesce().await;
    while let Ok(m) = ports.sink.try_recv() {
        seen.push(m);
    }
    assert!(
        seen.iter().any(|m| route_of(m) == "hp"),
        "the call leaves as the character's own message: {:?}",
        seen.iter().map(route_of).collect::<Vec<_>>()
    );
    assert!(
        seen.iter().all(|m| route_of(m) != "hp_reply"),
        "one reply: {:?}",
        seen.iter().map(body_of).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_hp_text_beside_a_recall_is_not_the_reply() {
    // A handset round that says "one moment" beside a recall sent that
    // interim as THE reply and dropped the answer after the recall: the reply
    // is the text beside a `phone_call` or the round's final text, never an
    // interim beside the round's own tools.
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![
        canned_content_and_tool_calls(
            "One moment, let me think.",
            vec![("call-recall-1", "memory_recall", r#"{"query":"Tom rolls"}"#)],
        ),
        canned_chat_completion("Yes, Tom brought them at nine.", "stop"),
    ])
    .await;
    h.send(lane(
        "hp",
        json!({"text": HP_TEXT, "at": "2026-01-01T09:00:00Z", "from": "Mara"}),
    ))
    .await;
    let mut seen = Vec::new();
    let reply = next_on(&h, &mut ports, "hp_reply", &mut seen).await;
    assert_eq!(
        body_of(&reply)["hp_reply"]["text"],
        "Yes, Tom brought them at nine.",
        "the answer after the recall is the reply: {}",
        body_of(&reply)
    );
    h.quiesce().await;
    while let Ok(m) = ports.sink.try_recv() {
        seen.push(m);
    }
    assert!(
        seen.iter().all(|m| route_of(m) != "hp_reply"),
        "one reply: {:?}",
        seen.iter().map(body_of).collect::<Vec<_>>()
    );
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        2,
        "the recall's answer is the round's second call"
    );
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npc_a_repeated_hp_is_refused_loudly() {
    // The handset's turn id is `hp-<at>-<crc(text)>`: a second delivery of one
    // message ran a second round under the same id (the hang the obs lane
    // measured) and its reply was swallowed. It is refused on the error lane.
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![
        canned_chat_completion("Yes, he was here this morning.", "stop"),
        canned_chat_completion("A second round.", "stop"),
    ])
    .await;
    let message = json!({"text": HP_TEXT, "at": "2026-01-01T09:00:00Z", "from": "Mara"});
    h.send(lane("hp", message.clone())).await;
    h.send(lane("hp", message)).await;
    let mut seen = Vec::new();
    // The error and the reply come in either order.
    let mut got = first_of(&h, &mut ports, &["error", "hp_reply"], &mut seen).await;
    let slot = body_of(&got.remove("error").expect("an error"))["error"].clone();
    assert_eq!(slot["error_code"], "duplicate_turn", "{slot}");
    assert_eq!(slot["kind"], "hp", "{slot}");
    let reply = got.remove("hp_reply").expect("a reply");
    assert_eq!(
        body_of(&reply)["hp_reply"]["text"],
        "Yes, he was here this morning."
    );
    h.quiesce().await;
    assert_eq!(
        stubs.brain.recorded_requests().await.len(),
        1,
        "one round for the one message"
    );
    while let Ok(m) = ports.sink.try_recv() {
        seen.push(m);
    }
    assert!(
        seen.iter()
            .all(|m| route_of(m) != "hp_reply" && route_of(m) != "error"),
        "one reply, one error: {:?}",
        seen.iter().map(body_of).collect::<Vec<_>>()
    );
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}
