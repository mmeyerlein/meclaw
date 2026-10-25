//! GH #1292 -- `templates/figure`, the turns: a colony of the instantiated
//! figure (world adapter, phone adapter, `ref simple-agent`) with stub models
//! (no real model call).
//!
//! A world turn calls world tools as client tools and ends as one `intent`
//! when the model stops; a phone message leaves as a human-channel `turn`; an
//! answer on that channel waits for the open world turn; the game-agent switch
//! changes only the prefix and the speaker; a night closes every session of
//! the day and acknowledges after the last report; a failed interview names
//! its error code; both roles frame the agent truthful about being an AI; an
//! instance's override reaches the cells inside the referenced agent (C-T6).
//!
//! Real time with hang marks only (AGENTS rule 11): code cells are child
//! processes, a virtual clock does not reach them.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::mutation::subtree::{SubtreeOverrides, stage_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::deadline::HANG;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{
    MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_error_status,
    canned_tool_calls,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::mpsc;

const FIGURE: &str = "/figure";
const VAULT_CELL: &str = "/main/figure/simple-agent/access/vault";
const VAULT_PASSPHRASE: &[u8] = b"figure-test-passphrase";
const BRAIN_MODEL: &str = "brain-stub";
const FIGURE_ID: &str = "f1";
const WORLD_ID: &str = "w1";

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "figure",
        "simple-agent",
        "collector",
        "curator",
        "dispatcher",
        "memory-hive",
        "access",
        "tools",
        "affinity",
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
            // A pinned copy (`simple-agent@1.1.4`) is its own directory.
            let name = reference.split('@').next().unwrap_or_default();
            let pinned = repo("templates").join(reference);
            let target = if pinned.join("config.json").is_file() {
                pinned
            } else {
                repo("templates").join(name)
            };
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

/// Every bare `${NAME}` (no default) in the config files under `dir`.
fn bare_env_tokens(files: &[std::path::PathBuf]) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for f in files {
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
    vars
}

struct Stubs {
    brain: MockOpenAI,
    background: MockOpenAI,
}

/// The world's instance params: a human-sim with a phone unless `world`
/// says otherwise (laid over key by key).
fn world_params(world: &Value) -> Value {
    let mut p = json!({"figure_id": FIGURE_ID, "world_id": WORLD_ID, "role": "human-sim",
                       "phone": {"enabled": true, "speaker": "member", "contact": "Robin"}});
    for (k, v) in as_map(world) {
        p[k] = v;
    }
    p
}

/// The figure under `/figure`, every `llm` cell at a stub (the agent's brain at
/// the scripted one), every timer out of the run's way, the weather at the
/// background stub, the provider key in the agent's own vault.
fn build(td: &tempfile::TempDir, stubs: &Stubs, world: &Value) {
    let root = td.path();
    let main = root.join("main");
    let figure = main.join("figure");
    let agent = figure.join("simple-agent");
    copy_resolved(&repo("templates/figure"), &figure, 0);
    override_params_on_disk(&figure.join("world"), &world_params(world));
    override_params_on_disk(
        &agent.join("tools/weather"),
        &json!({"geocode_endpoint": format!("{}/v1/geocode", stubs.background.base_url),
                "forecast_endpoint": format!("{}/v1/forecast", stubs.background.base_url)}),
    );
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    for f in &files {
        let mut cfg = read_json(f);
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(&agent)
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
        &agent.join("memory-hive/writer"),
        &json!({"night_at_utc": ""}),
    );
    let edges = json!([
        {"from": "./figure", "to": "/sink", "condition": "has(hop.route)"}
    ]);
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let key_file = root.join("vault.key");
    std::fs::write(&key_file, VAULT_PASSPHRASE).expect("key file");
    override_params_on_disk(
        &agent.join("access/vault"),
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
    .expect("the cold deposit into the agent's vault");
    let mut vars = bare_env_tokens(&files);
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
        ("bash".to_string(), Arc::new(meclaw_cells::BashCellFactory)),
        ("file".to_string(), Arc::new(meclaw_cells::FileCellFactory)),
        ("edit".to_string(), Arc::new(meclaw_cells::EditCellFactory)),
        (
            "web_fetch".to_string(),
            Arc::new(meclaw_cells::WebFetchCellFactory),
        ),
        (
            "web_search".to_string(),
            Arc::new(meclaw_cells::WebSearchCellFactory),
        ),
    ]
}

async fn boot(
    brain: Vec<meclaw_testing::mock_http::MockResponse>,
    world: Value,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    boot_capped(brain, world, None).await
}

/// `boot`, the agent's loop capped at `max_iter` rounds when given.
async fn boot_capped(
    brain: Vec<meclaw_testing::mock_http::MockResponse>,
    world: Value,
    max_iter: Option<u64>,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    let stubs = Stubs {
        brain: MockOpenAI::start(brain).await,
        // One answer for every background model: an empty verdict is a close
        // pass that changes nothing, and a summary all the same.
        background: MockOpenAI::start(vec![canned_chat_completion("{}", "stop")]).await,
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs, &world);
    if let Some(n) = max_iter {
        override_params_on_disk(
            &td.path()
                .join("main/figure/simple-agent/collector/assemble"),
            &json!({"max_iter": n}),
        );
    }
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the instantiated figure boots");
    // The agent's menu before the first turn, through the figure's own lane.
    h.send(rim_in("mutation_committed", json!({}), Value::Null))
        .await;
    for answerer in [
        "/figure/simple-agent/tools",
        "/figure/simple-agent/curator",
        "/figure/simple-agent/memory-hive",
    ] {
        wait_route(&h, answerer, "/figure/simple-agent/collector", "in_menu", 1).await;
    }
    h.quiesce().await;
    (td, stubs, h, Ports { sink: sink_rx })
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
            // Only what LEAVES the figure (`figure_rim` out): the sink also sees
            // the messages the test sends into it, on the same lane names.
            if route_of(&m) == route
                && m.headers.hop.get("figure_rim").and_then(|v| v.as_str()) == Some("out")
            {
                return m;
            }
            seen.push(m);
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(m) => m,
        Err(_) => panic!(
            "HANG: the figure never emitted `{route}`; the road:\n{}",
            road(h).await
        ),
    }
}

/// `from -> to route` of the last logged messages (the store's own traffic
/// left out), oldest first (insertion order; ids are not causal).
async fn road(h: &ColonyHandle) -> String {
    let mut rows = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    rows.reverse(); // newest first from the log: oldest first here
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
                ["error_code", "finish_reason", "phase"]
                    .iter()
                    .filter_map(|k| hop[*k].as_str().map(|v| format!("{k}={v}")))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn tool_names(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect()
}

/// How many logged messages on `route` went from under `from` to under `to`.
async fn count_route(h: &ColonyHandle, from: &str, to: &str, route: &str) -> usize {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| {
            r.from_path.starts_with(from)
                && r.to_path.starts_with(to)
                && meclaw_core::serde_json::from_str::<Value>(&r.headers_json)
                    .is_ok_and(|h| route.is_empty() || h["hop"]["route"] == route)
        })
        .count()
}

/// Waits until `n` messages on `route` went from under `from` to under `to`:
/// event-driven over the log's commit counter, the hang mark as the deadline.
async fn wait_route(h: &ColonyHandle, from: &str, to: &str, route: &str, n: usize) {
    let mut seq = h.subscribe_log_seq();
    let wait = async {
        loop {
            seq.borrow_and_update();
            if count_route(h, from, to, route).await >= n {
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
            "the log closed before {n} x {route} {from} -> {to}; the road:\n{}",
            road(h).await
        ),
        Err(_) => panic!(
            "HANG: never {n} x {route} {from} -> {to}; the road:\n{}",
            road(h).await
        ),
    }
}

// ─────────────────────────────────────────────────────────────── the rim

/// One message onto the figure's rim: `slot` under the lane's own name, `hop`
/// laid over the route.
fn rim_in(route: &str, slot: Value, hop: Value) -> Message {
    let mut h = json!({"route": route});
    if let Some(extra) = hop.as_object() {
        for (k, v) in extra {
            h[k] = v.clone();
        }
    }
    let mut body = json!({"messages": []});
    if !slot.is_null() && slot != json!({}) {
        body[route] = slot;
    }
    MessageBuilder::new(Path::new(FIGURE))
        .hop(as_map(&h))
        .context(Map::new())
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

fn world_tool(name: &str) -> Value {
    json!({"name": name, "description": format!("{name} in the world."),
           "parameters": {"type": "object", "properties": {"at": {"type": "string"}}}})
}

fn obs(turn_id: u64, text: &str, tools: Value) -> Message {
    rim_in(
        "obs",
        json!({"turn_id": turn_id, "kind": "event", "text": text, "tools": tools, "day": 1}),
        Value::Null,
    )
}

/// An `obs` that carries the world's game time `at` (contract V-W).
fn obs_at(turn_id: u64, text: &str, at: &str) -> Message {
    rim_in(
        "obs",
        json!({"turn_id": turn_id, "kind": "event", "text": text, "tools": [], "day": 1,
               "at": at}),
        Value::Null,
    )
}

const GAME_TIME: &str = "2026-10-21T19:30:00Z";

fn call_result(call: &Message, result: Value) -> Message {
    let c = body_of(call)["call"].clone();
    rim_in(
        "call_result",
        json!({"turn_id": c["turn_id"], "call_id": c["call_id"], "result": result}),
        Value::Null,
    )
}

/// The colony's answer on the figure's human channel (`hp:<world>:<figure>`).
fn phone_answer(text: &str) -> Message {
    let chat = format!("hp:{WORLD_ID}:{FIGURE_ID}");
    let mut ctx = Map::new();
    ctx.insert("channel".into(), json!(chat));
    MessageBuilder::new(Path::new(FIGURE))
        .hop(as_map(&json!({"route": "answer", "chat_id": chat})))
        .context(ctx)
        .body(Body::Inline(
            json!({"messages": [], "answer": {"text": text}}),
        ))
        .ttl(400)
        .build()
}

/// `phone_answer` as the colony's collector sends it: `hop.round_id` names the
/// round the answer left; a second answer of the same turn carries the base id
/// with the suffix `~<ms>~<hash>` (GH #728).
fn phone_answer_round(text: &str, round_id: &str) -> Message {
    let chat = format!("hp:{WORLD_ID}:{FIGURE_ID}");
    let mut ctx = Map::new();
    ctx.insert("channel".into(), json!(chat));
    MessageBuilder::new(Path::new(FIGURE))
        .hop(as_map(
            &json!({"route": "answer", "chat_id": chat, "round_id": round_id}),
        ))
        .context(ctx)
        .body(Body::Inline(
            json!({"messages": [], "answer": {"text": text}}),
        ))
        .ttl(400)
        .build()
}

// ═══════════════════════════════════════════════ the turns (colony)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_calls_two_world_tools_and_ends_when_the_model_stops() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the well"}"#)]),
            canned_tool_calls(vec![("c-walk", "walk", r#"{"at":"the well"}"#)]),
            canned_chat_completion("I walked to the well.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(
        7,
        "You stand in the square.",
        json!([world_tool("look"), world_tool("walk")]),
    ))
    .await;
    let mut seen = Vec::new();
    let mut tools = Vec::new();
    for _ in 0..2 {
        let call = next_on(&h, &mut ports, "call", &mut seen).await;
        let c = body_of(&call)["call"].clone();
        assert_eq!(c["turn_id"], 7, "the world's own id comes back: {c}");
        tools.push(c["tool"].as_str().unwrap_or_default().to_string());
        h.send(call_result(
            &call,
            json!({"accepted": true, "seen": "water"}),
        ))
        .await;
    }
    assert_eq!(tools, ["look", "walk"], "two world tools, in order");
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 7, "{slot}");
    assert_eq!(slot["done"], true, "{slot}");
    assert_eq!(slot["text"], "I walked to the well.", "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "one world turn, one turn through the door, no handoff"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "three model calls in the one turn");
    let names = tool_names(&reqs[0]);
    for t in ["look", "walk", "phone_message"] {
        assert!(names.iter().any(|n| n == t), "client tool {t}: {names:?}");
    }
    assert!(
        wire_text(&reqs[2]).contains("water"),
        "the world's result is in the window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_message_leaves_as_a_human_channel_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![(
                "c-phone",
                "phone_message",
                r#"{"text":"Are you coming tonight?"}"#,
            )]),
            canned_chat_completion("I texted Robin.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs_at(8, "Evening.", GAME_TIME)).await;
    let mut seen = Vec::new();
    let out = next_on(&h, &mut ports, "turn", &mut seen).await;
    assert_eq!(
        out.headers.hop["chat_id"],
        format!("hp:{WORLD_ID}:{FIGURE_ID}"),
        "{:?}",
        out.headers.hop
    );
    assert!(
        !out.headers.hop.contains_key("speaker"),
        "a member speaks without a speaker key: {:?}",
        out.headers.hop
    );
    assert_eq!(
        out.headers.hop["happened_at"], GAME_TIME,
        "the message carries the world time of the last obs: {:?}",
        out.headers.hop
    );
    let msgs = body_of(&out)["messages"].clone();
    assert_eq!(
        msgs.as_array().map(Vec::len),
        Some(1),
        "one user turn: {msgs}"
    );
    assert_eq!(msgs[0]["origin"], "user", "{msgs}");
    assert_eq!(msgs[0]["text"], "Are you coming tonight?", "{msgs}");
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    assert_eq!(body_of(&intent)["intent"]["done"], true);
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    // The tool message itself, not the prompt around it (OR-SA-7).
    let results: Vec<Value> = reqs[1]
        .messages()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m["role"] == "tool")
        .collect();
    assert!(
        results.iter().any(|m| {
            let c = m["content"].as_str().unwrap_or_default();
            c.contains("\"status\"") && c.contains("\"sent\"")
        }),
        "texting is asynchronous: the tool result is `{{status: sent}}` at once: {results:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_answer_waits_for_the_open_world_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the market"}"#)]),
            canned_chat_completion("I looked around.", "stop"),
            canned_chat_completion("Yes, see you at eight.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(9, "Market day.", json!([world_tool("look")])))
        .await;
    let mut seen = Vec::new();
    let call = next_on(&h, &mut ports, "call", &mut seen).await;
    // The world turn is open (its call is out): the phone answer queues.
    h.send(phone_answer("Dinner at eight?")).await;
    wait_route(&h, "/figure/phone", "/figure/world", "phone_in", 1).await;
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "no phone turn while the world turn is open"
    );
    h.send(call_result(
        &call,
        json!({"accepted": true, "seen": "stalls"}),
    ))
    .await;
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    assert_eq!(body_of(&intent)["intent"]["text"], "I looked around.");
    let reply = next_on(&h, &mut ports, "turn", &mut seen).await;
    assert_eq!(
        body_of(&reply)["messages"][0]["text"],
        "Yes, see you at eight.",
        "the phone turn's final text is the reply"
    );
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        2,
        "the phone turn went in after the world turn"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3);
    let third = wire_text(&reqs[2]);
    assert!(
        third.contains("Message from Robin: Dinner at eight?"),
        "{third}"
    );
    assert!(
        tool_names(&reqs[2]).iter().all(|n| n != "look"),
        "a phone turn carries no world tools: {:?}",
        tool_names(&reqs[2])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_game_agent_switch_changes_only_the_prefix_and_speaker() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-phone", "phone_message", r#"{"text":"Ready."}"#)]),
            canned_chat_completion("Sent.", "stop"),
        ],
        json!({"phone": {"enabled": true, "speaker": "game_agent", "contact": "Robin"}}),
    )
    .await;
    h.send(obs(10, "Your move.", json!([]))).await;
    let mut seen = Vec::new();
    let out = next_on(&h, &mut ports, "turn", &mut seen).await;
    assert_eq!(
        out.headers.hop["chat_id"],
        format!("game:{WORLD_ID}:{FIGURE_ID}")
    );
    assert_eq!(out.headers.hop["speaker"], "game_agent");
    assert!(
        !out.headers.hop.contains_key("happened_at"),
        "an obs without `at` stamps no time, never an empty one: {:?}",
        out.headers.hop
    );
    let msgs = body_of(&out)["messages"].clone();
    assert_eq!(msgs.as_array().map(Vec::len), Some(1), "{msgs}");
    assert_eq!(msgs[0]["text"], "Ready.", "the turn's form is unchanged");
}

/// An answer on the handset over `phone_replies_max` opens no turn and leaves
/// on the rim as `phone_capped` (GH #1292 CF): the world cell's contract has to
/// carry the route, or the runtime drops the message without a trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_answer_over_the_cap_leaves_on_the_rim_as_phone_capped() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![], json!({"phone_replies_max": 0})).await;
    h.send(phone_answer("Still there?")).await;
    let mut seen = Vec::new();
    let capped = next_on(&h, &mut ports, "phone_capped", &mut seen).await;
    let slot = body_of(&capped)["phone_capped"].clone();
    assert_eq!(slot["text"], "Still there?", "{slot}");
    assert_eq!(
        slot["channel"],
        format!("hp:{WORLD_ID}:{FIGURE_ID}"),
        "{slot}"
    );
    assert_eq!(slot["max"], 0, "{slot}");
    assert_eq!(slot["replies"], 0, "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        0,
        "a capped answer opens no turn"
    );
    assert!(
        stubs.brain.recorded_requests().await.is_empty(),
        "no model turn for a capped answer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn night_closes_both_sessions_and_acks_after_both_reports() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("A quiet day.", "stop"),
            canned_chat_completion("Good night, Robin.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(obs(11, "Noon.", json!([]))).await;
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.send(phone_answer("Sleep well.")).await;
    next_on(&h, &mut ports, "turn", &mut seen).await;
    h.send(rim_in("night", json!({"day": 1}), Value::Null))
        .await;
    let night = next_on(&h, &mut ports, "night", &mut seen).await;
    // The receipt only after both close reports: settled first, then the
    // order read off the log's insertion order (the writer commits in batches;
    // `read_log` hands the rows newest first, a shared second by insertion).
    h.quiesce().await;
    let mut rows = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    rows.reverse();
    let route_is = |r: &meclaw_colony::api_dto::MessageLogDto, want: &str| {
        meclaw_core::serde_json::from_str::<Value>(&r.headers_json)
            .is_ok_and(|h| h["hop"]["route"] == want)
    };
    let reports: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.from_path.starts_with("/figure/simple-agent") && r.to_path == "/figure/world"
        })
        .filter(|(_, r)| route_is(r, "close_report"))
        .map(|(i, _)| i)
        .collect();
    let receipt = rows.iter().position(|r| {
        r.from_path == "/figure/world"
            && !r.to_path.starts_with("/figure/simple-agent")
            && route_is(r, "night")
    });
    assert_eq!(
        reports.len(),
        2,
        "two close reports; the road:\n{}",
        road(&h).await
    );
    assert!(
        receipt.is_some_and(|n| reports.iter().all(|r| *r < n)),
        "the night answered before both reports; the road:\n{}",
        road(&h).await
    );
    let slot = body_of(&night)["night"].clone();
    assert_eq!(slot["day"], 1, "{slot}");
    assert_eq!(slot["ok"], true, "{slot}");
    let closes: Vec<String> = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| r.from_path == "/figure/world" && r.to_path.starts_with("/figure/simple-agent"))
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "close")
        .filter_map(|h| h["hop"]["session_id"].as_str().map(str::to_string))
        .collect();
    let mut closes = closes;
    closes.sort();
    assert_eq!(closes, ["day-1", "phone-1"], "both sessions of the day");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interview_error_names_its_error_code() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(vec![canned_error_status(400)], json!({})).await;
    h.send(rim_in(
        "interview",
        json!({"question_id": "q-1", "question": "What did you do today?"}),
        Value::Null,
    ))
    .await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "interview", &mut seen).await;
    let slot = body_of(&answer)["interview"].clone();
    assert_eq!(slot["question_id"], "q-1", "{slot}");
    assert!(slot["answer"].is_null(), "{slot}");
    let code = slot["error_code"].as_str().unwrap_or_default();
    assert!(
        !code.is_empty() && slot["error"] == slot["error_code"],
        "a failed interview names its code: {slot}; the road:\n{}",
        road(&h).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ai_question_is_answered_honestly_in_both_roles() {
    if !shipped() {
        return;
    }
    for (role, marker) in [("human-sim", "living the life"), ("npc", "game world")] {
        let (_td, stubs, h, mut ports) = boot(
            vec![canned_chat_completion("Yes, I am an AI.", "stop")],
            json!({"role": role}),
        )
        .await;
        h.send(rim_in(
            "interview",
            json!({"question_id": "q-ai", "question": "Are you an AI?"}),
            Value::Null,
        ))
        .await;
        let mut seen = Vec::new();
        let answer = next_on(&h, &mut ports, "interview", &mut seen).await;
        assert_eq!(
            body_of(&answer)["interview"]["answer"],
            "Yes, I am an AI.",
            "{role}"
        );
        h.quiesce().await;
        let reqs = stubs.brain.recorded_requests().await;
        let prompt = wire_text(&reqs[0]);
        assert!(prompt.contains(marker), "{role}: its frame: {prompt}");
        assert!(
            prompt.contains("say truthfully that you are"),
            "{role}: truthful about being an AI: {prompt}"
        );
        assert!(
            prompt.contains("You are an AI agent"),
            "{role}: the agent's own frame stays beside the role: {prompt}"
        );
    }
}

/// R-NPC-14 (GH #1292 FT): a world turn whose model answers with text alone
/// gets exactly one follow-up turn that asks for an action from the world's
/// menu; here the figure then acts, and the world turn ends as one intent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_that_ends_in_text_alone_is_asked_once_for_an_action() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("Fire! I run to the market.", "stop"),
            canned_tool_calls(vec![("c-walk", "walk", r#"{"at":"the market"}"#)]),
            canned_chat_completion("I am at the market.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(
        11,
        "Smoke rises over the market.",
        json!([world_tool("walk")]),
    ))
    .await;
    let mut seen = Vec::new();
    let call = next_on(&h, &mut ports, "call", &mut seen).await;
    let c = body_of(&call)["call"].clone();
    assert_eq!(
        c["turn_id"], 11,
        "the world's own id, also after the follow-up: {c}"
    );
    assert_eq!(c["tool"], "walk", "{c}");
    h.send(call_result(&call, json!({"accepted": true}))).await;
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 11, "{slot}");
    assert_eq!(slot["done"], true, "{slot}");
    let text = slot["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("I run to the market."),
        "the first text stays: {slot}"
    );
    assert!(text.contains("I am at the market."), "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        2,
        "the world turn and its one follow-up"
    );
    assert_eq!(
        count_route(&h, "/figure/world", "", "intent").await,
        1,
        "the text alone ended nothing on the rim"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "text, the follow-up's call, the final text");
    let follow = wire_text(&reqs[1]);
    assert!(
        follow.contains("Choose one action from the world's tools"),
        "the follow-up asks for an action: {follow}"
    );
    assert!(
        tool_names(&reqs[1]).iter().any(|n| n == "walk"),
        "the follow-up carries the turn's menu: {:?}",
        tool_names(&reqs[1])
    );
}

/// The follow-up is asked once: a second answer in text alone ends the world
/// turn as today, both texts in its intent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_is_asked_for_an_action_only_once() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("I keep watching.", "stop"),
            canned_chat_completion("I stay where I am.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(12, "Rain.", json!([world_tool("walk")]))).await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 12, "{slot}");
    assert_eq!(slot["done"], true, "{slot}");
    let text = slot["text"].as_str().unwrap_or_default();
    assert!(text.contains("I keep watching."), "{slot}");
    assert!(text.contains("I stay where I am."), "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        2,
        "one follow-up, no second"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 2);
}

/// A phone turn may stay silent or answer in text alone (OR-SA-43): no
/// follow-up, its text is the reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_turn_in_text_alone_gets_no_follow_up() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("See you at eight.", "stop")],
        json!({}),
    )
    .await;
    h.send(phone_answer("Dinner at eight?")).await;
    let mut seen = Vec::new();
    let reply = next_on(&h, &mut ports, "turn", &mut seen).await;
    assert_eq!(body_of(&reply)["messages"][0]["text"], "See you at eight.");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "the phone turn alone"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 1);
}

/// A world turn that hit the agent's loop cap (`partial`) has spent its
/// budget: no follow-up, even when no call of it was taken (GH #1292 FT).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_at_the_loop_cap_gets_no_follow_up() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot_capped(
        vec![canned_tool_calls(vec![(
            "c-fly",
            "fly",
            r#"{"at":"the market"}"#,
        )])],
        json!({}),
        Some(1),
    )
    .await;
    h.send(obs(13, "Smoke rises.", json!([world_tool("walk")])))
        .await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 13, "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "the capped turn alone, no follow-up"
    );
    assert_eq!(
        count_route(&h, "/figure/world", "", "call").await,
        0,
        "the call of a tool not offered never left"
    );
}

/// A `contact` turn's text is its answer to the one in front of the figure
/// (OR-SA.FT.1): text alone, world tools offered, no follow-up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_contact_turn_in_text_alone_gets_no_follow_up() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Hello, neighbour.", "stop")],
        json!({}),
    )
    .await;
    h.send(rim_in(
        "obs",
        json!({"turn_id": 14, "kind": "contact", "text": "Kim greets you.",
               "tools": [world_tool("walk")], "day": 1}),
        Value::Null,
    ))
    .await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 14, "{slot}");
    assert_eq!(
        slot["text"], "Hello, neighbour.",
        "the answer unchanged: {slot}"
    );
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "the contact turn alone"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 1);
}

/// A text message is an action: a world turn that only calls
/// `phone_message` is not asked for one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_that_only_texts_gets_no_follow_up() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![(
                "c-phone",
                "phone_message",
                r#"{"text":"Fire at the market!"}"#,
            )]),
            canned_chat_completion("I warned Robin.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(15, "Smoke rises.", json!([world_tool("walk")])))
        .await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 15, "{slot}");
    assert_eq!(slot["text"], "I warned Robin.", "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "the world turn alone"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 2);
}

/// A world that offers no tool of its own (the menu is `phone_message` alone)
/// asks for nothing: text alone ends the turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_world_turn_with_only_the_phone_on_the_menu_gets_no_follow_up() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("I sit and wait.", "stop")],
        json!({}),
    )
    .await;
    h.send(obs(16, "Quiet evening.", json!([]))).await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(slot["turn_id"], 16, "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "no follow-up without a world tool"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 1);
    let names = tool_names(&reqs[0]);
    assert!(
        names.iter().any(|n| n == "phone_message"),
        "the menu: {names:?}"
    );
    assert!(!names.iter().any(|n| n == "walk"), "the menu: {names:?}");
}

/// The answer of a follow-up whose world cell lost its open turn (a replaced
/// child) ends as the world turn's intent under the world's id, never under
/// `<id>:act`, and asks for nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_up_answer_after_a_replaced_child_ends_the_world_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(vec![], json!({})).await;
    let channel = format!("world:{WORLD_ID}");
    let mut ctx = Map::new();
    ctx.insert("channel".into(), json!(channel));
    h.send_from(
        Path::new("/figure/simple-agent"),
        MessageBuilder::new(Path::new("/figure/world"))
            .hop(as_map(&json!({"route": "answer", "turn_id": "17:act",
                                "channel": channel})))
            .context(ctx)
            .body(Body::Inline(json!({"messages": [],
                                      "answer": {"turn_id": "17:act", "text": "I walk."}})))
            .ttl(400)
            .build(),
    )
    .await;
    let mut seen = Vec::new();
    let intent = next_on(&h, &mut ports, "intent", &mut seen).await;
    let slot = body_of(&intent)["intent"].clone();
    assert_eq!(
        slot["turn_id"], "17",
        "the world's id, not the follow-up's: {slot}"
    );
    assert_eq!(slot["done"], true, "{slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        0,
        "no further follow-up"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 0);
}

// ═══════════════════════════════════════════════ instance params (C-T6)

fn registry_of(dir: &std::path::Path) -> TemplatesRegistry {
    let scanned = scan_templates_dir(dir).unwrap_or_else(|e| panic!("scan {}: {e}", dir.display()));
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

/// An instance addresses the cells inside the referenced agent by their path
/// (`agent/brain`, `agent/door`, `agent/tools/file`): the real staging path
/// writes the values into the staged configs. No model is called.
#[test]
fn an_instance_override_reaches_the_agent_brain() {
    if !shipped() {
        return;
    }
    let registry = registry_of(&repo("templates"));
    let root = tempfile::tempdir().expect("tempdir");
    let mut files = Vec::new();
    // Every template a ref of the figure may pull in: the bare tokens of all.
    configs_under(&repo("templates"), &mut files);
    let env: HashMap<String, String> = bare_env_tokens(&files).into_iter().collect();
    let ctx: HashMap<String, String> = [("model".to_string(), "the-template-model".to_string())]
        .into_iter()
        .collect();
    let seed = json!({"standard": {"protocol": "AIEOS", "version": "1.1.0"},
                      "identity": {"names": {"first": "Ava"}}});
    let overrides = SubtreeOverrides::from_add_node(&json!({"override_params": {
        "world": world_params(&json!({})),
        "simple-agent/brain": {"model": "the-instance-model", "x_title": "figure-lab f1",
                        "http_referer": "https://meclaw.ai"},
        "simple-agent/door": {"agent_id": "ava", "aieos": seed},
        "simple-agent/tools/file": {"base_path": "ava-files"}
    }}));
    let staged = stage_subtree(
        root.path(),
        "m-figure-ct6",
        "/main",
        "ava",
        &repo("templates/figure"),
        &env,
        &ctx,
        None,
        &overrides,
        &registry,
        &Default::default(),
        &meclaw_colony::WorkPulse::silent(),
        meclaw_colony::mutation::Birth::Active,
    )
    .unwrap_or_else(|e| panic!("stage the figure: {e:?}"));
    let params_of = |abs: &str| -> Value {
        staged
            .cells
            .iter()
            .find(|c| c.absolute_path.as_str() == abs)
            .map(|c| c.params.clone())
            .unwrap_or_else(|| {
                let known: Vec<&str> = staged
                    .cells
                    .iter()
                    .map(|c| c.absolute_path.as_str())
                    .collect();
                panic!("no staged cell at {abs:?}; staged: {known:?}")
            })
    };
    let brain = params_of("/main/ava/simple-agent/brain");
    assert_eq!(brain["model"], "the-instance-model", "{brain}");
    assert_eq!(brain["x_title"], "figure-lab f1", "{brain}");
    assert_eq!(brain["http_referer"], "https://meclaw.ai", "{brain}");
    let door = params_of("/main/ava/simple-agent/door");
    assert_eq!(door["agent_id"], "ava", "{door}");
    assert_eq!(door["aieos"]["identity"]["names"]["first"], "Ava", "{door}");
    let file = params_of("/main/ava/simple-agent/tools/file");
    assert_eq!(file["base_path"], "ava-files", "{file}");
    // What the instance did not name keeps the template's value.
    let curator_brain = params_of("/main/ava/simple-agent/curator/summarizer");
    assert_ne!(
        curator_brain["model"], "the-instance-model",
        "the override is per cell, not per tree: {curator_brain}"
    );
}

/// GH #1292 FA: the follow-up of a world turn requires a tool call of its
/// FIRST brain call (`tool_choice: "required"` on the wire); the world turn
/// before it and the call after the tool result stay free.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_follow_up_requires_a_tool_call_of_its_first_brain_call_only() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("Fire! I run to the market.", "stop"),
            canned_tool_calls(vec![("c-walk", "walk", r#"{"at":"the market"}"#)]),
            canned_chat_completion("I am at the market.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(obs(
        21,
        "Smoke rises over the market.",
        json!([world_tool("walk")]),
    ))
    .await;
    let mut seen = Vec::new();
    let call = next_on(&h, &mut ports, "call", &mut seen).await;
    h.send(call_result(&call, json!({"accepted": true}))).await;
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "text, the follow-up's call, the final text");
    assert!(
        reqs[0].body.get("tool_choice").is_none(),
        "the world turn itself is free: {}",
        reqs[0].body["tool_choice"]
    );
    assert_eq!(
        reqs[1].body["tool_choice"], "required",
        "the follow-up requires a call: {}",
        reqs[1].body["tool_choice"]
    );
    assert!(
        reqs[2].body.get("tool_choice").is_none(),
        "after the result the turn may end in text: {}",
        reqs[2].body["tool_choice"]
    );
}

/// GH #1292 FA (OR-SA-43, R-NPC-14): a phone turn, a contact turn and an
/// interview turn require no tool call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_contact_and_interview_turns_require_no_tool_call() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("See you at eight.", "stop"),
            canned_chat_completion("Hello, neighbour.", "stop"),
            canned_chat_completion("I walked to the market.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(phone_answer("Dinner at eight?")).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "turn", &mut seen).await;
    h.send(rim_in(
        "obs",
        json!({"turn_id": 22, "kind": "contact", "text": "Kim greets you.",
               "tools": [world_tool("walk")], "day": 1}),
        Value::Null,
    ))
    .await;
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.send(rim_in(
        "interview",
        json!({"question_id": "q-fa", "question": "What did you do today?"}),
        Value::Null,
    ))
    .await;
    next_on(&h, &mut ports, "interview", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        3,
        "the phone, the contact and the interview turn"
    );
    for (i, r) in reqs.iter().enumerate() {
        assert!(
            r.body.get("tool_choice").is_none(),
            "call {i} requires nothing: {}",
            r.body["tool_choice"]
        );
    }
}

/// GH #1292 FA2 (village M3b3): a second answer of the same colony turn
/// (`<round_id>~<ms>~<hash>`) reaches the figure once: no second phone turn,
/// and it does not spend a reply of `phone_replies_max`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_answer_of_the_same_round_is_delivered_once() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("See you at eight.", "stop")],
        json!({"phone_replies_max": 1}),
    )
    .await;
    // The handset is a pool (`warm`): answers sent together may reach the
    // world in any order, so the round's first answer is through before the
    // repeat and the next round are sent.
    h.send(phone_answer_round("Dinner at eight?", "r-1")).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "turn", &mut seen).await;
    h.send(phone_answer_round(
        "Dinner at eight?",
        "r-1~1761130000000~ab12cd34",
    ))
    .await;
    h.send(phone_answer_round("Still there?", "r-2")).await;
    let capped = next_on(&h, &mut ports, "phone_capped", &mut seen).await;
    let slot = body_of(&capped)["phone_capped"].clone();
    assert_eq!(
        slot["text"], "Still there?",
        "the repeat neither opened a turn nor was capped: {slot}"
    );
    assert_eq!(slot["replies"], 1, "one reply spent, not two: {slot}");
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        1,
        "exactly one phone turn for the round"
    );
    assert_eq!(
        count_route(&h, "/figure/world", "/figure", "phone_capped").await,
        1,
        "the repeat is dropped, not capped"
    );
    assert_eq!(stubs.brain.recorded_requests().await.len(), 1);
}

/// GH #1292 FA3: a second answer of the same round with another text is a
/// correction, read like one on a phone: it opens no turn and spends no reply,
/// and waits unread for the figure's next world turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corrected_answer_of_the_same_round_waits_unread() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("See you at eight.", "stop"),
            canned_chat_completion("I sit and wait.", "stop"),
        ],
        json!({"phone_replies_max": 1}),
    )
    .await;
    h.send(phone_answer_round("Dinner at eight?", "r-1")).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "turn", &mut seen).await;
    // Each answer through before the next: the handset is a pool (`warm`).
    h.send(phone_answer_round(
        "Sorry, dinner at nine.",
        "r-1~1761130000000~ab12cd34",
    ))
    .await;
    h.quiesce().await;
    h.send(phone_answer_round("Still there?", "r-2")).await;
    let capped = next_on(&h, &mut ports, "phone_capped", &mut seen).await;
    let slot = body_of(&capped)["phone_capped"].clone();
    assert_eq!(slot["text"], "Still there?", "{slot}");
    assert_eq!(slot["replies"], 1, "the correction spent no reply: {slot}");
    h.send(obs(17, "Quiet evening.", json!([]))).await;
    next_on(&h, &mut ports, "intent", &mut seen).await;
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/figure/world", "/figure/simple-agent", "turn").await,
        2,
        "one phone turn for the round, then the world turn"
    );
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    let world = wire_text(&reqs[1]);
    assert!(
        world.contains("- Sorry, dinner at nine.") && world.contains("- Still there?"),
        "the correction and the capped answer wait unread: {world}"
    );
}
