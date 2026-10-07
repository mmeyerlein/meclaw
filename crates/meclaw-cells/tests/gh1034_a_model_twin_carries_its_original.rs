//! GH #1034 -- a model twin takes from its original what it does not bring
//! itself: the instructions, the durable slots, the reasoning budget.
//!
//! Swapping an `llm` cell for one on another model is two diff entries: a
//! template for the twin (its standing `config.json` with the model changed)
//! and a `swap_nodes` that swings the original's edges onto it. Measured in a
//! lab colony (06.10.2026): the twins were born without the original's
//! `seed/`, so a judge whose instructions live in its `system` table answered
//! like a chat assistant and every state behind it stayed empty; and every
//! swapped brain behind a curator held 0 of the 47-57 tools its curator ledger
//! listed, because a curator hands its durable slots (`system.tools`, ...) to
//! its brain only when one changes -- the new brain started with none and the
//! curator believed it held them. No error anywhere.
//!
//! The rule completes the twin and never refuses it -- a deliberate change is
//! the main case of a swap (another model often needs other reasoning params;
//! a provider refuses an effort beside a reasoning limit, GH #854). Measured
//! where it lands:
//!
//! 1. a twin that brings neither a seed nor a budget stands with the
//!    original's `seed/` byte for byte, the original's `system` rows (seeded
//!    AND written at run time) and the original's budget -- and its next call
//!    offers the model the same tools, the same system text and the same
//!    budget as the last call of the original;
//! 2. the `add_nodes` spelling of the same swap carries the same;
//! 3. a twin that declares a budget of its own keeps it, and nothing of the
//!    original's budget is mixed in -- while the curator's slots still arrive;
//! 4. the budget a twin inherits is the one the original RUNS on, a run-time
//!    params update included;
//! 5. a twin that brings instructions of its own keeps them, and the slots it
//!    does not hold from them -- the curator's menu among them -- come from the
//!    original.
//!
//! Free of a paid call by construction: every provider here is the in-process
//! mock, and every model name is generic.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30 s convention -- not a timing claim.
const MARKER: Duration = Duration::from_secs(30);

/// The original's instructions, in the staging form (the header names
/// `updated_at`, every row carries it).
const SEED: &str = concat!(
    r#"{"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}"#,
    "\n",
    r#"{"slot_path": "identity.role", "value": {"text": "You are the judge. Answer with a verdict only."}, "updated_at": 0}"#,
    "\n",
    r#"{"slot_path": "instructions.form", "value": {"text": "One line: GO or NO-GO, then the reason."}, "updated_at": 0}"#,
    "\n",
);

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The original's reasoning budget.
fn original_budget() -> Value {
    json!({"reasoning_effort": "low", "max_tokens": 4096})
}

fn llm_config(base_url: &str, model: &str, budget: &Value) -> String {
    let mut params = json!({
        "provider": "openai", "model": model, "api_key": "test-key", "base_url": base_url
    });
    for (k, v) in budget.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    meclaw_core::serde_json::to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": params,
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// The curator's menu: three tools under `system.tools`, `$replace` like the
/// collector writes it.
fn menu() -> Value {
    let mut tools = meclaw_core::serde_json::Map::new();
    tools.insert("$replace".into(), json!(true));
    for name in ["character_name", "consult_cogny", "hangup"] {
        let tool = json!({"type": "function", "function": {"name": name, "parameters": {}}});
        tools.insert(name.into(), json!({"text": tool.to_string()}));
    }
    Value::Object(tools)
}

fn turn(text: &str) -> Value {
    json!([{"origin": "user", "type": "text", "text": text}])
}

struct Rig {
    td: TempDir,
    h: ColonyHandle,
    mock: MockOpenAI,
    sink_rx: mpsc::Receiver<Message>,
}

/// `/brain` (model-a, seeded, the original budget) -> `/sink`, and a template
/// `twin` on model-b with the budget and the seed the test gives it.
async fn boot(twin_budget: Value, twin_seed: Option<&str>, responses: Vec<MockResponse>) -> Rig {
    let mock = MockOpenAI::start(responses).await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        &meclaw_core::serde_json::to_string_pretty(&json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [{"from": "./brain", "to": "/sink"}]}}
        }))
        .unwrap(),
    );
    write(
        td.path(),
        "main/brain/config.json",
        &llm_config(&base_url, "model-a", &original_budget()),
    );
    write(td.path(), "main/brain/seed/system.jsonl", SEED);
    let twin = td.path().join("templates").join("twin");
    write(&twin, "template.json", r#"{"name":"twin"}"#);
    write(
        &twin,
        "config.json",
        &llm_config(&base_url, "model-b", &twin_budget),
    );
    if let Some(seed) = twin_seed {
        write(&twin, "seed/system.jsonl", seed);
    }

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    rescan_templates(&h, td.path().join("templates")).await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap");
    Rig {
        td,
        h,
        mock,
        sink_rx,
    }
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the rescan must not abort");
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// One turn into `to`, waited out at the sink.
async fn one_turn(rig: &mut Rig, to: &str, body: Value) {
    rig.h
        .send(
            MessageBuilder::new(Path::new(to))
                .body(Body::Inline(body))
                .build(),
        )
        .await;
    tokio::time::timeout(MARKER, rig.sink_rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("the answer of {to} reaches the sink"));
}

/// The `system` table of a cell, `(slot_path, value)` in key order.
fn system_rows(cell_dir: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("cell.db");
    let mut stmt = conn
        .prepare("SELECT slot_path, value FROM system ORDER BY slot_path")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Every file under `<cell_dir>/seed`, bytes by relative name.
fn seed_bytes(cell_dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let dir = cell_dir.join("seed");
    let mut out = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.map(Result::unwrap) {
            out.insert(
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            );
        }
    }
    out
}

fn tool_names(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|ts| {
            ts.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn system_text(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .filter(|m| m["role"] == "system")
                .map(|m| m["content"].to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn committed(outcome: &MutationOutcome) {
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a twin is completed, never refused: {outcome:?}"
    );
}

/// The budget fields of one request as the provider received it.
fn wire_budget(body: &Value) -> Vec<(String, Value)> {
    ["max_tokens", "reasoning", "reasoning_effort", "thinking"]
        .iter()
        .map(|k| (k.to_string(), body.get(*k).cloned().unwrap_or(Value::Null)))
        .collect()
}

/// The menu turn into the original, so it holds the curator's slots.
async fn hand_the_menu(rig: &mut Rig) {
    one_turn(
        rig,
        "/brain",
        json!({"system": {"tools": menu()}, "messages": turn("verdict?")}),
    )
    .await;
}

async fn swap_in_the_twin(rig: &Rig) {
    let swap = send_mutation(
        &rig.h,
        json!({"scope": "/", "diff": {"swap_nodes": [
            {"match": {"name": "brain"}, "with": {"template": "twin", "name": "brain-b"}}
        ]}}),
    )
    .await;
    committed(&swap);
}

fn slot<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rows.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn budget(cell_dir: &std::path::Path) -> Vec<(String, Value)> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).unwrap();
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    [
        "reasoning_effort",
        "reasoning",
        "thinking_budget",
        "max_tokens",
    ]
    .iter()
    .map(|k| (k.to_string(), cfg["params"][*k].clone()))
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1034_the_twin_is_born_with_the_originals_seed_slots_and_budget() {
    let mut rig = boot(
        json!({}),
        None,
        vec![
            canned_chat_completion("GO, the numbers hold.", "stop"),
            canned_chat_completion("GO, still.", "stop"),
        ],
    )
    .await;
    // The curator's durable slot, handed over once with the turn it changed in.
    one_turn(
        &mut rig,
        "/brain",
        json!({"system": {"tools": menu()}, "messages": turn("verdict?")}),
    )
    .await;

    let swap = send_mutation(
        &rig.h,
        json!({"scope": "/", "diff": {"swap_nodes": [
            {"match": {"name": "brain"}, "with": {"template": "twin", "name": "brain-b"}}
        ]}}),
    )
    .await;
    assert!(
        matches!(swap, MutationOutcome::Committed { .. }),
        "a twin that brings neither seed nor budget is swapped in: {swap:?}"
    );

    let original = rig.td.path().join("main/brain");
    let twin = rig.td.path().join("main/brain-b");
    assert_eq!(
        seed_bytes(&twin),
        seed_bytes(&original),
        "the twin stands with the original's seed/, byte for byte"
    );
    let rows = system_rows(&original);
    assert!(
        rows.iter().any(|(k, _)| k == "tools.hangup"),
        "the original held the menu: {rows:?}"
    );
    assert_eq!(
        system_rows(&twin),
        rows,
        "the twin holds every system row of the original, seeded and run-time alike"
    );
    assert_eq!(
        budget(&twin),
        budget(&original),
        "and the original's reasoning budget"
    );

    // What the curator sends next: the turn only -- nothing of its system changed.
    one_turn(&mut rig, "/brain-b", json!({"messages": turn("and now?")})).await;
    let requests = rig.mock.recorded_requests().await;
    assert_eq!(requests.len(), 2, "one call each side of the swap");
    assert_eq!(requests[1].model(), Some("model-b"), "the twin answered");
    assert_eq!(
        tool_names(&requests[1].body),
        vec!["character_name", "consult_cogny", "hangup"],
        "the twin's first call offers the tools the original's last call offered"
    );
    assert_eq!(tool_names(&requests[1].body), tool_names(&requests[0].body));
    assert_eq!(
        system_text(&requests[1].body),
        system_text(&requests[0].body),
        "and the same instructions"
    );
    assert!(
        system_text(&requests[1].body)
            .iter()
            .any(|t| t.contains("You are the judge")),
        "which are the seeded ones: {:?}",
        system_text(&requests[1].body)
    );
    assert_eq!(
        wire_budget(&requests[1].body),
        wire_budget(&requests[0].body),
        "and the same budget on the wire"
    );
    rig.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1034_the_add_nodes_spelling_carries_the_same() {
    let mut rig = boot(json!({}), None, vec![canned_chat_completion("GO.", "stop")]).await;
    one_turn(
        &mut rig,
        "/brain",
        json!({"system": {"tools": menu()}, "messages": turn("verdict?")}),
    )
    .await;
    let swap = send_mutation(
        &rig.h,
        json!({"scope": "/", "diff": {
            "add_nodes": [{"name": "brain-b", "template": "twin"}],
            "swap_nodes": [{"match": {"name": "brain"}, "with": {"name": "brain-b"}}]
        }}),
    )
    .await;
    assert!(
        matches!(swap, MutationOutcome::Committed { .. }),
        "the generation-change spelling swaps the twin in: {swap:?}"
    );
    let original = rig.td.path().join("main/brain");
    let twin = rig.td.path().join("main/brain-b");
    assert_eq!(seed_bytes(&twin), seed_bytes(&original));
    assert_eq!(system_rows(&twin), system_rows(&original));
    rig.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1034_a_twin_with_a_budget_of_its_own_keeps_it_unmixed() {
    // The OpenRouter form of a reasoning model: a limit, and no effort beside it.
    let mut rig = boot(
        json!({"thinking_budget": 1024, "max_tokens": 8192}),
        None,
        vec![
            canned_chat_completion("GO.", "stop"),
            canned_chat_completion("GO, still.", "stop"),
        ],
    )
    .await;
    hand_the_menu(&mut rig).await;
    swap_in_the_twin(&rig).await;
    let original = rig.td.path().join("main/brain");
    let twin = rig.td.path().join("main/brain-b");
    let b = budget(&twin);
    assert_eq!(
        b,
        vec![
            ("reasoning_effort".to_string(), Value::Null),
            ("reasoning".to_string(), Value::Null),
            ("thinking_budget".to_string(), json!(1024)),
            ("max_tokens".to_string(), json!(8192)),
        ],
        "the twin keeps its own budget, and no effort of the original's is mixed in"
    );
    assert_eq!(
        seed_bytes(&twin),
        seed_bytes(&original),
        "the seed it did not bring is still carried"
    );
    assert_eq!(
        system_rows(&twin),
        system_rows(&original),
        "and so are the slots"
    );
    one_turn(&mut rig, "/brain-b", json!({"messages": turn("and now?")})).await;
    let requests = rig.mock.recorded_requests().await;
    assert_eq!(
        tool_names(&requests[1].body),
        vec!["character_name", "consult_cogny", "hangup"],
        "the curator's menu reaches the twin's first call"
    );
    assert!(
        requests[1].body.get("reasoning_effort").is_none()
            && requests[1].body["reasoning"].get("effort").is_none()
            && requests[1].body["reasoning"]["max_tokens"] == 1024,
        "the twin's limit on the wire, and no inherited effort beside it: {}",
        requests[1].body
    );
    rig.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1034_the_budget_inherited_is_the_one_the_original_runs_on() {
    let rig = boot(json!({}), None, vec![]).await;
    // A model push changed the original's effort at run time: the overlay row
    // is what the cell runs on, its config.json is the birth snapshot.
    {
        let conn = rusqlite::Connection::open(rig.td.path().join("main/brain/cell.db")).unwrap();
        conn.busy_timeout(MARKER).unwrap();
        conn.execute(
            "INSERT INTO params (key, value, updated_at) VALUES ('reasoning_effort', '\"high\"', 1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .unwrap();
    }
    swap_in_the_twin(&rig).await;
    let b = budget(&rig.td.path().join("main/brain-b"));
    assert_eq!(
        b[0],
        ("reasoning_effort".to_string(), json!("high")),
        "the running effort, not the birth one: {b:?}"
    );
    assert_eq!(b[3], ("max_tokens".to_string(), json!(4096)), "{b:?}");
    rig.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1034_a_twin_with_instructions_of_its_own_keeps_them() {
    let own = concat!(
        r#"{"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}"#,
        "\n",
        r#"{"slot_path": "identity.role", "value": {"text": "You are a careful reviewer."}, "updated_at": 0}"#,
        "\n",
    );
    let mut rig = boot(
        json!({}),
        Some(own),
        vec![canned_chat_completion("GO.", "stop")],
    )
    .await;
    hand_the_menu(&mut rig).await;
    swap_in_the_twin(&rig).await;
    let twin = rig.td.path().join("main/brain-b");
    assert_eq!(
        seed_bytes(&twin).get("system.jsonl").map(Vec::as_slice),
        Some(own.as_bytes()),
        "the twin keeps the seed it brought"
    );
    let rows = system_rows(&twin);
    assert!(
        slot(&rows, "identity.role").is_some_and(|v| v.contains("careful reviewer")),
        "its own instruction wins over the original's: {rows:?}"
    );
    for key in [
        "tools.character_name",
        "tools.consult_cogny",
        "tools.hangup",
    ] {
        assert!(
            slot(&rows, key).is_some(),
            "the curator's slot {key} comes from the original: {rows:?}"
        );
    }
    assert!(
        slot(&rows, "instructions.form").is_some(),
        "and so does every slot its own seed does not hold: {rows:?}"
    );
    rig.h.shutdown().await;
}
