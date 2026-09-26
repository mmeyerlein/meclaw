//! GH #855 — a replacement set at the registry reaches a SHIPPED brain through
//! its composite's `in_model` door, and is measured where it lands: in the
//! request body the next turn sends to the provider.
//!
//! The tree is the shipped `llm-registry` beside the shipped `talky`, wired the
//! way `meclaw-os` and the builder wire them: the registry's `update` lane,
//! addressed by `hop.subscriber`, restamped `in_model` onto the composite's own
//! path. What is claimed:
//!
//! 1. a targeted replacement onto a model with a prompt block moves the brain:
//!    the next provider call names the new model, and the block is the FIRST
//!    thing in the system part (GH #853, `model_prompt`);
//! 2. clearing it puts the brain back on its start value -- the model it was
//!    born on and no block -- by a `$reset`, not by a second package;
//! 3. neither push costs a provider call: the door delivers a params-only
//!    message, past the collector, and the brain answers it with nothing;
//! 4. the seal still holds: an edge from outside onto `./brain` is refused
//!    at the mutation door with `hive_port_boundary`, so the door is the only
//!    way in;
//! 5. both composites carry the door, and it bypasses their collector;
//! 6. the hand's package key list IS the llm cell's.
//!
//! Free of a paid call by construction: the only provider is the in-process
//! mock. R2b / GH #49: a tree without the templates skips.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped() -> bool {
    ["llm-registry", "talky", "cogny"]
        .iter()
        .all(|t| templates_root().join(t).join("config.json").exists())
}

/// The shipped template, copied cell by cell, refs resolved: `config.json`
/// files and the seeds beside them travel, which is what instantiation copies.
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

/// A harness text becomes the surface turn, on the ingress lane.
const SURFACE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn", "chat_id": "c-855"},
                             "messages": d.get("messages", [])}))
"#;

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

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000855";
const NEVER: &str = "0 0 0 1 1 *";
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;
/// What the brain is BORN on -- its start value.
const START: &str = "start/model";
/// What the targeted replacement moves it to, and the block that model needs.
const M2: &str = "test/m2";
const PROMPT: &str = "P -- the lines test/m2 needs in every system prompt.";
const BRAIN: &str = "/talky/brain";

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        {"from": "./talky/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'answer'"},
        {"from": "./talky/errors", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // the registry's hand port, the actor promoted by the edge
        {"from": "./operator", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"actor": "hop.actor"}}},
        {"from": "./llm_registry", "to": "/acks",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./llm_registry", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // THE ROAD: the registry's update, addressed by the brain's path, onto
        // the composite's model door -- the edge `grow_level assistant` draws
        {"from": "./llm_registry", "to": "./talky",
         "condition": format!("has(hop.route) && hop.route == 'update' && \
                               has(hop.subscriber) && hop.subscriber == '{BRAIN}'"),
         "modifier": {"set_hop": {"route": "'in_model'"}}},
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/acks",
         "condition": "context.registry_origin == 'admin'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/surface/config.json",
        &code_cell(
            SURFACE,
            &["turn"],
            json!({"chat_id": {"type": "string", "required": false}}),
        ),
    );
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
    });
}

struct Ports {
    answers: mpsc::Receiver<Message>,
    acks: mpsc::Receiver<Message>,
    _park: mpsc::Receiver<Message>,
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
    let (a_tx, a_rx) = mpsc::channel::<Message>(64);
    let (k_tx, k_rx) = mpsc::channel::<Message>(64);
    let (p_tx, p_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(a_tx.clone()))
        .await;
    h.spawn(Path::new("/acks"), move || CaptureCell::new(k_tx.clone()))
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
            answers: a_rx,
            acks: k_rx,
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

/// One registry command, answered by its ack.
async fn command(h: &ColonyHandle, p: &mut Ports, cmd: Value) -> Value {
    h.send(text_to("/operator", &cmd.to_string())).await;
    loop {
        let m = recv(&mut p.acks, "the ack").await;
        if m.headers.hop.get("route").and_then(|v| v.as_str()) == Some("ack") {
            return turn_json(&m);
        }
    }
}

/// One turn through the composite, answered; the provider call it cost is the
/// last recorded request.
async fn a_turn(h: &ColonyHandle, p: &mut Ports, mock: &MockOpenAI, text: &str) -> Value {
    let before = mock.recorded_requests().await.len();
    h.send(text_to("/surface", text)).await;
    let _ = recv(&mut p.answers, "the answer").await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        before + 1,
        "one provider call per turn -- and none for a push"
    );
    reqs[before].body.clone()
}

fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|m| m.first())
        .filter(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_targeted_replacement_reaches_the_brain_and_clearing_it_restores_the_start() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("one", "stop"),
        canned_chat_completion("two", "stop"),
        canned_chat_completion("three", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
    let (h, mut p) = boot(&td).await;

    // The catalogue row: no endpoint and no dialect of its own (the brain keeps
    // its, and needs no `base_url_allow`), a parameter and a prompt block.
    h.send(text_to(
        "/admin",
        &json!({"operation": "insert", "table": "models",
                "row": {"model_id": M2, "provider": "gateway", "base_url": "",
                        "wire_dialect": "", "context_window": 32000, "cost_in": 1,
                        "cost_out": 1, "caps": {}, "traits": {}, "status": "active",
                        "note": "TEST ROW", "package": {"max_tokens": 777},
                        "prompt": PROMPT}})
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

    // 0. Before anything: the start value, and no block.
    let body = a_turn(&h, &mut p, &mock, "hello").await;
    assert_eq!(body["model"], START, "{body}");
    assert!(!system_text(&body).starts_with("P --"), "{body}");

    // 1. The targeted replacement, and the next turn.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": BRAIN, "model_id": M2}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let id = ack["id"].as_str().unwrap_or_default().to_string();
    let body = a_turn(&h, &mut p, &mock, "and now?").await;
    assert_eq!(
        body["model"], M2,
        "the package's model is on the wire: {body}"
    );
    assert_eq!(body["max_tokens"], 777, "and its parameters: {body}");
    assert!(
        system_text(&body).starts_with(PROMPT),
        "the model's prompt block is the FIRST thing in the system part: {:?}",
        system_text(&body)
    );

    // 2. Cleared: back on the start value by a reset, and the block is gone.
    let ack = command(&h, &mut p, json!({"op": "override_clear", "id": id})).await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let body = a_turn(&h, &mut p, &mock, "and again?").await;
    assert_eq!(body["model"], START, "{body}");
    assert!(
        !system_text(&body).contains("P --"),
        "no remnant of the package: {:?}",
        system_text(&body)
    );
    assert_ne!(body["max_tokens"], 777, "{body}");

    h.shutdown().await;
}

/// The seal is intact: the door is the ONE way in, because an edge from
/// outside onto the brain itself is still refused at the mutation door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_from_outside_onto_the_brain_is_still_refused() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
    let (h, _p) = boot(&td).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {"add_edges": [
                {"from": "./llm_registry", "to": "./talky/brain",
                 "condition": "has(hop.route) && hop.route == 'update'"}
            ]}}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    let outcome = ack_rx.await.expect("mutation ack");
    let shown = format!("{outcome:?}");
    assert!(
        !matches!(outcome, MutationOutcome::Committed { .. })
            && shown.contains("hive_port_boundary"),
        "an edge onto ./talky/brain from outside must be refused by the seal: {shown}"
    );
    h.shutdown().await;
}

/// Both composites carry the door, it goes straight to `./brain`, and no edge
/// of theirs hands `in_model` to the collector: a params body is not a turn.
#[test]
fn both_composites_open_the_door_straight_onto_their_brain() {
    if !shipped() {
        return;
    }
    for composite in ["talky", "cogny"] {
        let cfg = read_json(&templates_root().join(composite).join("config.json"));
        let accepts = cfg["params"]["contract"]["accepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            accepts.iter().any(|l| l["route"] == "in_model"),
            "{composite} does not accept in_model"
        );
        let edges = cfg["params"]["graph"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let doors: Vec<&Value> = edges
            .iter()
            .filter(|e| {
                e["from"] == "."
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("'in_model'"))
            })
            .collect();
        assert_eq!(doors.len(), 1, "{composite}: {doors:?}");
        assert_eq!(doors[0]["to"], "./brain", "{composite}: {doors:?}");
        let brain = read_json(&templates_root().join(composite).join("brain/config.json"));
        assert!(
            brain["contract"]["consumes"]["body"]["params"].is_object(),
            "{composite}/brain does not declare the params slot it is pushed"
        );
    }
}

/// The literal the hand pushes against is the constant of the llm cell. A
/// package key the cell grows and the hand does not know would survive every
/// model change as a leftover of the previous package.
#[test]
fn the_hand_knows_the_package_keys_the_llm_cell_knows() {
    if !shipped() {
        return;
    }
    let hand = read_json(&templates_root().join("llm-registry/hand/config.json"));
    let script = hand["params"]["script_inline"].as_str().unwrap_or_default();
    let start = script
        .find("MODEL_PACKAGE_KEYS = [")
        .expect("the hand spells the package keys once");
    let end = start + script[start..].find(']').expect("a closed list");
    let literal: Vec<String> = script[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    let rust: Vec<String> = meclaw_cells::llm::params::MODEL_PACKAGE_KEYS
        .iter()
        .map(|k| k.to_string())
        .collect();
    assert_eq!(literal, rust, "hand/config.json and the llm cell disagree");
}
