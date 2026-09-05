//! GH #55 / GH #283 — the shipped `talky` serves the one tool that is its own.
//!
//! `thread_recall` is not the parent's tool. The collector inside this composite
//! SERVES it — it owns the round table a `thread_recall` stub points at, in its
//! own `cell.db`, which no other cell in the substrate may read — and yet until
//! GH #55 the wiring that reached it had to be drawn from outside: the README
//! asked every parent to draw a self-loop at `./talky`, and a parent that forgot
//! got a tool call that left the composite, found no cell and stalled its round
//! until the idle window closed it.
//!
//! `memory_recall` stood beside it until `talky@5.0.0`. It does not any more
//! (GH #552): the memory belongs to the MEMBER, the rules a recall obeys are
//! enforced in the memory hive, and serving the call here meant typing that
//! hive's schema by hand. So the composite now does with that name what it does
//! with `weather` — it lets it leave on the tool lane — and the third test below,
//! which used to be the positive control, is a claim about `memory_recall` too.
//!
//! This file asks the two questions that turn the remaining recipe into topology:
//!
//! 1. does a `thread_recall` call reach the collector's own lane
//!    (`in_thread_call`) with **no** edge drawn by the parent, and
//! 2. does it stay inside — nothing on the composite's `tool` lane for that
//!    call.
//!
//! The second question is only worth asking beside a positive control, so the
//! remaining tests drive an ordinary tool name and `memory_recall` through the
//! same tree and assert both DO leave: that is the guarded default edge of
//! GH #283 firing, and it is what proves the silence in test one is the reserved
//! name being claimed rather than the lane being dead.
//!
//! Free of a real provider by construction: the brain talks to the mock OpenAI
//! wire, every other cell is a `code`/`store`/`timer` cell.

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
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
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

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION-side substitution.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1155";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";
/// The round these turns are spoken in, in the affinity vocabulary.
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;

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
/// channel exactly as a real parent does.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "turn")
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-55"},
                             "messages": d.get("messages", [])}))
"#;

/// The tool a parent wires per instance — the positive control of the third
/// test. It answers instantly so the round can close.
const TOOL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "res", "tool_call_id": hop.get("tool_call_id", "")},
    "messages": [{"origin": "tool", "type": "tool_result",
                  "id": hop.get("tool_call_id", ""),
                  "text": "berlin: 21C"}]}))
"#;

/// The port wiring a parent draws around the composite. Deliberately WITHOUT
/// the two self-loops the README used to demand: this harness is the parent
/// that never heard of `memory_recall`, which is the whole point of #55.
///
/// `./talky` is the only endpoint named from outside — the composite's own
/// address — so no edge here shares a sender with an edge inside the composite
/// and nothing in this file can silence the composite's default edge.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ingress: the surface turn, with the channel promotion the keeper needs
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        // reply exit
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        // the housekeeping pair the composite's required_drains obliges
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'prune'",
         "modifier": {"set_hop": {"route": "'in_prune'"}}},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'prune'"},
        // the error drain and the extraction lane
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'extraction'"},
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && (hop.route == 'write' || hop.route == 'turn_write')"},
        // THE TWO PROBES. Both take a lane of the composite's public contract
        // and nothing else: `tool` is what a call that LEFT travels on,
        // `recall` is what the collector asks memory with when it served a
        // `memory_recall` call itself.
        {"from": "./talky", "to": "/tool_port",
         "condition": "has(hop.route) && hop.route == 'tool'"},
        {"from": "./talky", "to": "/recall_port",
         "condition": "has(hop.route) && hop.route == 'recall'"},
        // the one per-instance tool lane, wired at the composite's own address
        // and named: a parent answers the tools it wired and NOTHING else, so a
        // reserved name that escaped would find no cell — which is exactly the
        // stall #55 is about.
        {"from": "./talky", "to": "./weather",
         "condition": "has(hop.tool_name) && hop.tool_name == 'weather'"},
        {"from": "./weather", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'res'",
         "modifier": {"set_hop": {"route": "'in_tool'"}}}
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
            &["turn", "prune"],
            json!({"chat_id": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/weather/config.json",
        &code_cell(
            TOOL,
            &["res"],
            json!({"tool_call_id": {"type": "string", "required": false}}),
        ),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));

    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // Every open generation is a candidate the moment the sweep runs. It was a
    // `KEEPER_IDLE_MS=0` line in the `.env` above until GH #138; the knob is a
    // param of `./close` now, so such a line would be read by NOTHING -- the
    // sweep would keep the shipped two hours, find no candidate, and this test
    // would wait for a close that cannot come. Patching the copied config is
    // what an `override_params` entry does to a staged one.
    patch(root, "main/talky/session-keeper/close/config.json", |v| {
        v["params"]["idle_ms"] = json!(0);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    tool: mpsc::Receiver<Message>,
    recall: mpsc::Receiver<Message>,
    /// The drain of everything this file does not assert on (the write lanes,
    /// the error lane, the prune report). Held rather than dropped: a capture
    /// whose receiver is gone turns every delivery into a send error.
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (tool_tx, tool_rx) = mpsc::channel::<Message>(64);
    let (recall_tx, recall_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/tool_port"), move || {
        CaptureCell::new(tool_tx.clone())
    })
    .await;
    h.spawn(Path::new("/recall_port"), move || {
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
    (
        h,
        Ports {
            sink: sink_rx,
            tool: tool_rx,
            recall: recall_rx,
            _park: park_rx,
        },
    )
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(200)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The negative half of #55, and it is only ever asked AFTER a positive receipt
/// of the same fan-out has arrived: the dispatcher decides both edges in one
/// pass, so a call that reached the collector internally has already had its
/// chance to leave. Two seconds past that point is a settled tree, not a race.
async fn nothing_on(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .ok()
        .flatten()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// `thread_recall` (GH #245) is served inside, and its answer re-enters the
/// running round: the receipt is the SECOND provider call, whose conversation
/// carries the tool result under the original call id. Nothing about that is
/// possible unless the call reached `in_thread_call`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_recall_is_served_inside_and_never_leaves_on_the_tool_lane() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("call-t1", "thread_recall", r#"{"query":"berlin"}"#)]),
        canned_chat_completion("Here is what the turn held.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(turn("what did that tool actually return?")).await;

    let answer = recv_bounded(&mut ports.sink)
        .await
        .expect("the round closed, so the served call answered the fan-in");
    assert_eq!(hop_of(&answer, "route"), "answer");
    assert_eq!(
        hop_of(&answer, "iter"),
        "1",
        "the round re-entered the seam: {:?}",
        answer.headers.hop
    );

    // The receipt: the second call's conversation carries a tool RESULT under
    // the original call id. Nothing in this tree can produce one but the
    // collector — the only tool cell the parent wired answers to `weather`, and
    // the composite's tool lane never carried this call at all.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one round: ask for the tool, then answer");
    let wire = reqs[1].messages().expect("wire messages");
    let result = wire
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "call-t1")
        .unwrap_or_else(|| {
            panic!(
                "no tool result for the served call reached the brain: {}",
                meclaw_core::serde_json::to_string(wire).unwrap_or_default()
            )
        });
    let text = result["content"].as_str().unwrap_or_default();
    // Both of `thread_payload`'s answers are its own vocabulary and nothing
    // else in the tree writes either: the bracketed slate header when the turn
    // held a matching row, the "found nothing" sentence when the select ran
    // before the assistant turn was filed. Which of the two arrives is a race
    // this test has no reason to pin — that the ANSWER came from the collector
    // is the claim.
    assert!(
        text.starts_with('[') || text.starts_with("thread recall"),
        "the result is `thread_payload`'s own answer, not a stand-in tool's: {text:?}"
    );

    let leaked = nothing_on(&mut ports.tool).await;
    assert!(
        leaked.is_none(),
        "a reserved tool name must not also leave on the tool lane: {:?}",
        leaked.map(|m| m.headers.hop)
    );

    h.shutdown().await;
}

/// The positive control for the guarded default edge (GH #283). An ordinary
/// tool name fires no regular out-edge of the dispatcher, so the default
/// carries it outward exactly as the unconditional edge used to — which is
/// what makes the silence in the test above a claim about the one reserved
/// name and not about a dead lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_tool_call_still_leaves_on_the_tool_lane() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("call-w1", "weather", r#"{"city":"Berlin"}"#)]),
        canned_chat_completion("It is 21 degrees in Berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(turn("what is the weather in berlin?")).await;

    let call = recv_bounded(&mut ports.tool)
        .await
        .expect("an ordinary tool call leaves the composite");
    assert_eq!(hop_of(&call, "route"), "tool");
    assert_eq!(hop_of(&call, "tool_name"), "weather");
    assert_eq!(hop_of(&call, "tool_call_id"), "call-w1");

    let answer = recv_bounded(&mut ports.sink).await.expect("the answer");
    assert_eq!(hop_of(&answer, "route"), "answer");
    assert_eq!(hop_of(&answer, "iter"), "1", "the tool round re-entered");

    h.shutdown().await;
}

/// The second positive control, and it is a claim of GH #552 rather than a
/// control: `memory_recall` is an ORDINARY tool name here now. It fires no
/// regular out-edge of the dispatcher any more, so the guarded default carries it
/// out of the composite — to the member's memory, which declares the schema and
/// answers the call. Before `talky@5.0.0` this exact message stayed inside and
/// was answered against a schema this template had typed by hand.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_recall_call_leaves_on_the_tool_lane_like_any_other() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![(
            "call-m1",
            "memory_recall",
            r#"{"query":"what did we say about berlin","window_from":"2026-01-01","window_to":"2026-02-01"}"#,
        )]),
        canned_chat_completion("Nothing on file.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut ports) = boot(&td).await;

    h.send(turn("what did we say about berlin?")).await;

    let call = recv_bounded(&mut ports.tool)
        .await
        .expect("the memory call leaves the composite on the tool lane");
    assert_eq!(hop_of(&call, "route"), "tool");
    assert_eq!(
        hop_of(&call, "tool_name"),
        "memory_recall",
        "the dispatcher names the tool and an edge OUTSIDE this composite knows \
         the cell — which is the whole of GH #552: {:?}",
        call.headers.hop
    );
    assert_eq!(hop_of(&call, "tool_call_id"), "call-m1");

    let asked = nothing_on(&mut ports.recall).await;
    assert!(
        asked.is_none(),
        "and nothing was asked on the collector's own recall port: a deliberate \
         call is not the ambient leg: {:?}",
        asked.map(|m| m.headers.hop)
    );

    h.shutdown().await;
}
