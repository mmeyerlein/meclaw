//! GH #263 — a push writes a slot; it must not buy an inference.
//!
//! `affinity/brief` answers on two lanes through one `answer()`. GH #242 put
//! the pack into the `tool_result` text so a sealed agent hive could read it,
//! and GH #258 gave every slot a `text` rendering so an `llm` cell could render
//! it. Both fixes were about the TOOL lane's shape, and both left the
//! `messages[]` slot on the PUSH lane, where nobody asked for it.
//!
//! An `llm` cell stays silent for a body with **no** `messages[]` at all
//! (`llm::cell`, step 4: `None => return`) — that silence is the entire reason
//! the README can promise that a slot update costs nothing. With a
//! `messages[]` beside the `system` slot the cell does the opposite: it calls
//! the provider, and it calls it on a `tool_result` whose `call_id` this cell
//! never opened. Against a real provider that is an orphaned tool result and
//! therefore a 400; the mock accepts it, which is why no test noticed.
//!
//! # What is measured here
//!
//! The ABSENCE of a provider request, which is why both halves run against a
//! counting mock rather than against the body that arrived:
//!
//! 1. **The push lane calls nobody.** The shipped hive drives a real tick
//!    through `./clock` → `./push` → `./brief`; the answer it produces is
//!    handed to a real `LlmCell` in front of a mock provider, and the mock must
//!    have recorded nothing at all.
//! 2. **And it is not silent by being empty.** The same delivery has to leave
//!    the four documented slots in the subscriber's `cell.db` — a "fix" that
//!    drops the push message would pass claim 1 and fail here.
//! 3. **The tool lane still costs exactly one.** The same brief asked over the
//!    tool lane keeps its `tool_result` turn, and the subscriber's model is
//!    called exactly once for it.

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

/// The shipped template, copied the way instantiation copies it.
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
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

// ────────────────────────────────────────────────────────── the test-only cells

/// The asking side of the read port: one tool call, and the audience declared
/// on the hop for the port edge to promote to `context.asker`.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
sys.stdout.write(json.dumps({
    "header": {"route": "brief", "audience": str(a.get("audience") or ""),
               # GH #306: the ROUND is edge truth too, and the door refuses a
               # request that declares none. This lane is a 1:1, so it says so.
               # GH #330: it has one name, `audience_set`; the retired spelling
               # is not read by the door any more.
               "audience_set": json.dumps([str(a.get("audience") or "")])},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-263",
                  "text": json.dumps({"subject": a.get("subject"),
                                      "channel": a.get("channel") or "*"})}]}))
"#;

/// The writing side of the write port, with the actor on the hop -- and, since
/// GH #288, the subscriber address beside it. A subscription names the cell that
/// will be handed somebody's briefs; that is a routing decision, so it is the
/// wiring's to state and not the body's. A shipped producer stamps it from its
/// own topology, which is what this stand-in does with the constant below.
const WRITER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "propose", "actor": "member:alex",
               "subscriber": "/main/consumer"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-263-w",
                  "text": raw}]}))
"#;

fn code_cell(script: &str, route: &str, extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": [route], "required": false}});
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
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the shipped affinity template.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// Both lanes at the hive PATH, and one exit for both answers — `out_brief` and
/// `out_push` are told apart by `hop.subscriber`, exactly as the README says.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./asker", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'brief'",
         "modifier": {"set_hop": {"route": "'in_brief'"},
                      "set_context": {"asker": "hop.audience"}}},
        {"from": "./writer", "to": "./affinity",
         "condition": "has(hop.route) && hop.route == 'propose'",
         // GH #288: the write lane declares `subscriber` beside `actor`, so
         // this edge promotes both. The `has()` guard is not decoration -- an
         // unresolvable `set_context` expression makes the modifier fail and
         // the edge is SKIPPED, so a request without the key would vanish
         // instead of being refused by the gate.
         "modifier": {"set_hop": {"route": "'in_propose'"},
                      "set_context": {
                          "actor": "hop.actor",
                          "subscriber": "has(hop.subscriber) ? hop.subscriber : ''"}}},
        {"from": "./affinity", "to": "/sink",
         "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'ack' \
                       || hop.route == 'error')"}
    ]}}})
}

/// Short enough that a tick lands inside the test budget, long enough that the
/// subscribe write is committed before the tick that must find it.
const FAST_CRON: &str = "*/2 * * * * *";

/// The subscribing cell's address. Nothing runs there in this colony: what the
/// subscriber would do with the answer is measured below, against a mock.
///
/// `WRITER` spells the same path a second time, because a raw script string
/// cannot interpolate a Rust const. The two must stay in step: since GH #288
/// the address reaches the gate on the hop, and a mismatch would push at a path
/// `recv_answer` never waits for.
const SUBSCRIBER: &str = "/main/consumer";

async fn boot(
    root_template: &std::path::Path,
) -> (tempfile::TempDir, ColonyHandle, mpsc::Receiver<Message>) {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!("AFFINITY_PUSH_CRON={FAST_CRON}\n"),
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            "brief",
            json!({"audience": {"type": "string", "required": false},
                   "audience_set": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/writer/config.json",
        &code_cell(
            WRITER,
            "propose",
            json!({"actor": {"type": "string", "required": false},
                   "subscriber": {"type": "string", "required": false}}),
        ),
    );
    copy_cells(root_template, &root.join("main/affinity"));
    // `${uuid7:…}` is minted on the instantiation path; a raw filesystem
    // bootstrap has to be handed a literal.
    patch(root, "main/affinity/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-000000000263");
    });

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

fn to(cell: &str, text: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
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

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next answer whose `hop.subscriber` matches, skipping acks and the other
/// lane's answers.
async fn recv_answer(rx: &mut mpsc::Receiver<Message>, subscriber: &str) -> Message {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..16 {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                panic!("no answer for subscriber {subscriber:?} arrived; saw {seen:?}")
            });
        if hop_of(&m, "route") == "answer" && hop_of(&m, "subscriber") == subscriber {
            return m;
        }
        seen.push(format!(
            "{}/{}",
            hop_of(&m, "route"),
            hop_of(&m, "subscriber")
        ));
    }
    panic!("no answer for subscriber {subscriber:?} among 16 messages; saw {seen:?}");
}

/// Both answers of the shipped hive for one and the same subject: the one a
/// tick pushed at `SUBSCRIBER`, and the one the tool lane served.
async fn both_lanes() -> Option<(Value, Value)> {
    let root = shipped_affinity()?;
    let (_td, h, mut rx) = boot(&root).await;

    // GH #288: the body says WHAT to subscribe to and on which channel; WHERE
    // the pushes go and WHO they are cut for come off the edge, so naming
    // either here would now be refused with `identity_from_body`.
    let op = json!({"op": "subscribe", "subject": "entity:alex",
                    "channel": "telegram"});
    h.send(to(
        "/writer",
        &meclaw_core::serde_json::to_string(&op).unwrap(),
    ))
    .await;
    let pushed = body_of(&recv_answer(&mut rx, SUBSCRIBER).await).clone();

    h.send(to(
        "/asker",
        r#"{"audience":"agent:aiden","subject":"entity:alex","channel":"telegram"}"#,
    ))
    .await;
    let asked = body_of(&recv_answer(&mut rx, "").await).clone();

    h.shutdown().await;
    Some((pushed, asked))
}

// ─────────────────────────────────────────────────────── the receiving side

/// The four slots the README promises this lane writes.
const SLOTS: [&str; 4] = ["identity", "peer", "relationship", "channel"];

/// A real `llm` cell on a fresh `cell.db`, pinned to the four documented slots
/// and pointed at the mock.
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

/// Deliver one body into the cell exactly as the colony would.
async fn deliver(cell: &mut LlmCell, db: &mut DbConn, body: Value) {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(SUBSCRIBER),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(SUBSCRIBER))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let _ = rx.recv().await;
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// GH #263, claims 1 and 2. A pushed brief updates the subscriber's system tree
/// and calls nobody — and it is not silent by being empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_brief_updates_the_slots_without_calling_a_provider() {
    let Some((pushed, _)) = both_lanes().await else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = subscriber(&td, &mock.base_url);

    deliver(&mut cell, &mut db, pushed.clone()).await;

    // Claim 1. The whole point of the lane: a change costs a write, not a call.
    let requests = mock.recorded_requests().await;
    assert!(
        requests.is_empty(),
        "a push must not reach the provider at all, got {} request(s); the \
         pushed body was {pushed}",
        requests.len()
    );

    // Claim 2. Silence bought by delivering nothing would be the wrong fix.
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
        "the push still has to write the four documented slots: {pushed}"
    );
}

/// GH #263, claim 3. The tool lane is the one that ASKED, so it keeps its
/// `tool_result` turn and costs exactly the one inference it always did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tool_lane_still_costs_exactly_one_inference() {
    let Some((_, asked)) = both_lanes().await else {
        return;
    };
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = subscriber(&td, &mock.base_url);

    deliver(&mut cell, &mut db, asked.clone()).await;

    let requests = mock.recorded_requests().await;
    assert_eq!(
        requests.len(),
        1,
        "the lane that opened a call must still get its answer inferred: {asked}"
    );
}
