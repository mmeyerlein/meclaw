//! GH #258 — a brief that an `llm` cell cannot render is a brief nobody reads.
//!
//! `affinity/brief` answers on two lanes. GH #242 repaired the tool lane: the
//! pack is serialised into the `tool_result` text, because a sealed agent hive
//! drops `system` at its boundary. The push lane was left as it was — the pack
//! attached as `body["system"]`, the pack object itself.
//!
//! `system.*` is the right *place* for that lane. It is not the right *shape*.
//! An `llm` cell flattens what arrives in `system.*` into leaves and stops at
//! the first object carrying a `text` key (`llm::state::flatten_to_leaves`);
//! it then concatenates exactly those `text` values into the system prompt
//! (`llm::translate::concat_system_prompt`). A pack of nested slot documents
//! has no `text` key anywhere, so it produces no leaf at all: nothing is
//! persisted, nothing is rendered, and the subscriber's model answers from
//! whatever else it has while the audit table says `ok`.
//!
//! # Why this file boots the hive and then a real `llm` cell
//!
//! Asserting that the `system` slot ARRIVES proves nothing — it arrived before
//! this fix too. The effect being claimed is one step further on: the
//! recipient's **composed system prompt** carries the disclosed material. So
//! the shipped template runs in a real colony (same guard form as
//! `gh241_a_brief_answers_the_call_that_asked_for_it.rs`), and the answer it
//! produces is handed to a real `LlmCell` pointed at a mock provider. What the
//! provider was sent is what the model saw.
//!
//! # The claims
//!
//! 1. **The pack reaches the prompt.** The captured request carries a `system`
//!    message and the disclosed material is in it.
//! 2. **It reaches it through the four documented slot paths.** The receiving
//!    cell pins `system_writable` to `identity`, `peer`, `relationship` and
//!    `channel` — the four slots the README promises — so a rendering smuggled
//!    in under a fifth slot would be refused rather than silently accepted.
//! 3. **The rendering discloses nothing extra.** What no `disclosure` row named
//!    is still absent from the prompt.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

/// Every file the hive is made of; a missing one makes this file skip rather
/// than fail (GH #49 — `affinity` is not in `PUBLIC_TEMPLATES`).
const AFFINITY_FILES: &[&str] = &[
    "config.json",
    "store/config.json",
    "brief/config.json",
    "gate/config.json",
    "push/config.json",
    "clock/config.json",
    "store/seed/entities.jsonl",
    "store/seed/disclosure.jsonl",
    "store/seed/trust.jsonl",
    "store/seed/relations.jsonl",
];

fn shipped_affinity() -> Option<std::path::PathBuf> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/affinity");
    AFFINITY_FILES
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

/// The shipped template, copied the way instantiation copies it: the
/// `config.json` files and the seed tables next to them.
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

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
    std::fs::write(
        root.join(rel),
        meclaw_core::serde_json::to_string_pretty(v).unwrap(),
    )
    .unwrap();
}

// ────────────────────────────────────────────────────────── the asking side

/// The subscriber's stand-in on the request side: it opens one tool call and
/// declares the audience on the hop, which the port edge turns into
/// `context.asker`. `brief` learns an audience no other way.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
sys.stdout.write(json.dumps({
    "header": {"route": "brief", "audience": str(a.get("audience") or "")},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-258",
                  "text": json.dumps({"subject": a.get("subject"),
                                      "channel": a.get("channel") or "*"})}]}))
"#;

fn asker_config() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": ASKER, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["brief"], "required": false},
                    "audience": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for whoever asks the affinity for a brief.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Both lanes at the hive PATH — `params.ports` is empty, so naming a cell
/// inside the hive is refused.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./asker", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_hop": {"route": "'in_brief'"},
                      "set_context": {"asker": "hop.audience"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"}
    ]}}})
}

/// Far enough away that the push tick never fires during a test that is not
/// about ticks.
const QUIET_CRON: &str = "0 0 4 * * *";

async fn boot(
    root_template: &std::path::Path,
) -> (tempfile::TempDir, ColonyHandle, mpsc::Receiver<Message>) {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!("AFFINITY_PUSH_CRON={QUIET_CRON}\n"),
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(root, "main/asker/config.json", &asker_config());
    copy_cells(root_template, &root.join("main/affinity"));
    // `${uuid7:…}` is minted on the instantiation path; a raw filesystem
    // bootstrap has to be handed a literal.
    let clock = root.join("main/affinity/clock/config.json");
    let mut v: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&clock).unwrap()).unwrap();
    v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-000000000258");
    std::fs::write(
        &clock,
        meclaw_core::serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
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
    (td, h, sink_rx)
}

fn ask(request: &str) -> Message {
    MessageBuilder::new(Path::new("/asker"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": request}]}),
        ))
        .ttl(400)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn recv_answer(rx: &mut mpsc::Receiver<Message>) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..12 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("no answer arrived; saw {seen:?}"));
        let route = m
            .headers
            .hop
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if route == "answer" {
            return m;
        }
        seen.push(route);
    }
    panic!("no answer among 12 messages; saw {seen:?}");
}

/// One brief for `agent:aiden` about `entity:alex`, straight out of the shipped
/// hive with the shipped seed rows behind it.
async fn a_served_brief() -> Option<Value> {
    let root = shipped_affinity()?;
    let (_td, h, mut rx) = boot(&root).await;
    h.send(ask(
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let answer = body_of(&recv_answer(&mut rx).await).clone();
    h.shutdown().await;
    Some(answer)
}

// ─────────────────────────────────────────────────────── the receiving side

/// The four slots the README promises this lane writes, and the order they are
/// concatenated in.
const SLOTS: [&str; 4] = ["identity", "peer", "relationship", "channel"];

/// A real `llm` cell on a fresh `cell.db`, pinned to the four documented slots
/// and pointed at the mock. The pin is deliberate: a fix that rendered the pack
/// under a slot outside the documented four would be refused here instead of
/// passing unnoticed.
fn subscriber(td: &tempfile::TempDir, base_url: &str) -> (LlmCell, DbConn) {
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
        "system_order": SLOTS,
        "system_writable": SLOTS,
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// Deliver one body into the cell exactly as the colony would, and return what
/// the cell emitted (a reject is an emission too).
async fn deliver(cell: &mut LlmCell, db: &mut DbConn, body: Value) -> Option<Value> {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/subscriber"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/subscriber"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    rx.recv().await.map(|e| e.content)
}

/// The `system` message of the request the provider actually received — the
/// composed prompt, not the slot that arrived.
async fn composed_system_prompt(mock: &MockOpenAI) -> String {
    let reqs = mock.recorded_requests().await;
    let req = reqs
        .first()
        .expect("the subscriber must have called the provider at all");
    let msgs = req.messages().expect("an OpenAI request has messages[]");
    msgs.iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("system"))
        .filter_map(|m| m.get("content").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// GH #258, claim 1 and 3. The disclosed material is in the prompt the
/// subscriber's model was sent.
///
/// Before the fix the pack carried no `text` leaf anywhere, `flatten_to_leaves`
/// returned nothing for it, and the request went out with no `system` message
/// at all — the brief was stored nowhere and read by nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pushed_pack_reaches_the_composed_system_prompt() {
    let Some(answer) = a_served_brief().await else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = subscriber(&td, &mock.base_url);

    deliver(&mut cell, &mut db, answer.clone()).await;

    let prompt = composed_system_prompt(&mock).await;
    assert!(
        !prompt.is_empty(),
        "the brief has to reach the prompt, not just the cell: {answer}"
    );
    for disclosed in ["Alex", "Kern", "trusted", "entity:robin", "telegram"] {
        assert!(
            prompt.contains(disclosed),
            "the disclosed material must be readable in the prompt; '{disclosed}' is \
             missing from: {prompt}"
        );
    }
    // Claim 3: rendering is not disclosure. What no row named stays out, in the
    // prompt exactly as it stays out of the pack.
    for undisclosed in ["INTP", "Example City", "1980-04-12", "gardening"] {
        assert!(
            !prompt.contains(undisclosed),
            "the rendering widened the disclosure decision with '{undisclosed}': {prompt}"
        );
    }
}

/// GH #258, claim 2. The rendering rides inside the four documented slots, so a
/// subscriber that pins `system_writable` to them accepts the write — and every
/// one of those slots is a leaf, which is what makes it renderable at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_brief_writes_exactly_the_four_documented_slots() {
    let Some(answer) = a_served_brief().await else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = subscriber(&td, &mock.base_url);

    let emitted = deliver(&mut cell, &mut db, answer.clone()).await;
    let error = emitted
        .as_ref()
        .and_then(|c| c.pointer("/meta/error"))
        .cloned();
    assert_eq!(
        error, None,
        "the write must pass the slot allowlist the README promises: {answer}"
    );

    let slots = db
        .call(|conn| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare("SELECT slot_path FROM system ORDER BY slot_path")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
        .await
        .expect("cell.db is readable");
    let mut expected: Vec<String> = SLOTS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(
        slots, expected,
        "the pack must land as exactly the four documented leaves — one per slot, \
         each carrying its own rendering"
    );
}
