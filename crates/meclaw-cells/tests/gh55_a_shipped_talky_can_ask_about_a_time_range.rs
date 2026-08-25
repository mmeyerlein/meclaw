//! GH #55 — a talky instantiated from the shipped library answers a time-range
//! question without its owner hand-writing a tool schema first.
//!
//! The issue's done-when has two halves. The edge half (Task 14) is that the
//! composite serves `memory_recall` itself; this file carries the SCHEMA half:
//! `templates/talky/brain/seed/system.jsonl` ships the two tool schemas the
//! composite implements, so the model is told the tool exists — and is told the
//! two window arguments exist, which is the whole of the time-range question.
//!
//! Without the seed the brain arrives at the provider with no `tools[]` at all.
//! A model that is never shown `memory_recall` cannot call it, so every edge
//! Task 14 draws is unreachable and the owner is back to hand-writing a schema
//! into `system.tools` before the agent can answer "what did we say last week".
//!
//! # Why this goes to the wire and reads the shipped bytes
//!
//! The chain here is the shipped one: the bytes of `templates/talky/` (every
//! `config.json` **and** the brain's `seed/`), `bootstrap_from_filesystem`, and
//! the real `LlmCellFactory`, which loads the seed exactly the way a boot loads
//! it. The assertion is on what the provider recorded and on what came out of
//! the composite — never on the seed file's own text, which would pass on a
//! seed the loader never reads. The sibling pin for that pattern is
//! `gh342_the_shipped_judge_tool_reaches_the_wire.rs`, whose seed this one
//! copies the form of.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Whether the library ships with this checkout (the documented R2b exception
/// form): a public clone without `templates/` skips instead of failing.
fn shipped() -> bool {
    templates_root().join("talky/config.json").is_file()
}

/// The shipped template, copied cell by cell. Unlike `talky_composite.rs` this
/// copy takes the `seed/*.jsonl` files as well — they are exactly what is under
/// test here, and a harness that dropped them would prove nothing about the
/// library it claims to boot.
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
            || (src.file_name().is_some_and(|d| d == "seed")
                && from.extension().is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

/// GH #277: a directory whose `config.json` declares `cell.type: "ref"` is a
/// REFERENCE, not a cell — the referenced template's tree belongs in its place.
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
        let name = reference.split('@').next().unwrap_or_default();
        dir = templates_root().join(name);
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
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// A fixed schedule id — `${uuid7:*}` is an INSTANTIATION-side substitution, so
/// a tree written straight to disk carries a real one.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c5500";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

/// The round these turns are spoken in, in the affinity vocabulary the audience
/// gate speaks (ADR-0002 E8).
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;

/// The two tools the composite serves itself, in the alphabetical order
/// `extract_tools` puts them on the wire in.
const SERVED_TOOLS: [&str; 2] = ["memory_recall", "thread_recall"];

/// The time range the model asks about. These are the MODEL's own argument
/// values: they exist nowhere in the tree, so seeing them come out of the
/// composite proves they travelled from the wire and were not defaulted.
const WINDOW_FROM: &str = "2026-08-01T00:00:00Z";
const WINDOW_TO: &str = "2026-08-08T00:00:00Z";

// ────────────────────────────────────────────────────────── the test-only cells

/// A `code` cell config with the contract the substrate validates against.
fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the talky composite.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The surface: turns a harness message into the ingress lane, promoting the
/// channel the keeper needs — exactly what a real parent does.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn", "chat_id": "c-55"},
                             "messages": d.get("messages", [])}))
"#;

/// The port wiring a parent draws around the composite. The `recall` edge is
/// the shipped recipe from `templates/collector/README.md` § *The memory tool*:
/// the five hop keys promoted to context, which is where the memory hive reads
/// them.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        // the recall port -- the capture stands in for the memory hive
        {"from": "./talky", "to": "/recall",
         "condition": "has(hop.route) && hop.route == 'recall'",
         "modifier": {"set_context": {"recall_query": "hop.recall_query",
                                      "memory_tier": "hop.memory_tier",
                                      "memory_call_id": "hop.memory_call_id",
                                      "recall_window_from": "hop.recall_window_from",
                                      "recall_window_to": "hop.recall_window_to"}}},
        // reply exit
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // the remaining declared exits, drained so nothing dead-letters
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && (hop.route == 'write' || hop.route == 'turn_write' \
          || hop.route == 'extraction' || hop.route == 'prune' || hop.route == 'error' \
          || hop.route == 'tool')"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n",
    )
    .unwrap();
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
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));

    // Two patches, both about the clock and the wire rather than about
    // behaviour: a schedule the test can never trigger, and the two llm cells
    // pointed at the mock. `${ctx.model}` is an INSTANTIATION substitution; a
    // tree booted from disk carries a literal.
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    for rel in [
        "main/talky/brain/config.json",
        "main/talky/summarizer/writer/config.json",
    ] {
        patch(root, rel, |v| {
            v["params"]["base_url"] = json!(base_url);
            v["params"]["model"] = json!("gpt-4o-mock");
        });
    }
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
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
    let (recall_tx, recall_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/recall"), move || {
        CaptureCell::new(recall_tx.clone())
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
        .expect("bootstrap_from_filesystem must succeed");
    (h, recall_rx, park_rx)
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(200)
        .build()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

fn context_of(m: &Message, key: &str) -> String {
    m.headers
        .context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The model's answer to the time-range question: one `memory_recall` call
/// carrying both window arguments. Nothing in the tree could have produced
/// these two values.
fn asks_about_the_window() -> meclaw_testing::mock_http::MockResponse {
    let args = json!({
        "query": "what we said",
        "window_from": WINDOW_FROM,
        "window_to": WINDOW_TO
    })
    .to_string();
    canned_tool_calls(vec![("call-window", "memory_recall", &args)])
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// **The assertion GH #55 turns on.** The shipped tree, booted as shipped: the
/// very first request the brain makes carries a `tools[]` array in which
/// `memory_recall` is declared. Nobody wrote a schema — the seed did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_talky_declares_memory_recall_to_the_provider() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![asks_about_the_window()]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut recall_rx, _park_rx) = boot(&td).await;

    h.send(turn("what did we talk about in the first week of august?"))
        .await;
    // The recall is what proves the round got as far as the provider AND back;
    // it is asserted properly in the sibling test below.
    let _ = recv_bounded(&mut recall_rx).await;

    let reqs = mock.recorded_requests().await;
    assert!(
        !reqs.is_empty(),
        "the brain must have reached the provider at all"
    );
    let tools = reqs[0].tools().expect(
        "the shipped talky's request must carry tools[] — without it the model is never \
                 shown the memory tool and the owner is back to hand-writing a schema",
    );
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert_eq!(
        names, SERVED_TOOLS,
        "the composite declares exactly the two tools it serves itself: {tools:?}"
    );

    h.shutdown().await;
}

/// The other half of the time-range question: the two window ARGUMENTS the
/// model produced reach the recall port as its own context keys. A schema that
/// declared `memory_recall` without `window_from`/`window_to` would pass the
/// test above and still leave every time-range question answered out of a point
/// query — so the values are asserted, not the shape.
///
/// This is the assertion that also needs Task 14's internal edge: the call has
/// to reach the collector INSIDE the composite for the collector to translate
/// it into a recall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_models_own_window_reaches_the_recall_port() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![asks_about_the_window()]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut recall_rx, _park_rx) = boot(&td).await;

    h.send(turn("what did we talk about in the first week of august?"))
        .await;
    let recall = recv_bounded(&mut recall_rx)
        .await
        .expect("the memory tool call must leave on the recall port");

    assert_eq!(
        context_of(&recall, "recall_window_from"),
        WINDOW_FROM,
        "the model's own window start must reach the memory hive: {:?}",
        recall.headers.context
    );
    assert_eq!(
        context_of(&recall, "recall_window_to"),
        WINDOW_TO,
        "the model's own window end must reach the memory hive: {:?}",
        recall.headers.context
    );
    assert_eq!(
        context_of(&recall, "memory_call_id"),
        "call-window",
        "a recall answering a CALL carries the call id; empty would be the ambient leg: {:?}",
        recall.headers.context
    );

    h.shutdown().await;
}

/// GH #55 Step 3: the seed carries the two tools the composite serves itself
/// and **nothing else**. No identity, no instructions, no persona — the
/// retraction the README carries draws the line at tools the composite serves,
/// and a seeded persona would cross it: the composite carries the topology, the
/// instance carries the agent.
#[test]
fn the_brain_seed_carries_tools_and_nothing_else() {
    if !shipped() {
        return;
    }
    let seed = templates_root().join("talky/brain/seed/system.jsonl");
    let text = std::fs::read_to_string(&seed).expect("the shipped brain seed is on disk");
    let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        rows.len(),
        3,
        "the schema header plus exactly two tool rows: {rows:?}"
    );

    let header: Value = meclaw_core::serde_json::from_str(rows[0]).expect("line 1 parses");
    assert!(
        header["schema"].is_object(),
        "line 1 is the schema header: {}",
        rows[0]
    );

    let slots: Vec<String> = rows[1..]
        .iter()
        .map(|l| {
            let row: Value = meclaw_core::serde_json::from_str(l).expect("a data row parses");
            row["slot_path"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    for slot in &slots {
        assert!(
            slot.starts_with("tools."),
            "the talky brain seeds tools and nothing else — {slot} is not a tool"
        );
    }
    let expected: Vec<String> = SERVED_TOOLS.iter().map(|t| format!("tools.{t}")).collect();
    assert_eq!(slots, expected, "the two tools the composite serves itself");
}
