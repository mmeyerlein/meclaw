//! GH #863 — a push the addressed `llm` cell refuses reaches the registry, and
//! `show` says so.
//!
//! Before 0.47.0 a refused push left the brain as an ordinary error: a
//! conversation error in `talky` (`./brain -> ./errors`), `route error` in
//! `cogny`, a VERDICT in the memory hive's glue and in `argus` (their out-edges
//! carried no condition). The registry never learned it, so `show` named a
//! model the cell did not run. What is claimed here, measured at the seam:
//!
//! 1. **The cell.** The refusal of a params push addressed to the cell ITSELF
//!    (`hop.subscriber` is its own path, GH #862) carries two header keys of its
//!    own: `refused_subscriber` (the cell's path, from the sink the substrate
//!    stamps, never from the message) and `refused_model` (the `model` the push
//!    named, empty when it named none or one past the registry's bound). It
//!    stays `finish_reason error` with its `error_code`. A refusal of anything
//!    else -- the operator's push without an address, a push for another cell --
//!    keeps the shape it had, byte for byte.
//! 2. **The tree.** The shipped registry beside the shipped `talky`, wired the
//!    way the builder wires a generation: the push edge onto the composite's
//!    `in_model` door and the way back from the composite on `model_refused`,
//!    restamped `in_refused` for the registry. A push onto an endpoint the brain
//!    has no `base_url_allow` for is refused, `show` names the refusal and the
//!    model it refused within the deadline, nothing reaches `./errors`, and
//!    clearing the replacement puts the row back on its start value with
//!    `refused` empty.
//!
//! The shipped road -- the shell, a generation grown by the recipe -- is
//! measured in `gh855_a_grown_assistant_is_a_subscriber.rs`
//! (`a_refused_push_on_the_shipped_road_reaches_show`); this file is the cheap
//! seam. Free of a paid call by construction: the only provider is the
//! in-process mock, and a push never reaches it. R2b / GH #49: a tree without
//! the templates skips the colony half.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::MockOpenAI;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the cell

/// The cell's own address -- the path the substrate stamps on its sink.
const OWN: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";
/// An endpoint the cell was not born on and has no allow list for.
const ELSEWHERE: &str = "https://elsewhere.example/v1";

fn sink_at(own: &str) -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(own),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// A cell on a fresh `cell.db`, born on the mock with NO `base_url_allow`: its
/// `base_url` is fixed at run time.
fn cell(td: &TempDir, mock: &MockOpenAI) -> (LlmCell, DbConn) {
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "start/model", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// One params-only push, the registry's form, with `hop` as given. Returns the
/// one emission, or `None` when the cell stayed silent.
async fn push(cell: &mut LlmCell, db: &mut DbConn, hop: Value, params: Value) -> Option<Value> {
    let (sink, mut rx) = sink_at(OWN);
    let msg = MessageBuilder::new(Path::new(OWN))
        .hop(hop.as_object().cloned().unwrap_or_default())
        .body(Body::Inline(json!({"system": {}, "params": params})))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    rx.recv().await.map(|e| e.content)
}

fn header(out: &Option<Value>) -> Map<String, Value> {
    out.as_ref()
        .and_then(|c| c["header"].as_object().cloned())
        .unwrap_or_else(|| panic!("the refusal was silent: {out:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_own_push_carries_its_address_and_model() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &mock);
    let out = push(
        &mut c,
        &mut db,
        json!({"route": "in_model", "subscriber": OWN, "rank": "target", "model_id": "m2"}),
        json!({"model": "m2", "base_url": ELSEWHERE}),
    )
    .await;
    let h = header(&out);
    assert_eq!(h.get("finish_reason"), Some(&json!("error")), "{h:?}");
    assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
    assert_eq!(
        h.get("refused_subscriber"),
        Some(&json!(OWN)),
        "the refusal names the cell it came from -- its own path: {h:?}"
    );
    assert_eq!(
        h.get("refused_model"),
        Some(&json!("m2")),
        "and the model the push named: {h:?}"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a refused push costs no provider call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_reset_names_no_model() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &mock);
    let out = push(
        &mut c,
        &mut db,
        json!({"route": "in_model", "subscriber": OWN, "rank": "start", "model_id": ""}),
        json!({"$reset": ["no_such_key"]}),
    )
    .await;
    let h = header(&out);
    assert_eq!(h.get("error_code"), Some(&json!("invalid_input")), "{h:?}");
    assert_eq!(h.get("refused_subscriber"), Some(&json!(OWN)), "{h:?}");
    assert_eq!(
        h.get("refused_model"),
        Some(&json!("")),
        "a push that named no model is refused with an empty one: {h:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_name_past_the_registrys_bound_is_named_as_none() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &mock);
    let long = "m".repeat(129);
    let out = push(
        &mut c,
        &mut db,
        json!({"route": "in_model", "subscriber": OWN}),
        json!({"model": long, "base_url": ELSEWHERE}),
    )
    .await;
    let h = header(&out);
    assert_eq!(h.get("refused_subscriber"), Some(&json!(OWN)), "{h:?}");
    assert_eq!(
        h.get("refused_model"),
        Some(&json!("")),
        "no id the registry could hold, so none is carried back cut: {h:?}"
    );
}

/// Counter-check, green before the fix: the operator's push carries no address,
/// and its refusal is the error it always was -- three header keys, no more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operator_refusal_keeps_its_old_shape() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &mock);
    let out = push(
        &mut c,
        &mut db,
        json!({}),
        json!({"model": "m2", "base_url": ELSEWHERE}),
    )
    .await;
    let h = header(&out);
    let keys: Vec<&str> = h.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["error_code", "finish_reason", "latency_ms"],
        "an operator's refusal keeps its shape: {h:?}"
    );
}

/// Counter-check: a push addressed to ANOTHER cell names nobody -- whatever the
/// cell answers it (GH #862 refuses it before the slot is read), it never
/// claims to be the cell the push was for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_of_a_push_for_another_cell_names_nobody() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &mock);
    let out = push(
        &mut c,
        &mut db,
        json!({"route": "in_model", "subscriber": "/elsewhere/talky/brain"}),
        json!({"model": "m2", "base_url": ELSEWHERE}),
    )
    .await;
    let h = header(&out);
    assert!(
        !h.contains_key("refused_subscriber") && !h.contains_key("refused_model"),
        "a push for another cell is never reported as this cell's refusal: {h:?}"
    );
}

// ───────────────────────────────────────────────────────────── the tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped() -> bool {
    ["llm-registry", "talky"]
        .iter()
        .all(|t| templates_root().join(t).join("config.json").exists())
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

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
        let name = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template")
            .split('@')
            .next()
            .unwrap_or_default()
            .to_string();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": true}}, "hop": hop},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the registry and the composite.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A harness text is a registry command, as a tool_call turn; the actor rides
/// the hop so the edge can make it edge truth.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": "operator"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": raw}]}))
"#;

/// The boot-graph edge into `./store`, for the one catalogue row this file adds.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1", "text": raw}]}))
"#;

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000863";
const NEVER: &str = "0 0 0 1 1 *";
/// What the brain is BORN on -- its start value.
const START: &str = "start/model";
/// The replacement: a catalogue row on an endpoint the brain may not reach.
const M2: &str = "test/m2";
const BRAIN: &str = "/talky/brain";

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // Everything the composite says that is neither an answer nor the way
        // back is parked; the conversation's error lane separately, because
        // that is where the refusal went before GH #863.
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'answer' && hop.route != 'model_refused'"},
        {"from": "./talky/errors", "to": "/errors",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./operator", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"actor": "hop.actor"}}},
        {"from": "./llm_registry", "to": "/acks",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./llm_registry", "to": "/shows",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./llm_registry", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // THE ROAD: the push, addressed by the brain's path, onto the
        // composite's model door -- the edge `grow_level assistant` draws ...
        {"from": "./llm_registry", "to": "./talky",
         "condition": format!("has(hop.route) && hop.route == 'update' && \
                               has(hop.subscriber) && hop.subscriber == '{BRAIN}'"),
         "modifier": {"set_hop": {"route": "'in_model'"}}},
        // ... and the way back: the recipe's return edge and the shell's mirror
        // of the push edge, folded into one hop here.
        {"from": "./talky", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'model_refused'",
         "modifier": {"set_hop": {"route": "'in_refused'"}}},
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/acks",
         "condition": "context.registry_origin == 'admin'"}
    ]}}})
}

fn build_tree(td: &TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/operator/config.json",
        &code_cell(
            OPERATOR,
            &["command"],
            json!({"actor": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/admin/config.json",
        &code_cell(ADMIN, &["astore"], json!({})),
    );
    copy_cells(
        &templates_root().join("llm-registry"),
        &root.join("main/llm_registry"),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!(START);
        // No allow list: the brain's endpoint is fixed at run time, so a
        // package on another endpoint is refused.
        if let Some(p) = v["params"].as_object_mut() {
            p.remove("base_url_allow");
        }
    });
    // GH #889: the talky carries its own curator, and the curator's summarizer
    // is an `llm` cell whose model is `${ctx.model}` -- an instantiation-side
    // substitution a tree booted from disk cannot resolve. It names the mock
    // here; a run this short never reaches a rebuild, and the push below names
    // the brain, so it is never called.
    patch(root, "main/talky/curator/summarizer/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!(START);
    });
}

struct Ports {
    acks: mpsc::Receiver<Message>,
    shows: mpsc::Receiver<Message>,
    errors: mpsc::Receiver<Message>,
    _park: mpsc::Receiver<Message>,
}

async fn boot(td: &TempDir) -> (ColonyHandle, Ports) {
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
    let (k_tx, k_rx) = mpsc::channel::<Message>(64);
    let (s_tx, s_rx) = mpsc::channel::<Message>(64);
    let (e_tx, e_rx) = mpsc::channel::<Message>(64);
    let (p_tx, p_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/acks"), move || CaptureCell::new(k_tx.clone()))
        .await;
    h.spawn(Path::new("/shows"), move || CaptureCell::new(s_tx.clone()))
        .await;
    h.spawn(Path::new("/errors"), move || CaptureCell::new(e_tx.clone()))
        .await;
    h.spawn(Path::new("/park"), move || CaptureCell::new(p_tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (
        h,
        Ports {
            acks: k_rx,
            shows: s_rx,
            errors: e_rx,
            _park: p_rx,
        },
    )
}

fn text_to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(200)
        .build()
}

fn turn_json(m: &Message) -> Value {
    let Body::Inline(b) = &m.body else {
        panic!("inline expected")
    };
    meclaw_core::serde_json::from_str(b["messages"][0]["text"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null)
}

async fn recv(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what} never arrived"))
}

async fn command(h: &ColonyHandle, p: &mut Ports, cmd: Value) -> Value {
    h.send(text_to("/operator", &cmd.to_string())).await;
    loop {
        let m = recv(&mut p.acks, "the ack").await;
        if m.headers.hop.get("route").and_then(|v| v.as_str()) == Some("ack") {
            return turn_json(&m);
        }
    }
}

/// The view `show` answers for the brain.
async fn show_brain(h: &ColonyHandle, p: &mut Ports) -> Value {
    h.send(text_to(
        "/operator",
        &json!({"op": "show", "cell_path": BRAIN}).to_string(),
    ))
    .await;
    let answer = turn_json(&recv(&mut p.shows, "the show answer").await);
    answer["cells"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["cell_path"] == BRAIN))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `show` asked again until `done` holds or the deadline passes; the last view.
async fn show_until(h: &ColonyHandle, p: &mut Ports, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let view = show_brain(h, p).await;
        if done(&view) || std::time::Instant::now() > deadline {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_registry_shows_why_the_brain_refused() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
    let (h, mut p) = boot(&td).await;

    // The catalogue row: an endpoint of its own the brain was not born on.
    h.send(text_to(
        "/admin",
        &json!({"operation": "insert", "table": "models",
                "row": {"model_id": M2, "provider": "gateway", "base_url": ELSEWHERE,
                        "wire_dialect": "", "context_window": 32000, "cost_in": 1,
                        "cost_out": 1, "caps": {}, "traits": {}, "status": "active",
                        "note": "TEST ROW", "package": {}, "prompt": ""}})
        .to_string(),
    ))
    .await;
    let _ = recv(&mut p.acks, "the catalogue write").await;
    let ack = command(
        &h,
        &mut p,
        json!({"op": "subscribe", "cell_path": BRAIN, "start_model": START}),
    )
    .await;
    assert_eq!(ack["outcome"], "accepted", "{ack}");

    // The replacement is resolved and SENT -- and refused by the brain.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": BRAIN, "model_id": M2}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let id = ack["id"].as_str().unwrap_or_default().to_string();

    let sent = std::time::Instant::now();
    let view = show_until(&h, &mut p, |v| {
        v["refused"].as_str().is_some_and(|r| !r.is_empty())
    })
    .await;
    // How long one refusal took to come back and show -- the yardstick for
    // the window after the `$reset` below.
    let round_trip = sent.elapsed();
    assert_eq!(view["model_id"], M2, "show names what was sent: {view}");
    assert!(
        view["refused"]
            .as_str()
            .is_some_and(|r| r.contains("base_url")),
        "show names why the brain refused it: {view}"
    );
    assert!(
        view["refused_at"].as_str().is_some_and(|t| !t.is_empty()),
        "{view}"
    );
    // Nothing of it reached the conversation's error lane.
    let leaked = tokio::time::timeout(Duration::from_secs(2), p.errors.recv()).await;
    assert!(
        leaked.is_err(),
        "a refused push is no conversation's error: {:?}",
        leaked.ok().flatten().map(|m| m.headers.hop)
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "no push reached the provider"
    );

    // Cleared: the start value is pushed as a `$reset`, the brain takes it,
    // and the refusal of the old package no longer stands.
    let ack = command(&h, &mut p, json!({"op": "override_clear", "id": id})).await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let view = show_until(&h, &mut p, |v| v["refused"] == "" && v["model_id"] == START).await;
    assert_eq!(view["refused"], "", "{view}");
    assert_eq!(view["model_id"], START, "{view}");
    // That alone holds as soon as the registry SENDS the reset: the push
    // clears `refused` itself. The brain TOOK it only if no refusal of it
    // follows -- a refused `$reset` comes back on the same road, addressed to
    // this brain, and marks the row on its start value (review R2 M-5). So
    // the row is read again after three times the round trip the first
    // refusal needed, and never less than two seconds.
    tokio::time::sleep(std::cmp::max(round_trip * 3, Duration::from_secs(2))).await;
    let view = show_brain(&h, &mut p).await;
    assert_eq!(view["refused"], "", "the brain took the $reset: {view}");
    assert!(
        view["refused_at"].as_str().unwrap_or_default().is_empty(),
        "the brain took the $reset: {view}"
    );
    assert_eq!(view["model_id"], START, "{view}");
    h.shutdown().await;
}
