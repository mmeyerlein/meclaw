//! GH #858 — a model package the registry resolves for one of a memory hive's
//! llm cells reaches THAT cell through the hive's `in_model` door, and is
//! measured where it lands: in the cell's own params overlay in `cell.db`.
//!
//! The tree is the shipped `llm-registry` beside the shipped `memory-hive`,
//! wired the way the builder wires a member's memory (`grow_level member` in a
//! tree with a registry): the registry's `update` lane, addressed by
//! `hop.subscriber`, restamped `in_model` onto the hive's own path -- one edge
//! per cell. The memory hive runs four llm cells, so its door reads the
//! subscriber's last segment. What is claimed:
//!
//! 1. a targeted replacement for `judge` writes the package into `judge`'s
//!    overlay -- the model and its parameters -- and into no other cell;
//! 2. one for `closer` reaches `closer` through the same door, and `judge`
//!    keeps what it had;
//! 3. clearing the replacement takes the package back out: a `$reset`, so the
//!    cell is on the model it was born on again;
//! 4. no push costs a provider call: the door delivers a params-only message
//!    and the cell answers it with nothing;
//! 5. the seal still holds: an edge from outside onto `./memory/judge` is
//!    refused at the mutation door with `hive_port_boundary`.
//!
//! Free of a paid call by construction: the only provider is the in-process
//! mock, and nothing here asks it anything. R2b / GH #49: a tree without the
//! templates skips.

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
use mock_openai::MockOpenAI;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn shipped() -> bool {
    ["llm-registry", "memory-hive"]
        .iter()
        .all(|t| templates_root().join(t).join("config.json").exists())
}

/// The shipped template, copied cell by cell, refs resolved: `config.json`
/// files and the seeds beside them travel, which is what instantiation copies.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
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
            "purpose": "Test stand-in around the registry and the memory hive.",
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

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000858";
const NEVER: &str = "0 0 0 1 1 *";
/// What every memory cell is BORN on here -- its start value.
const START: &str = "start/model";
/// What the targeted replacement moves one of them to.
const M2: &str = "test/m2";
const JUDGE: &str = "/memory/judge";
const CLOSER: &str = "/memory/closer";

/// The road `grow_level member` draws for one memory cell: the registry's
/// update, addressed by the cell's path, onto the hive's model door.
fn road(cell: &str) -> Value {
    json!({"from": "./llm_registry", "to": "./memory",
           "condition": format!("has(hop.route) && hop.route == 'update' && \
                                 has(hop.subscriber) && hop.subscriber == '{cell}'"),
           "modifier": {"set_hop": {"route": "'in_model'"}}})
}

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./operator", "to": "./llm_registry",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_hop": {"route": "'in_hand'"},
                      "set_context": {"actor": "hop.actor"}}},
        {"from": "./llm_registry", "to": "/acks",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./llm_registry", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        road(JUDGE),
        road(CLOSER),
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/acks",
         "condition": "context.registry_origin == 'admin'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\nMODEL_CLOSER={START}\nMODEL_DIALECTIC={START}\n\
             MODEL_DREAMER={START}\nMODEL_JUDGE={START}\nMEMORY_LLM_BASE_URL={base_url}\n"
        ),
    )
    .unwrap();
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
    copy_cells(
        &templates_root().join("memory-hive"),
        &root.join("main/memory"),
    );
    patch(root, "main/memory/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
}

struct Ports {
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
    let (k_tx, k_rx) = mpsc::channel::<Message>(64);
    let (p_tx, p_rx) = mpsc::channel::<Message>(256);
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

/// The `cell.db` of the memory cell `name`, wherever the tree put it.
fn cell_db(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.ends_with(format!("memory/{name}")) && p.join("cell.db").is_file() {
                return Some(p.join("cell.db"));
            }
            if let Some(hit) = cell_db(&p, name) {
                return Some(hit);
            }
        }
    }
    None
}

/// The params overlay the cell persisted in its own `cell.db`: what a push
/// left there, read at the receiver.
fn overlay(root: &std::path::Path, name: &str) -> BTreeMap<String, Value> {
    let Some(db) = cell_db(root, name) else {
        return BTreeMap::new();
    };
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return BTreeMap::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT key, value FROM params") else {
        return BTreeMap::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(k, v)| {
                    let v = meclaw_core::serde_json::from_str(&v).unwrap_or(Value::String(v));
                    (k, v)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until the overlay of `name` satisfies `ok`, or fail with what it holds.
async fn settles(
    root: &std::path::Path,
    name: &str,
    what: &str,
    ok: impl Fn(&BTreeMap<String, Value>) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let now = overlay(root, name);
        if ok(&now) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: the overlay of memory/{name} is {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_cell_takes_its_package_through_the_door_and_gives_it_back() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
    let root = td.path();
    let (h, mut p) = boot(&td).await;

    // The catalogue row: no endpoint and no dialect of its own, one parameter.
    h.send(text_to(
        "/admin",
        &json!({"operation": "insert", "table": "models",
                "row": {"model_id": M2, "provider": "gateway", "base_url": "",
                        "wire_dialect": "", "context_window": 32000, "cost_in": 1,
                        "cost_out": 1, "caps": {}, "traits": {}, "status": "active",
                        "note": "TEST ROW", "package": {"max_tokens": 777},
                        "prompt": ""}})
        .to_string(),
    ))
    .await;
    let _ = recv(&mut p.acks, "the catalogue write").await;

    for cell in [JUDGE, CLOSER] {
        let ack = command(
            &h,
            &mut p,
            json!({"op": "subscribe", "cell_path": cell, "start_model": START}),
        )
        .await;
        assert_eq!(ack["outcome"], "accepted", "{cell}: {ack}");
    }

    // 1. judge, targeted: the package lands in judge's overlay and nowhere else.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": JUDGE, "model_id": M2}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let judge_id = ack["id"].as_str().unwrap_or_default().to_string();
    settles(root, "judge", "the push for judge", |o| {
        o.get("model") == Some(&json!(M2)) && o.get("max_tokens") == Some(&json!(777))
    })
    .await;
    for other in ["closer", "dialectic", "dreamer"] {
        assert!(
            !overlay(root, other).contains_key("model"),
            "a push addressed to judge moved memory/{other}: {:?}",
            overlay(root, other)
        );
    }

    // 2. closer, through the same door: closer moves, judge keeps its package.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": CLOSER, "model_id": M2}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    settles(root, "closer", "the push for closer", |o| {
        o.get("model") == Some(&json!(M2))
    })
    .await;
    assert_eq!(overlay(root, "judge").get("model"), Some(&json!(M2)));

    // 3. judge cleared: a reset, and the cell is back on what it was born on.
    let ack = command(&h, &mut p, json!({"op": "override_clear", "id": judge_id})).await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    settles(root, "judge", "the reset for judge", |o| {
        !o.contains_key("model") && !o.contains_key("max_tokens")
    })
    .await;
    assert_eq!(
        overlay(root, "closer").get("model"),
        Some(&json!(M2)),
        "clearing judge's replacement moved closer"
    );

    // 4. Nothing asked the provider anything.
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a push cost a provider call"
    );
    h.shutdown().await;
}

/// The seal is intact: the door is the ONE way in, because an edge from
/// outside onto a memory cell itself is still refused at the mutation door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_from_outside_onto_a_memory_cell_is_still_refused() {
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
                {"from": "./llm_registry", "to": "./memory/judge",
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
        "an edge onto ./memory/judge from outside must be refused by the seal: {shown}"
    );
    h.shutdown().await;
}
