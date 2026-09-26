//! GH #858 -- each llm cell states in prose what it needs, and the
//! `llm-registry` chooses against its catalogue, ONCE per change.
//!
//! What is pinned here, at the receiver wherever there is one:
//!
//! 1. **Asked once per (requirement, catalogue).** A `subscribe` with a
//!    requirement asks the translator exactly once; the answer lands as ONE row
//!    in `translations` with its reason, and the subscriber is pushed the chosen
//!    package -- proved by the model the subscriber's next inference names on
//!    the wire. A second subscriber with the same requirement asks nothing. A
//!    `model_upsert` asks once per DISTINCT requirement, not once per
//!    subscriber, and a change the translator is not shown (a package) asks
//!    nothing at all.
//! 2. **A refused or failed answer changes nothing.** A model outside the
//!    catalogue is refused and journalled, a failing translator is journalled,
//!    and in both cases every subscriber keeps what it resolved to;
//!    `retranslate` asks again.
//! 3. **The precedence** targeted > global > prose > tier > start value, each step
//!    visible in `show` -- with the translator's sentence as `because` while the
//!    rank is `prose` -- and a retired model is never a result.
//! 4. **`requirement` is immutable** in the llm cell: a params-only message
//!    that names it is `invalid_input`.
//! 5. **Once also while a question is open**: two subscriptions with the same
//!    new requirement in flight together ask once, and the answer settles both.
//! 6. **No flutter**: a new or changed requirement without a translation keeps
//!    the resolution the cell holds (else its tier, else its start value) --
//!    no push until the answer, then exactly one.
//! 7. **The fallback** is the newest translation whose model is still active;
//!    a refused answer reaches the journal on one short line.
//! 8. **`model_upsert` refuses** oversized prompt blocks, strengths and ids.
//!
//! Free of a paid call by construction: the translator and the subscribers
//! each talk to an in-process mock (`mock_openai.rs`), and the translator's
//! config is pointed at its mock after the shipped template is copied.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_error_status};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The template root, or `None` where it does not ship (R2b form, GH #49).
fn shipped_registry() -> Option<std::path::PathBuf> {
    let root = templates_root().join("llm-registry");
    for rel in [
        "config.json",
        "store/config.json",
        "hand/config.json",
        "select/config.json",
        "translate/config.json",
    ] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json" {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

// ────────────────────────────────────────────────────────── the test-only cells

/// The commanding side of the hand port; the actor rides the hop so the port
/// edge can make it edge truth.
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": "member:alex"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": raw}]}))
"#;

/// The maintenance cell at the far end of the boot-graph edge into `./store`.
const ADMIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "astore"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "a1", "text": raw}]}))
"#;

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
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped llm-registry template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A subscriber: a REAL `llm` cell on the subscribers' mock, born on its own
/// model and, like a shipped cell since 0.46.0, stating what it needs.
fn llm_cell(base_url: &str, model: &str, requirement: &str) -> Value {
    json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai", "model": model,
            "api_key": "test-key-858", "base_url": base_url,
            "requirement": requirement
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true},
                         "meta": {"type": "object", "required": false}},
                "hop": {"finish_reason": {"type": "string", "required": true},
                        "error_code": {"type": "string", "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false},
                                  "system": {"type": "object", "required": false}}},
            "capabilities": ["network:llm", "db:own"]
        },
        "description": {
            "purpose": "A subscribed brain that states its requirement.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./operator", "to": "./llm_registry/hand",
         "condition": "has(hop.route) && hop.route == 'command'",
         "modifier": {"set_context": {"actor": "hop.actor"}}},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ack'"},
        {"from": "./llm_registry/hand", "to": "./sub_a",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_a'"},
        {"from": "./llm_registry/hand", "to": "./sub_b",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_b'"},
        {"from": "./llm_registry/hand", "to": "./sub_c",
         "condition": "has(hop.route) && hop.route == 'update' && hop.subscriber == '/sub_c'"},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'update'"},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./llm_registry/hand", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./llm_registry/select", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"},
        {"from": "./admin", "to": "./llm_registry/store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"registry_origin": "'admin'"}}},
        {"from": "./llm_registry/store", "to": "/sink",
         "condition": "context.registry_origin == 'admin'"},
        {"from": "./sub_a", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_b", "to": "/sink", "condition": "has(hop.finish_reason)"},
        {"from": "./sub_c", "to": "/sink", "condition": "has(hop.finish_reason)"}
    ]}}})
}

const BIRTH_A: &str = "birth/sub-a";
const BIRTH_B: &str = "birth/sub-b";
const BIRTH_C: &str = "birth/sub-c";

/// Two requirements, in the form a template writes them: what the cell needs,
/// never a model name.
const R_TALK: &str = "Talks with one person in real time; short answers, low latency, tools, a \
                      modest price.";
const R_JUDGE: &str = "Judges a proposed change to the colony; careful reasoning over speed and \
                       price, long context.";

/// The catalogue the tests resolve against: no endpoint and no dialect of its
/// own on any row, so a subscriber on the mock can take every package.
const FAST: &str = "test/fast";
const DEEP: &str = "test/deep";

fn build_tree(
    td: &tempfile::TempDir,
    root_template: &std::path::Path,
    subscribers_url: &str,
    translator_url: &str,
) {
    let root = td.path();
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
    write(
        root,
        "main/sub_a/config.json",
        &llm_cell(subscribers_url, BIRTH_A, R_TALK),
    );
    write(
        root,
        "main/sub_b/config.json",
        &llm_cell(subscribers_url, BIRTH_B, R_TALK),
    );
    write(
        root,
        "main/sub_c/config.json",
        &llm_cell(subscribers_url, BIRTH_C, R_JUDGE),
    );
    // The template's cells WITHOUT its seeds: the tests write the catalogue
    // they need, so a price moving in the shipped seed moves no assertion.
    copy_cells(root_template, &root.join("main/llm_registry"));
    // The translator, pointed at its own mock. Everything else in its config
    // is the shipped one.
    let tp = root.join("main/llm_registry/translate/config.json");
    let mut translate: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&tp).unwrap()).unwrap();
    translate["params"]["base_url"] = json!(translator_url);
    translate["params"]["model"] = json!("test/translator");
    translate["params"]["api_key"] = json!("test-key-translator");
    std::fs::write(
        &tp,
        meclaw_core::serde_json::to_string_pretty(&translate).unwrap(),
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
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
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

fn to(cell: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

fn text_to(cell: &str, text: &str) -> Message {
    to(
        cell,
        json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
    )
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn turn_json(m: &Message) -> Value {
    let text = body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    meclaw_core::serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn recv_matching(
    rx: &mut mpsc::Receiver<Message>,
    label: &str,
    want: impl Fn(&Message) -> bool,
) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..40 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                panic!("nothing more arrived while waiting for {label}; saw {seen:?}")
            });
        if want(&m) {
            return m;
        }
        seen.push(format!("{:?}", m.headers.hop));
    }
    panic!("{label} never arrived; saw {seen:?}");
}

/// One store op over the boot-graph edge into `./store`, as its rows.
async fn admin(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op: Value) -> Value {
    h.send(text_to("/admin", &op.to_string())).await;
    let m = recv_matching(rx, "admin answer", |m| !hop_of(m, "operation").is_empty()).await;
    turn_json(&m)
}

/// A catalogue row over the boot-graph edge: no endpoint, no dialect.
async fn catalogue(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    model_id: &str,
    cost: i64,
    package: Value,
) {
    admin(
        h,
        rx,
        json!({"operation": "insert", "table": "models",
               "row": {"model_id": model_id, "provider": "gateway", "base_url": "",
                       "wire_dialect": "", "context_window": 32000, "cost_in": cost,
                       "cost_out": cost, "caps": {"tools": true}, "traits": {},
                       "strengths": format!("test row {model_id}"), "status": "active",
                       "note": "TEST ROW", "package": package, "prompt": ""}}),
    )
    .await;
}

/// One command through the hand port: every `update` the sink saw before the
/// acknowledgement, and the ack payload.
async fn command(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    cmd: Value,
) -> (Vec<Message>, Value) {
    h.send(text_to("/operator", &cmd.to_string())).await;
    let mut pushes = Vec::new();
    for _ in 0..40 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("no ack for {cmd}; pushes so far {}", pushes.len()));
        match hop_of(&m, "route").as_str() {
            "update" => pushes.push(m),
            "ack" => return (pushes, turn_json(&m)),
            _ => {}
        }
    }
    panic!("no ack for {cmd}");
}

/// The next push, which a translation sends AFTER the ack of the op that asked.
async fn next_push(rx: &mut mpsc::Receiver<Message>) -> Message {
    recv_matching(rx, "a push", |m| hop_of(m, "route") == "update").await
}

/// The `show` view, keyed by path, plus the whole answer.
async fn show(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
) -> (std::collections::BTreeMap<String, Value>, Value) {
    h.send(text_to("/operator", r#"{"op":"show"}"#)).await;
    let m = recv_matching(rx, "show", |m| hop_of(m, "route") == "answer").await;
    let v = turn_json(&m);
    let cells = v["cells"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c["cell_path"].as_str().unwrap_or_default().to_string(), c))
        .collect();
    (cells, v)
}

/// The journal lines with this reason, polled until `want` of them are there.
async fn journal_until(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    reason: &str,
    want: usize,
) -> Vec<Value> {
    let mut got = Vec::new();
    for _ in 0..100 {
        let rows = admin(
            h,
            rx,
            json!({"operation": "select", "table": "resolutions",
                   "columns": ["reason", "model_id", "source_id"],
                   "where": {"reason": reason}, "limit": 100}),
        )
        .await;
        got = rows.as_array().cloned().unwrap_or_default();
        if got.len() >= want {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the journal never carried {want} line(s) `{reason}`: {got:?}");
}

async fn translations(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>) -> Vec<Value> {
    admin(
        h,
        rx,
        json!({"operation": "select", "table": "translations",
               "columns": ["requirement_hash", "catalogue_hash", "model_id", "reason"],
               "limit": 100}),
    )
    .await
    .as_array()
    .cloned()
    .unwrap_or_default()
}

/// One inference on a subscriber, answered as the model the wire names.
async fn inference_model(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    cell: &str,
    mock: &MockOpenAI,
) -> String {
    let before = mock.recorded_requests().await.len();
    h.send(text_to(cell, "ping")).await;
    let _ = recv_matching(rx, "an inference answer", |m| {
        !hop_of(m, "finish_reason").is_empty()
    })
    .await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), before + 1, "one provider call per inference");
    snaps[before].model().unwrap_or_default().to_string()
}

fn answer(model_id: &str, reason: &str) -> meclaw_testing::mock_http::MockResponse {
    canned_chat_completion(
        &json!({"model_id": model_id, "reason": reason}).to_string(),
        "stop",
    )
}

fn subscribers_mock_answers() -> Vec<meclaw_testing::mock_http::MockResponse> {
    (0..16)
        .map(|i| canned_chat_completion(&format!("answer {i}"), "stop"))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// 1. Asked once per (requirement, catalogue), stored with its reason, pushed
///    at once -- and asked again only when the catalogue changed in what the
///    translator is shown, once per distinct requirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_requirement_is_translated_once_and_the_answer_is_a_row() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let translator = MockOpenAI::start(vec![
        answer(
            DEEP,
            "it talks, and the deep row is what the test wants first",
        ),
        answer(FAST, "a judge on a budget"),
        answer("test/new", "the new row fits"),
        answer("test/new", "the new row fits"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    catalogue(&h, &mut rx, FAST, 1, json!({})).await;
    catalogue(&h, &mut rx, DEEP, 50, json!({"max_tokens": 777})).await;

    // (a) The first subscriber with a requirement: the ack comes first and
    //     says one question went out; the push follows the answer.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_TALK}),
    )
    .await;
    assert!(pushes.is_empty(), "nothing is known before the answer");
    assert_eq!(ack["translations_asked"].as_i64(), Some(1), "{ack}");
    let push = next_push(&mut rx).await;
    assert_eq!(hop_of(&push, "subscriber"), "/sub_a");
    assert_eq!(hop_of(&push, "rank"), "prose");
    assert_eq!(body_of(&push)["params"]["model"], DEEP);
    assert_eq!(body_of(&push)["params"]["max_tokens"], 777);
    assert_eq!(translator.recorded_requests().await.len(), 1);
    // The question carries the requirement and the catalogue, and the system
    // part tells the translator what to answer.
    let q = &translator.recorded_requests().await[0];
    let asked = meclaw_core::serde_json::to_string(q.messages().unwrap()).unwrap();
    assert!(asked.contains("short answers, low latency"), "{asked}");
    assert!(asked.contains(DEEP) && asked.contains(FAST), "{asked}");
    assert!(asked.contains("Choose ONLY an id"), "{asked}");
    // At the receiver: the subscriber's next call names the chosen model.
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, DEEP);
    let rows = translations(&h, &mut rx).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["model_id"], DEEP);
    assert!(
        rows[0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("deep row")),
        "{rows:?}"
    );

    // (b) The same requirement again, for another cell: no question, and the
    //     push comes with the ack.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_b", "start_model": BIRTH_B,
               "requirement": R_TALK}),
    )
    .await;
    assert_eq!(ack["translations_asked"].as_i64(), Some(0), "{ack}");
    assert_eq!(pushes.len(), 1, "{ack}");
    assert_eq!(hop_of(&pushes[0], "subscriber"), "/sub_b");
    assert_eq!(body_of(&pushes[0])["params"]["model"], DEEP);
    assert_eq!(inference_model(&h, &mut rx, "/sub_b", &subs).await, DEEP);
    assert_eq!(
        translator.recorded_requests().await.len(),
        1,
        "a requirement already answered for this catalogue asks nothing"
    );

    // (c) A second requirement: one more question.
    let (_, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_c", "start_model": BIRTH_C,
               "requirement": R_JUDGE}),
    )
    .await;
    assert_eq!(ack["translations_asked"].as_i64(), Some(1), "{ack}");
    let push = next_push(&mut rx).await;
    assert_eq!(hop_of(&push, "subscriber"), "/sub_c");
    assert_eq!(body_of(&push)["params"]["model"], FAST);
    assert_eq!(translator.recorded_requests().await.len(), 2);

    // (d) A new row the translator is shown: three subscribers, TWO distinct
    //     requirements, two questions -- and every subscriber moves.
    let (_, ack) = command(
        &h,
        &mut rx,
        json!({"op": "model_upsert", "model": {"model_id": "test/new", "cost_in": 2,
               "cost_out": 2, "strengths": "new and fits everything"}}),
    )
    .await;
    assert_eq!(ack["reason_code"], "model_upserted", "{ack}");
    assert_eq!(ack["translations_asked"].as_i64(), Some(2), "{ack}");
    let mut moved = Vec::new();
    for _ in 0..3 {
        let p = next_push(&mut rx).await;
        assert_eq!(body_of(&p)["params"]["model"], "test/new");
        moved.push(hop_of(&p, "subscriber"));
    }
    moved.sort();
    assert_eq!(moved, vec!["/sub_a", "/sub_b", "/sub_c"]);
    assert_eq!(translator.recorded_requests().await.len(), 4);
    assert_eq!(translations(&h, &mut rx).await.len(), 4);

    // (e) A change the translator is NOT shown -- the package of a row -- is no
    //     new question; the package still reaches the cells that run it.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "model_upsert", "model": {"model_id": "test/new",
               "package": {"max_tokens": 900}}}),
    )
    .await;
    assert_eq!(ack["translations_asked"].as_i64(), Some(0), "{ack}");
    assert_eq!(pushes.len(), 3, "{ack}");
    assert!(
        pushes
            .iter()
            .all(|p| body_of(p)["params"]["max_tokens"] == 900)
    );
    assert_eq!(translator.recorded_requests().await.len(), 4);

    h.shutdown().await;
}

/// 2. The translator's answer is a candidate: outside the catalogue it is
///    refused, a failed call is journalled, and either way the subscriber keeps
///    what it resolved to. `retranslate` asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_or_failed_answer_leaves_the_last_resolution() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let translator = MockOpenAI::start(vec![
        answer(DEEP, "first answer"),
        answer("vendor/not-in-the-catalogue", "a model nobody listed"),
        canned_error_status(500),
        answer("test/third", "asked again"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    catalogue(&h, &mut rx, FAST, 1, json!({})).await;
    catalogue(&h, &mut rx, DEEP, 50, json!({})).await;

    command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_TALK}),
    )
    .await;
    let push = next_push(&mut rx).await;
    assert_eq!(body_of(&push)["params"]["model"], DEEP);

    // Outside the catalogue: refused, journalled, nothing stored, nothing moves.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "model_upsert", "model": {"model_id": "test/other", "cost_in": 3}}),
    )
    .await;
    assert!(pushes.is_empty(), "the last translation still holds");
    let lines = journal_until(&h, &mut rx, "translation_outside_catalogue", 1).await;
    assert_eq!(lines[0]["model_id"], "vendor/not-in-the-catalogue");
    assert_eq!(translations(&h, &mut rx).await.len(), 1);
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["model_id"], DEEP, "{view:?}");
    assert_eq!(view["/sub_a"]["rank"], "prose");
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, DEEP);

    // A failing translator: journalled, nothing moves.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "model_upsert", "model": {"model_id": "test/third", "cost_in": 4}}),
    )
    .await;
    assert!(pushes.is_empty());
    journal_until(&h, &mut rx, "translation_failed", 1).await;
    assert_eq!(translations(&h, &mut rx).await.len(), 1);
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["model_id"], DEEP, "{view:?}");

    // `retranslate` asks what the current catalogue has not answered -- once.
    let (_, ack) = command(&h, &mut rx, json!({"op": "retranslate"})).await;
    assert_eq!(ack["reason_code"], "retranslated", "{ack}");
    assert_eq!(ack["translations_asked"].as_i64(), Some(1), "{ack}");
    let push = next_push(&mut rx).await;
    assert_eq!(body_of(&push)["params"]["model"], "test/third");
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &subs).await,
        "test/third"
    );
    assert_eq!(translator.recorded_requests().await.len(), 4);

    h.shutdown().await;
}

/// 3. targeted > global > prose > start value, each step visible in `show`,
///    and a retired model is never a result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_precedence_is_target_global_prose_start_and_show_says_why() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    // The first answer, then a failure for the question the retirement asks:
    // with no active translation left the cell falls to its start value.
    let translator = MockOpenAI::start(vec![
        answer(DEEP, "the talking cell wants the deep row"),
        canned_error_status(500),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    for (m, cost) in [(FAST, 1), (DEEP, 50), ("test/g", 5), ("test/t", 6)] {
        catalogue(&h, &mut rx, m, cost, json!({})).await;
    }
    // A tier the subscriber is ALSO on: prose replaces it.
    admin(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "tiers",
               "row": {"tier": "mid", "model_id": FAST, "since": "2026-09-26T00:00:00Z",
                       "decided_by": "test", "active": 1}}),
    )
    .await;
    command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_TALK, "tier": "mid"}),
    )
    .await;
    let push = next_push(&mut rx).await;
    assert_eq!(hop_of(&push, "rank"), "prose");

    let (view, whole) = show(&h, &mut rx).await;
    assert_eq!(
        whole["precedence"],
        json!(["target", "global", "prose", "tier", "start"])
    );
    let a = &view["/sub_a"];
    assert_eq!(a["rank"], "prose", "{a}");
    assert_eq!(a["model_id"], DEEP, "{a}");
    assert_eq!(a["reason"], "prose_translated", "{a}");
    assert_eq!(a["because"], "the talking cell wants the deep row", "{a}");
    assert_eq!(a["requirement"], R_TALK, "{a}");

    // global: "the deep row everywhere -> test/g" replaces the translation.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "global", "match": DEEP, "model_id": "test/g"}),
    )
    .await;
    let global_id = ack["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(pushes.len(), 1, "{ack}");
    assert_eq!(hop_of(&pushes[0], "rank"), "global");
    // targeted beats global.
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "override_set", "scope": "target", "match": "/sub_a", "model_id": "test/t"}),
    )
    .await;
    let target_id = ack["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(hop_of(&pushes[0], "rank"), "target");
    assert_eq!(
        inference_model(&h, &mut rx, "/sub_a", &subs).await,
        "test/t"
    );
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["rank"], "target");
    assert_eq!(
        view["/sub_a"]["because"], "",
        "only the prose rank has a sentence"
    );

    // Clearing them walks back down: global, then prose.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_clear", "id": target_id}),
    )
    .await;
    assert_eq!(hop_of(&pushes[0], "rank"), "global");
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "override_clear", "id": global_id}),
    )
    .await;
    assert_eq!(hop_of(&pushes[0], "rank"), "prose");
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, DEEP);

    // The translated model retires and the new question fails: no active
    // translation and no held model left, so the tier the cell is on carries
    // it (review of the strand, I-2) -- never an empty start value while an
    // operator statement stands.
    let (pushes, ack) = command(&h, &mut rx, json!({"op": "model_retire", "model_id": DEEP})).await;
    assert_eq!(ack["reason_code"], "model_retired", "{ack}");
    assert_eq!(pushes.len(), 1, "{ack}");
    assert_eq!(hop_of(&pushes[0], "rank"), "tier");
    assert_eq!(body_of(&pushes[0])["params"]["model"], FAST);
    journal_until(&h, &mut rx, "translation_failed", 1).await;
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["rank"], "tier", "{view:?}");
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, FAST);

    h.shutdown().await;
}

/// 4. `requirement` is the template's statement, not a knob: a params-only
///    message that names it is refused, and the cell keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requirement_is_immutable_at_run_time() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let translator = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    h.send(to(
        "/sub_a",
        json!({"system": {}, "params": {"requirement": "Something else entirely."}}),
    ))
    .await;
    let refused = recv_matching(&mut rx, "the refusal", |m| {
        !hop_of(m, "finish_reason").is_empty()
    })
    .await;
    assert_eq!(hop_of(&refused, "finish_reason"), "error");
    assert_eq!(hop_of(&refused, "error_code"), "invalid_input");
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, BIRTH_A);
    assert!(
        translator.recorded_requests().await.is_empty(),
        "nothing here asks the translator"
    );

    h.shutdown().await;
}

/// A second requirement for the tests that change what a cell states.
const R_NEW: &str = "Summarises a long thread once a day; price over speed, a large context.";

/// The key half a requirement contributes, as the hand computes it: sha256 of
/// the trimmed prose, 16 hex chars.
fn requirement_key(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex: String = Sha256::digest(text.trim().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hex[..16].to_string()
}

/// 5. Two subscriptions with the same NEW requirement in flight together ask
///    the translator ONCE (review of the strand, I-1): the question is written
///    down as open in the bundle that asks it, a second op that finds it open
///    asks nothing, and the one answer settles both subscribers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_subscriptions_in_flight_together_ask_once() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    // The first answer is held back, so the second subscription is read,
    // settled and acknowledged while the first question is still open.
    let translator = MockOpenAI::start(vec![
        answer(DEEP, "one question, one answer").with_delay(Duration::from_secs(3)),
        answer(FAST, "a second question nobody should have asked"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    catalogue(&h, &mut rx, FAST, 1, json!({})).await;
    catalogue(&h, &mut rx, DEEP, 50, json!({})).await;

    for (cell, birth) in [("/sub_a", BIRTH_A), ("/sub_b", BIRTH_B)] {
        let cmd = json!({"op": "subscribe", "cell_path": cell, "start_model": birth,
                         "requirement": R_TALK});
        h.send(text_to("/operator", &cmd.to_string())).await;
    }
    let mut acks = 0;
    let mut moved = std::collections::BTreeMap::new();
    for _ in 0..40 {
        if acks == 2 && moved.len() == 2 {
            break;
        }
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("stalled: {acks} ack(s), pushes {moved:?}"));
        match hop_of(&m, "route").as_str() {
            "ack" => acks += 1,
            "update" => {
                moved.insert(
                    hop_of(&m, "subscriber"),
                    body_of(&m)["params"]["model"].clone(),
                );
            }
            _ => {}
        }
    }
    assert_eq!(acks, 2);
    assert_eq!(
        moved,
        [
            ("/sub_a".to_string(), json!(DEEP)),
            ("/sub_b".to_string(), json!(DEEP))
        ]
        .into_iter()
        .collect(),
        "the one answer settles both subscribers"
    );
    // A second question would queue behind the first in the translate cell and
    // leave the moment the first answer is back; this is long enough for it.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        translator.recorded_requests().await.len(),
        1,
        "one question per (requirement, catalogue), also while it is open"
    );
    assert_eq!(translations(&h, &mut rx).await.len(), 1);
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, DEEP);
    assert_eq!(inference_model(&h, &mut rx, "/sub_b", &subs).await, DEEP);
    // The open question went with its answer.
    let open = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "open_questions",
               "columns": ["requirement_hash"], "limit": 10}),
    )
    .await;
    assert_eq!(open, json!([]), "{open}");

    h.shutdown().await;
}

/// The shipped hand, run once over one message the way a `code` cell runs it:
/// the script goes to python3 on stdin (GH #279), the document is the one
/// `meclaw_testing::code_stdin` builds. Returns the emissions.
fn run_hand(root: &std::path::Path, flat: Value) -> Vec<Value> {
    use std::io::Write;
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("hand/config.json")).unwrap(),
    )
    .unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap_or_default();
    let stdin_doc = meclaw_testing::code_stdin(&flat).to_string();
    let src = format!(
        "import sys, io\n_script = {}\nsys.stdin = io.StringIO({})\n\
         exec(compile(_script, 'cell', 'exec'), globals())\n",
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(src.as_bytes())
        .expect("write program");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "hand exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice::<Value>(&out.stdout).expect("hand output is JSON") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

/// The store's echo of a claim bundle, as the hand reads it: per claim three
/// results (expired removed, the open question found before the claim, the
/// claim), then the catalogue as it stands after the bundle.
fn claimed_echo(claims: &[Value], before: &[Value], catalogue: &[Value]) -> Value {
    let turn = |id: String, rows: &Value| json!({"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()});
    let mut messages = Vec::new();
    for (i, found) in before.iter().enumerate() {
        messages.push(turn(format!("expired-{i}"), &json!([])));
        messages.push(turn(format!("before-{i}"), found));
        messages.push(turn(format!("claim-{i}"), &json!([])));
    }
    messages.push(turn("catalogue".to_string(), &json!(catalogue)));
    json!({
        "header": {
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0},
            "context": {"lr_phase": "claimed",
                        "lr_carry": json!({"n": 0, "claims": claims}).to_string()},
        },
        "messages": messages,
    })
}

/// 5b. Review rev2-L2b1, m-A: a `subscribe` of a NEW cell races a
///    `model_upsert`. The upsert read its page before the cell's row existed
///    (the subscribe's own claim bundle writes it), its bundle ran first, and
///    the subscribe's echo finds the catalogue moved. Dropping the question
///    there left the requirement unasked until the next trigger -- nobody
///    else had seen the subscriber. The echo already holds the catalogue as it
///    stands, so the question is asked against THAT one, and the claim moves
///    with it. A question someone else holds open is still not asked, and a
///    catalogue that is empty now asks nothing -- and in both cases the claim
///    is closed, so it does not block its pair until it expires.
///
///    The race itself is not reproducible on purpose in a colony (two bundles
///    in a fixed order around one store cell); the echo is its outcome, so the
///    lock drives the shipped hand with exactly that echo.
#[test]
fn a_claim_whose_catalogue_moved_is_asked_against_the_catalogue_now() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let row = json!({"model_id": FAST, "provider": "vendor", "context_window": 1000,
                     "cost_in": 1, "cost_out": 2, "caps": {}, "traits": {},
                     "strengths": "quick to answer", "status": "active"});
    let claim = json!({"rh": "r1", "ch": "the-catalogue-before", "requirement": R_TALK,
                       "cid": "claim-1"});

    let out = run_hand(
        &root,
        claimed_echo(
            std::slice::from_ref(&claim),
            &[json!([])],
            std::slice::from_ref(&row),
        ),
    );
    let asks: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "translate")
        .collect();
    assert_eq!(asks.len(), 1, "the moved question is asked: {out:?}");
    let carry: Value =
        meclaw_core::serde_json::from_str(asks[0]["header"]["carry"].as_str().unwrap_or("{}"))
            .unwrap();
    let ch_now = carry["ch"].as_str().unwrap_or_default().to_string();
    assert!(
        !ch_now.is_empty() && ch_now != "the-catalogue-before",
        "against the catalogue as it stands: {carry}"
    );
    assert_eq!(carry["cid"], "claim-1", "{carry}");
    assert_eq!(carry["rh"], "r1", "{carry}");
    let question = asks[0]["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(question.contains(R_TALK), "{question}");
    assert!(
        question.contains(FAST),
        "the catalogue now is shown: {question}"
    );
    // The claim moves with the question, so a third op on the new pair finds it
    // open -- and it is not closed.
    let store_ops = |out: &[Value]| -> Vec<Value> {
        out.iter()
            .filter(|m| m["header"]["route"] == "rstore")
            .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
            .map(|t| {
                meclaw_core::serde_json::from_str(t["text"].as_str().unwrap_or("null")).unwrap()
            })
            .collect()
    };
    // Review rev-F2, m2: a claim left standing where nothing is asked blocks
    // its pair for the 120 s of `OPEN_QUESTION_SECONDS` with no question in
    // flight, so the two not-asked cases below lock the close positively.
    let closes_claim_1 = |ops: &[Value]| {
        ops.iter().any(|op| {
            op["operation"] == "delete"
                && op["table"] == "open_questions"
                && op["where"]["claim"] == "claim-1"
        })
    };
    let ops = store_ops(&out);
    assert!(
        ops.iter().any(|op| op["operation"] == "update"
            && op["table"] == "open_questions"
            && op["where"]["claim"] == "claim-1"
            && op["set"]["catalogue_hash"] == json!(ch_now)),
        "the claim moves to the catalogue it is asked against: {ops:?}"
    );
    assert!(!closes_claim_1(&ops), "an asked claim stays open: {ops:?}");

    // Held open by another op: not asked here, and this claim is closed.
    let out = run_hand(
        &root,
        claimed_echo(
            std::slice::from_ref(&claim),
            &[json!([{"claim": "someone-else"}])],
            std::slice::from_ref(&row),
        ),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "translate"),
        "{out:?}"
    );
    let ops = store_ops(&out);
    assert!(
        closes_claim_1(&ops),
        "held elsewhere: this claim is closed: {ops:?}"
    );

    // Moved to an empty catalogue: nothing to ask against.
    let out = run_hand(
        &root,
        claimed_echo(std::slice::from_ref(&claim), &[json!([])], &[]),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "translate"),
        "{out:?}"
    );
    let ops = store_ops(&out);
    assert!(
        closes_claim_1(&ops),
        "empty catalogue: this claim is closed: {ops:?}"
    );
}

/// 6. While a new or changed requirement has no translation, the subscriber
///    keeps the resolution it holds (review of the strand, I-2): no `$reset`,
///    no flutter through the start value, and a failing translator leaves the
///    cell where it was -- after prose as after a tier. With a healthy
///    translator the change is exactly ONE push.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_requirement_keeps_the_resolution_until_it_is_answered() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let translator = MockOpenAI::start(vec![
        answer(DEEP, "it talks"),
        canned_error_status(500),
        canned_error_status(500),
        answer(FAST, "a new\nneed\u{7}  on one line"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    catalogue(&h, &mut rx, FAST, 1, json!({})).await;
    catalogue(&h, &mut rx, DEEP, 50, json!({})).await;
    admin(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "tiers",
               "row": {"tier": "mid", "model_id": FAST, "since": "2026-09-26T00:00:00Z",
                       "decided_by": "test", "active": 1}}),
    )
    .await;

    // Prose -> changed prose, translator down: nothing moves.
    command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_TALK}),
    )
    .await;
    assert_eq!(body_of(&next_push(&mut rx).await)["params"]["model"], DEEP);
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_JUDGE}),
    )
    .await;
    assert!(pushes.is_empty(), "no push before an answer: {ack}");
    assert_eq!(ack["translations_asked"].as_i64(), Some(1), "{ack}");
    journal_until(&h, &mut rx, "translation_failed", 1).await;
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_a"]["model_id"], DEEP, "{view:?}");
    assert_eq!(view["/sub_a"]["rank"], "prose", "{view:?}");
    assert_eq!(view["/sub_a"]["requirement"], R_JUDGE, "{view:?}");
    // Review rev2-L2b1, m-D: the sentence was the answer to the OLD
    // requirement; beside the new one it would read as the answer to it.
    assert_eq!(
        view["/sub_a"]["because"], "held until the new requirement is answered",
        "{view:?}"
    );
    assert_eq!(inference_model(&h, &mut rx, "/sub_a", &subs).await, DEEP);

    // Tier -> prose, translator down: the tier carries on.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_c", "start_model": BIRTH_C,
               "tier": "mid"}),
    )
    .await;
    assert_eq!(hop_of(&pushes[0], "rank"), "tier");
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_c", "start_model": BIRTH_C,
               "requirement": R_JUDGE}),
    )
    .await;
    assert!(pushes.is_empty(), "no push before an answer: {ack}");
    journal_until(&h, &mut rx, "translation_failed", 2).await;
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_c"]["model_id"], FAST, "{view:?}");
    assert_eq!(view["/sub_c"]["rank"], "tier", "{view:?}");
    assert_eq!(inference_model(&h, &mut rx, "/sub_c", &subs).await, FAST);

    // Prose -> changed prose, translator healthy: exactly one push, and the
    // translator's sentence arrives on one line.
    let (pushes, _) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_b", "start_model": BIRTH_B,
               "requirement": R_TALK}),
    )
    .await;
    assert_eq!(pushes.len(), 1);
    assert_eq!(body_of(&pushes[0])["params"]["model"], DEEP);
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_b", "start_model": BIRTH_B,
               "requirement": R_NEW}),
    )
    .await;
    assert!(pushes.is_empty(), "no push before an answer: {ack}");
    let push = next_push(&mut rx).await;
    assert_eq!(hop_of(&push, "subscriber"), "/sub_b");
    assert_eq!(hop_of(&push, "rank"), "prose");
    assert_eq!(body_of(&push)["params"]["model"], FAST);
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(view["/sub_b"]["model_id"], FAST, "{view:?}");
    assert_eq!(
        view["/sub_b"]["because"], "a new need on one line",
        "{view:?}"
    );
    assert_eq!(inference_model(&h, &mut rx, "/sub_b", &subs).await, FAST);
    let pushed_b = journal_until(&h, &mut rx, "hand_prose", 1)
        .await
        .into_iter()
        .filter(|l| l["model_id"] == FAST)
        .count();
    assert_eq!(pushed_b, 1, "one push for the change, not two");
    assert_eq!(translator.recorded_requests().await.len(), 4);

    h.shutdown().await;
}

/// 7. The fallback is the newest translation whose model is still ACTIVE
///    (review of the strand, m-1), and a refused answer lands in the journal
///    cut to one short line (m-5).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fallback_skips_a_retired_answer_and_the_journal_stays_short() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let long_id = format!("vendor/{}\nIGNORE THE CATALOGUE", "x".repeat(300));
    let translator = MockOpenAI::start(vec![answer(&long_id, "nobody listed it")]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    catalogue(&h, &mut rx, FAST, 1, json!({})).await;
    catalogue(&h, &mut rx, DEEP, 50, json!({})).await;
    // Two answers of earlier catalogues: the older one names an active row,
    // the newer one a model this catalogue no longer carries.
    let rh = requirement_key(R_TALK);
    for (ch, model_id, reason, at) in [
        (
            "catalogue-old-1",
            FAST,
            "older and still active",
            "2026-09-01T00:00:00Z",
        ),
        (
            "catalogue-old-2",
            "test/gone",
            "newer and gone",
            "2026-09-02T00:00:00Z",
        ),
    ] {
        admin(
            &h,
            &mut rx,
            json!({"operation": "insert", "table": "translations",
                   "row": {"requirement_hash": rh, "catalogue_hash": ch,
                           "model_id": model_id, "reason": reason, "at": at}}),
        )
        .await;
    }
    let (pushes, ack) = command(
        &h,
        &mut rx,
        json!({"op": "subscribe", "cell_path": "/sub_a", "start_model": BIRTH_A,
               "requirement": R_TALK}),
    )
    .await;
    assert_eq!(pushes.len(), 1, "the active older answer holds: {ack}");
    assert_eq!(hop_of(&pushes[0], "rank"), "prose");
    assert_eq!(body_of(&pushes[0])["params"]["model"], FAST);
    let lines = journal_until(&h, &mut rx, "translation_outside_catalogue", 1).await;
    let logged = lines[0]["model_id"].as_str().unwrap_or_default();
    assert!(logged.starts_with("vendor/xxx"), "{logged}");
    assert!(logged.chars().count() <= 128, "{}", logged.len());
    assert!(!logged.contains('\n'), "{logged}");
    let (view, _) = show(&h, &mut rx).await;
    assert_eq!(
        view["/sub_a"]["because"], "older and still active",
        "{view:?}"
    );

    h.shutdown().await;
}

/// 8. `model_upsert` refuses what no cell could take or the translator should
///    read (review of the strand, m-3): a prompt block over 8 KiB (the llm
///    cell's `MODEL_PROMPT_MAX_BYTES`), strengths over 2 KiB, an id that is
///    not one short token. Refused by name, never cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_upsert_refuses_oversized_fields() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let subs = MockOpenAI::start(subscribers_mock_answers()).await;
    let translator = MockOpenAI::start(vec![]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(
        &td,
        &root,
        &format!("{}/v1", subs.base_url),
        &format!("{}/v1", translator.base_url),
    );
    let (h, mut rx) = boot(&td).await;
    let long_id = format!("vendor/{}", "m".repeat(200));
    for model in [
        json!({"model_id": "test/big", "prompt": "p".repeat(8 * 1024 + 1)}),
        json!({"model_id": "test/big", "strengths": "s".repeat(2 * 1024 + 1)}),
        json!({"model_id": "test/two words"}),
        json!({"model_id": "test/line\nbreak"}),
        json!({"model_id": long_id}),
    ] {
        let (_, ack) = command(&h, &mut rx, json!({"op": "model_upsert", "model": model})).await;
        assert_eq!(ack["outcome"], "rejected", "{ack}");
        assert_eq!(ack["reason_code"], "invalid_model_field", "{ack}");
    }
    let (_, ack) = command(
        &h,
        &mut rx,
        json!({"op": "model_upsert", "model": {"model_id": "test/ok:free",
               "prompt": "p".repeat(8 * 1024), "strengths": "s".repeat(2 * 1024)}}),
    )
    .await;
    assert_eq!(ack["reason_code"], "model_upserted", "{ack}");
    let rows = admin(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "models", "columns": ["model_id"],
               "limit": 10}),
    )
    .await;
    assert_eq!(rows, json!([{"model_id": "test/ok:free"}]), "{rows}");

    h.shutdown().await;
}

/// The literals the hand bounds itself with, against what they mirror: an
/// open question outlives the translator's backstop (review I-1), and the
/// prompt block `model_upsert` takes is exactly what an llm cell accepts as
/// `model_prompt` (m-3). A literal that drifts from its neighbour fails here,
/// not in a colony.
#[test]
fn the_hands_bounds_match_what_they_mirror() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let read = |rel: &str| -> Value {
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(root.join(rel)).unwrap())
            .unwrap()
    };
    let hand = read("hand/config.json");
    let script = hand["params"]["script_inline"].as_str().unwrap_or_default();
    let literal = |name: &str| -> u64 {
        let line = script
            .lines()
            .find(|l| l.starts_with(&format!("{name} = ")))
            .unwrap_or_else(|| panic!("{name} is not a literal of the hand"));
        line.split('=')
            .nth(1)
            .unwrap_or_default()
            .split('*')
            .map(|f| f.trim().parse::<u64>().unwrap_or_else(|_| panic!("{line}")))
            .product()
    };
    let backstop = read("translate/config.json")["cell"]["message_timeout"]
        .as_u64()
        .unwrap_or(0);
    assert!(backstop > 0, "the translator has a backstop");
    assert!(
        literal("OPEN_QUESTION_SECONDS") * 1000 > backstop,
        "an open question must outlive the translator's backstop ({backstop} ms)"
    );
    assert_eq!(
        literal("MODEL_PROMPT_MAX_BYTES") as usize,
        meclaw_cells::llm::params::MODEL_PROMPT_MAX_BYTES
    );
    assert_eq!(
        literal("REQUIREMENT_MAX_BYTES") as usize,
        meclaw_cells::llm::params::REQUIREMENT_MAX_BYTES
    );
}

/// The shipped catalogue is real, dated and complete enough to translate
/// against: every row has prices, a context window and a sentence of
/// strengths, says in its note when it was read, and every tier points at an
/// ACTIVE row. The prices themselves are not pinned -- they move with the
/// provider, and the note says when they were read.
#[test]
fn the_shipped_catalogue_is_dated_and_translatable() {
    let Some(root) = shipped_registry() else {
        return;
    };
    let read_rows = |rel: &str| -> Vec<Value> {
        std::fs::read_to_string(root.join(rel))
            .unwrap()
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| meclaw_core::serde_json::from_str(l).unwrap())
            .collect()
    };
    let models = read_rows("store/seed/models.jsonl");
    assert!(models.len() >= 5, "a handful of rows: {}", models.len());
    let mut active = Vec::new();
    for m in &models {
        let id = m["model_id"].as_str().unwrap_or_default();
        assert!(id.contains('/'), "a provider id: {m}");
        assert!(m["cost_in"].as_i64().is_some_and(|c| c >= 0), "{m}");
        assert!(m["cost_out"].as_i64().is_some_and(|c| c >= 0), "{m}");
        assert!(m["context_window"].as_i64().is_some_and(|c| c > 0), "{m}");
        assert!(
            m["strengths"].as_str().is_some_and(|s| s.len() > 20),
            "the translator reads the strengths: {m}"
        );
        assert!(
            m["note"].as_str().is_some_and(|n| n.contains("2026-")),
            "a row says when it was read: {m}"
        );
        assert!(
            !m["base_url"].as_str().unwrap_or_default().is_empty(),
            "{m}"
        );
        match m["status"].as_str() {
            Some("active") => active.push(id.to_string()),
            Some("retired") => {}
            other => panic!("status {other:?}: {m}"),
        }
    }
    for t in read_rows("store/seed/tiers.jsonl") {
        let mid = t["model_id"].as_str().unwrap_or_default();
        assert!(
            active.iter().any(|a| a == mid),
            "tier {t} points at a row that is not active"
        );
    }
}
