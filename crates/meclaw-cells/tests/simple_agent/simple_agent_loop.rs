//! GH #1292 -- `templates/simple-agent`, the loop: a colony of the
//! instantiated template with stub models (no real model call).
//!
//! A turn loops over the agent's own tools until the model stops; a client
//! tool leaves on `calls` and its `tool_result` continues the same turn; the
//! client menu lives for one turn; a client tool named like an own one is
//! dropped; a missing client result ends the turn named (the round's
//! watchdog); a result without its round keys is refused named; the loop stops
//! at `max_iter` with a partial answer; `close` reports after the close pass;
//! the AIeOS seed reaches the brain as affinity's pack, again on change; the
//! sidecar's memory section reaches the memory hive.
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
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::deadline::HANG;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

const AGENT: &str = "/agent";
const VAULT_CELL: &str = "/main/agent/access/vault";
const VAULT_PASSPHRASE: &[u8] = b"simple-agent-test-passphrase";
const BRAIN_MODEL: &str = "brain-stub";
const AUDIENCE: &str = "[\"member:caller\"]";

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
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

struct Stubs {
    brain: MockOpenAI,
    background: MockOpenAI,
}

/// Every `llm` cell at a stub (the brain at the scripted one), every model of
/// the tree named, every timer out of the run's way, the weather's endpoints
/// at the background stub (no network), the env file and the provider key
/// deposited in the agent's own vault -- the grants stay the template's.
/// `overlay` maps a cell path of the agent to params laid over it.
fn build(td: &tempfile::TempDir, stubs: &Stubs, overlay: &Value) {
    let root = td.path();
    let main = root.join("main");
    let agent = main.join("agent");
    copy_resolved(&repo("templates/simple-agent"), &agent, 0);
    override_params_on_disk(
        &agent.join("tools/weather"),
        &json!({"geocode_endpoint": format!("{}/v1/geocode", stubs.background.base_url),
                "forecast_endpoint": format!("{}/v1/forecast", stubs.background.base_url)}),
    );
    for (cell, params) in as_map(overlay) {
        override_params_on_disk(&agent.join(cell), &params);
    }
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
        {"from": "./agent", "to": "/sink", "condition": "has(hop.route)"}
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
    overlay: Value,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    let stubs = Stubs {
        brain: MockOpenAI::start(brain).await,
        // One answer for every background model: an empty verdict is a close
        // pass that changes nothing, and a summary all the same.
        background: MockOpenAI::start(vec![canned_chat_completion("{}", "stop")]).await,
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs, &overlay);
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
        .expect("the instantiated agent boots");
    // The menu before the first turn: a colony asks its collector after a
    // mutation; here the rim lane does, and the run waits for the answers.
    h.send(
        MessageBuilder::new(Path::new(AGENT))
            .hop(as_map(&json!({"route": "mutation_committed"})))
            .context(Map::new())
            .body(Body::Inline(json!({"messages": []})))
            .ttl(400)
            .build(),
    )
    .await;
    for answerer in ["/agent/tools", "/agent/curator", "/agent/memory-hive"] {
        wait_route(&h, answerer, "/agent/collector", "in_menu", 1).await;
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
            if route_of(&m) == route {
                return m;
            }
            seen.push(m);
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(m) => m,
        Err(_) => panic!(
            "HANG: the agent never emitted `{route}`; the road:\n{}",
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

// ─────────────────────────────────────────────────────────────── the rim

fn look_tool() -> Value {
    json!({"name": "look", "description": "Look at something in front of you.",
           "parameters": {"type": "object", "properties": {"at": {"type": "string"}},
                          "required": ["at"]}})
}

/// One `turn` on the door: hop keys of V-T, a user text, the client tools.
fn turn(turn_id: &str, text: &str, tools: Value) -> Message {
    let mut body = json!({"messages": [{"origin": "user", "type": "text", "text": text}]});
    if !tools.is_null() {
        body["tools"] = tools;
    }
    MessageBuilder::new(Path::new(AGENT))
        .hop(as_map(
            &json!({"route": "turn", "turn_id": turn_id, "session_id": "s1",
                             "channel": "c1", "audience_set": AUDIENCE}),
        ))
        .context(Map::new())
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

/// The `tool_result` for a `calls` message: `keys` of its hop come back.
fn result_for(calls: &Message, keys: &[&str], content: &str) -> Message {
    let mut hop = json!({"route": "tool_result"});
    for k in keys {
        if let Some(v) = calls.headers.hop.get(*k) {
            hop[*k] = v.clone();
        }
    }
    let id = body_of(calls)["calls"]["id"].clone();
    MessageBuilder::new(Path::new(AGENT))
        .hop(as_map(&hop))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": [],
                                   "tool_result": {"id": id, "content": content}})))
        .ttl(400)
        .build()
}

const ROUND_KEYS: [&str; 5] = ["turn_id", "session_id", "iter", "channel", "audience_set"];

fn tool_names(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect()
}

fn tool_description(req: &OpenAiRequestSnapshot, name: &str) -> String {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|t| t["function"]["name"] == name)
        .and_then(|t| t["function"]["description"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// How many logged messages on `route` (any route when empty) went from under `from` to under `to`.
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

fn aieos(motivation: &str) -> Value {
    json!({"standard": {"protocol": "AIEOS", "version": "1.1.0"},
           "metadata": {"instance_id": "ava-1"},
           "identity": {"names": {"first": "Ava", "last": "Quill"}},
           "motivations": {"core_drive": motivation}})
}

// ═══════════════════════════════════════════════ the loop (colony)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_loops_over_own_tools_until_the_model_stops() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_tool_calls(vec![(
                "c-file",
                "file",
                r#"{"op":"write","path":"note.txt","content":"hello"}"#,
            )]),
            canned_chat_completion("Done.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "Note the weather.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    assert_eq!(slot["text"], "Done.", "{slot}");
    assert!(slot.get("partial").is_none(), "a full answer: {slot}");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "three model calls for one answer");
    let names = tool_names(&reqs[0]);
    for t in ["weather", "file", "bash", "edit"] {
        assert!(names.iter().any(|n| n == t), "own tool {t}: {names:?}");
    }
    for (i, id) in [(1, "c-weather"), (2, "c-file")] {
        let msgs = reqs[i].messages().cloned().unwrap_or_default();
        assert!(
            msgs.iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == id),
            "call {i} carries the result of {id}: {}",
            wire_text(&reqs[i])
        );
    }
    assert!(
        seen.iter().all(|m| route_of(m) != "calls"),
        "own tools never leave the hive"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_leaves_as_calls_and_its_result_continues_the_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("I see a red door.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    for k in ROUND_KEYS
        .iter()
        .chain(["tool_name", "tool_call_id"].iter())
    {
        assert!(
            calls.headers.hop.contains_key(*k),
            "calls carries {k}: {:?}",
            calls.headers.hop
        );
    }
    assert_eq!(calls.headers.hop["turn_id"], "t-1");
    let slot = body_of(&calls)["calls"].clone();
    assert_eq!(slot["name"], "look", "{slot}");
    assert_eq!(slot["arguments"]["at"], "the door", "{slot}");
    assert!(
        seen.iter().all(|m| route_of(m) != "answer"),
        "the client call does not end the turn"
    );
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "I see a red door.");
    assert_eq!(body_of(&answer)["answer"]["turn_id"], "t-1");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "the same turn went on: two model calls");
    assert!(
        wire_text(&reqs[1]).contains("A red door."),
        "the client result is in the window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_tools_are_in_the_menu_of_the_same_turn_and_gone_in_the_next() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("First.", "stop"),
            canned_chat_completion("Second.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn("t-1", "One.", json!([look_tool()]))).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.send(turn("t-2", "Two.", Value::Null)).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    assert!(
        tool_names(&reqs[0]).contains(&"look".to_string()),
        "the client tool is in the menu of its turn: {:?}",
        tool_names(&reqs[0])
    );
    assert!(
        !tool_names(&reqs[1]).contains(&"look".to_string()),
        "a turn without tools[] empties the client menu: {:?}",
        tool_names(&reqs[1])
    );
    assert!(
        tool_names(&reqs[1]).contains(&"weather".to_string()),
        "the own tools stay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_named_like_an_own_tool_is_dropped() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Fine.", "stop")], json!({})).await;
    let fake = json!({"name": "weather", "description": "CLIENT WEATHER",
                      "parameters": {"type": "object", "properties": {}}});
    h.send(turn("t-1", "Hi.", json!([fake, look_tool()]))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    let names = tool_names(&reqs[0]);
    assert_eq!(
        names.iter().filter(|n| *n == "weather").count(),
        1,
        "{names:?}"
    );
    assert!(names.contains(&"look".to_string()), "{names:?}");
    assert!(
        !tool_description(&reqs[0], "weather").contains("CLIENT"),
        "the own weather stays"
    );
    let dropped = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_turn")
        .filter_map(|h| {
            h["hop"]["client_tools_dropped"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(
        !dropped.is_empty() && dropped.iter().all(|d| d == "[\"weather\"]"),
        "the turn names the drop: {dropped:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_named_like_an_own_family_is_dropped() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Fine.", "stop")], json!({})).await;
    let fake = json!({"name": "web", "description": "CLIENT WEB",
                      "parameters": {"type": "object", "properties": {}}});
    h.send(turn("t-1", "Hi.", json!([fake, look_tool()]))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        !tool_description(&reqs[0], "web").contains("CLIENT"),
        "the own web family stays: {:?}",
        tool_names(&reqs[0])
    );
    let dropped = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_turn")
        .filter_map(|h| {
            h["hop"]["client_tools_dropped"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(
        !dropped.is_empty() && dropped.iter().all(|d| d == "[\"web\"]"),
        "the door names the drop of the family word: {dropped:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_turn_while_a_client_call_waits_is_refused_busy() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("I see a red door.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    let other = json!({"name": "smell", "description": "Smell something.",
                       "parameters": {"type": "object", "properties": {}}});
    h.send(turn("t-2", "And now?", json!([other]))).await;
    let err = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&err)["error"].clone();
    assert_eq!(slot["error_code"], "turn_busy", "{slot}");
    assert_eq!(slot["turn_id"], "t-2", "{slot}");
    assert_eq!(slot["open_turn"], "t-1", "{slot}");
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["turn_id"], "t-1");
    assert_eq!(body_of(&answer)["answer"]["text"], "I see a red door.");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "the refused turn opened no round");
    let names = tool_names(&reqs[1]);
    assert!(
        names.contains(&"look".to_string()) && !names.contains(&"smell".to_string()),
        "the open turn's menu row stays: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_memory_comes_back_in_a_callers_recall() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-rec", "memory_recall", r#"{"query":"rowan tree"}"#)]),
            canned_chat_completion("A rowan.", "stop"),
        ],
        json!({"door": {"start_memories": [{"what": "You planted a rowan tree by the lighthouse."}]}}),
    )
    .await;
    h.send(turn("t-1", "What did you plant?", Value::Null))
        .await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        wire_text(&reqs[1]).contains("rowan tree by the lighthouse"),
        "the caller's recall finds the start memory: {}",
        wire_text(&reqs[1])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_client_result_ends_the_turn_named_not_silent() {
    if !shipped() {
        return;
    }
    // A small idle window: the round's watchdog fires 1 s after it.
    let idle = json!({"round_idle_ms": 1500});
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("I could not look.", "stop"),
        ],
        json!({"door": idle, "collector/assemble": idle}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "calls", &mut seen).await;
    // No result: the watchdog fires, the collector sweeps the round.
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    assert_eq!(
        slot["stale"], true,
        "the answer names the lost result: {slot}"
    );
    assert!(
        // The timer delivers its strike to `emit_to` itself (origin `@external`);
        // the relative `emit_to` resolves over the hive's edge ./watch -> ./collector.
        count_route(&h, "@external", "/agent/collector", "in_round_sweep").await >= 1,
        "the watchdog fired; the road:\n{}",
        road(&h).await
    );
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        wire_text(&reqs[1]).contains("tool result lost"),
        "the brain saw the lost result: {}",
        wire_text(&reqs[1])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_result_without_its_iter_is_refused_named() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("A door.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    h.send(result_for(&calls, &["turn_id"], "A door.")).await;
    let err = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&err)["error"].clone();
    assert_eq!(slot["error_code"], "tool_result_unkeyed", "{slot}");
    assert!(
        slot["missing"]
            .as_array()
            .is_some_and(|m| m.contains(&json!("iter"))),
        "{slot}"
    );
    // The keyed result still continues the turn.
    h.send(result_for(&calls, &ROUND_KEYS, "A door.")).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "A door.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_loop_stops_at_max_iter_with_a_partial_answer() {
    if !shipped() {
        return;
    }
    let call = |id: &str| canned_tool_calls(vec![(id, "weather", r#"{"place":"Berlin"}"#)]);
    let (_td, stubs, h, mut ports) = boot(
        vec![call("c-1"), call("c-2"), call("c-3"), call("c-4")],
        json!({"collector/assemble": {"max_iter": 2}}),
    )
    .await;
    h.send(turn("t-1", "Loop.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["partial"], true, "the capped turn says so: {slot}");
    h.quiesce().await;
    assert!(
        stubs.brain.recorded_requests().await.len() <= 3,
        "the cap stopped the loop"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_reports_after_the_close_pass_and_triggers_the_dream() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Hello.", "stop")], json!({})).await;
    let mut seen = Vec::new();
    h.send(turn("t-1", "Hi.", Value::Null)).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.send(
        MessageBuilder::new(Path::new(AGENT))
            .hop(as_map(&json!({"route": "close"})))
            .context(Map::new())
            .body(Body::Inline(
                json!({"messages": [], "close": {"session_id": "s1"}}),
            ))
            .ttl(400)
            .build(),
    )
    .await;
    let report = next_on(&h, &mut ports, "close_report", &mut seen).await;
    let slot = body_of(&report)["close_report"].clone();
    assert_eq!(slot["session_id"], "s1", "{slot}");
    assert_eq!(slot["ok"], true, "{slot}");
    assert!(
        count_route(&h, "/agent/memory-hive", "/agent/door", "close_report").await >= 1,
        "the report follows the memory's close pass"
    );
    wait_route(&h, "/agent/door", "/agent/memory-hive", "clock_op", 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_aieos_seed_reaches_the_brain_as_a_pack_once_and_again_on_change() {
    if !shipped() {
        return;
    }
    let changed = json!({"op": "upsert_entity", "entity_id": "entity:ava", "kind": "agent",
                         "display_name": "Ava Quill", "aieos": aieos("to map the coast")});
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-aff", "affinity_propose", &changed.to_string())]),
            canned_chat_completion("Noted.", "stop"),
        ],
        json!({"door": {"agent_id": "ava", "aieos": aieos("to keep the lighthouse")}}),
    )
    .await;
    h.send(turn("t-1", "Who are you?", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Noted.");
    let reqs = stubs.brain.recorded_requests().await;
    let first = system_text(&reqs[0]);
    assert!(
        first.contains("Ava"),
        "the first turn knows the agent: {first}"
    );
    assert!(first.contains("lighthouse"), "{first}");
    assert!(first.contains("You are an AI agent"), "the frame: {first}");
    assert!(
        wire_text(&reqs[1]).contains("accepted"),
        "the affinity tool's ack is its result: {}",
        wire_text(&reqs[1])
    );
    // The change sends a second pack, and each is booked.
    wait_route(&h, "/agent/affinity", "/agent/curator", "in_pack", 2).await;
    wait_route(&h, "/agent/curator", "/agent/affinity", "in_pack_ack", 2).await;
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/agent/door", "/agent/affinity", "in_propose").await,
        4,
        "boot (upsert, disclosure, subscribe) once, plus the tool's write"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sidecar_memory_section_reaches_the_memory_hive() {
    if !shipped() {
        return;
    }
    let block = json!({"memory": {"nothing_new": false,
                                  "facts": [{"text": "The caller keeps bees.",
                                             "source": "I keep bees."}],
                                  "topic": {"movement": "continue"}}});
    let text = format!("Bees, then.\n```sidecar\n{block}\n```");
    let (_td, _stubs, h, mut ports) =
        boot(vec![canned_chat_completion(&text, "stop")], json!({})).await;
    h.send(turn("t-1", "I keep bees.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert!(
        !body_of(&answer)["answer"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("sidecar"),
        "the block never reaches the caller"
    );
    wait_route(&h, "/agent/splitter", "/agent/curator", "in_section", 1).await;
    wait_route(&h, "/agent/curator", "/agent/memory-hive", "in_remember", 1).await;
}

/// `in_turn` hops the door handed the collector, from the log.
async fn in_turn_hops(h: &ColonyHandle) -> Vec<Value> {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| {
            r.from_path.starts_with("/agent/door") && r.to_path.starts_with("/agent/collector")
        })
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .map(|h| h["hop"].clone())
        .filter(|hop| hop["route"] == "in_turn")
        .collect()
}

/// GH #1292 FA: a turn whose body says `tool_scope: {require: true}` asks its
/// FIRST brain call for a tool call (`tool_choice: "required"`, the normal
/// form every agent has); the next call of the same tool loop goes without,
/// so the turn can end in text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_requires_a_tool_call_requires_it_of_its_first_brain_call_only() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_chat_completion("Sunny.", "stop"),
        ],
        json!({}),
    )
    .await;
    let mut t = turn("t-1", "Weather?", Value::Null);
    if let Body::Inline(b) = &mut t.body {
        b["tool_scope"] = json!({"require": true});
    }
    h.send(t).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Sunny.");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "the required call, then the answer");
    assert_eq!(
        reqs[0].body["tool_choice"], "required",
        "the first call of the turn requires a tool call: {}",
        reqs[0].body
    );
    assert!(
        reqs[1].body.get("tool_choice").is_none(),
        "the next call of the loop is free again: {}",
        reqs[1].body["tool_choice"]
    );
}

/// GH #1292 FA: a turn without `tool_scope` is the turn of before -- no
/// requirement on the wire and no new key on the door's `in_turn`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_a_requirement_reaches_the_wire_unchanged() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_chat_completion("Sunny.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "Weather?", Value::Null)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    for (i, r) in reqs.iter().enumerate() {
        assert!(
            r.body.get("tool_choice").is_none(),
            "call {i} carries no tool_choice: {}",
            r.body["tool_choice"]
        );
    }
    let hops = in_turn_hops(&h).await;
    assert_eq!(hops.len(), 1, "{hops:?}");
    assert!(
        hops[0].get("tool_require").is_none(),
        "the door's in_turn is the one of before: {}",
        hops[0]
    );
}
