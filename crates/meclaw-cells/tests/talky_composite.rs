//! meclaw-os -- the `talky` composite in a running colony (GH #112).
//!
//! The four sub-templates have their own pins (`session_keeper.rs`,
//! `collector_window.rs` / `collector_colony.rs`, `dispatcher_split.rs`,
//! `summarizer_prep.rs` / `summarizer_colony.rs`). This file asks the only
//! question the composite adds: is the WIRING between them right? So it boots
//! the shipped `talky` tree and drives
//!
//!   turn -> stamp -> seam -> brain(mock) -> split -> fake tool -> seam -> answer
//!
//! and a close through the write path, where the batch leaves on the write
//! port AND enters the summarizer, whose handover reaches the brain's
//! `system.handover` without a provider call.
//!
//! Free of a real provider by construction: both `llm` cells talk to the mock
//! OpenAI wire, and every other cell is a `code`/`store`/`timer` cell that
//! reports what it was given.

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

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION-side substitution, so
/// a tree written straight to disk carries a real one.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1120";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

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

/// The surface: turns a harness message into the ingress lane. A real parent
/// promotes the channel here -- so does this one, which is what makes the
/// keeper mint ONE id per channel.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "turn")
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-42"},
                             "messages": d.get("messages", [])}))
"#;

/// The tool the instance wires on `hop.tool_name` -- outside the composite,
/// exactly as the README says.
const TOOL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "res", "tool_call_id": hop.get("tool_call_id", "")},
    "messages": [{"origin": "tool", "type": "tool_result",
                  "id": hop.get("tool_call_id", ""),
                  "text": "berlin: 21C"}]}))
"#;

/// The write target the instance decides on (a day archive stands in here):
/// it reports the SIZE of the batch it received.
const ARCHIVE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
msgs = d.get("messages", [])
sys.stdout.write(json.dumps({"header": {"route": "archived"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "batch|session=%s|turns=%d|rounds=%d" % (
                                               hop.get("session_id", ""), len(msgs),
                                               len(d.get("rounds", []) or []))}]}))
"#;

/// The port wiring a parent draws around the composite: ONE ingress, ONE reply
/// exit, ONE write exit, ONE error drain -- plus the per-instance tool lanes.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ingress: the surface turn, with the channel promotion the keeper needs
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id"}}},
        // the operator lane of the keeper (a forced sweep)
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'sweep'",
         "modifier": {"set_hop": {"route": "'in_sweep'"}}},
        // reply exit
        {"from": "./talky/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer' && !has(hop.round_capped)"},
        // write exit -- the instance decides the target
        {"from": "./talky/collector", "to": "./archive",
         "condition": "has(hop.route) && hop.route == 'write'"},
        {"from": "./archive", "to": "/park"},
        // error drain
        {"from": "./talky/errors", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // tool lanes: OUTSIDE the composite, keyed on the name
        {"from": "./talky/dispatcher", "to": "./weather",
         "condition": "has(hop.tool_name) && hop.tool_name == 'weather'"},
        {"from": "./weather", "to": "./talky/collector",
         "condition": "has(hop.route) && hop.route == 'res'",
         "modifier": {"set_hop": {"route": "'in_tool'"}}}
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
            &["turn", "sweep"],
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
    write(
        root,
        "main/archive/config.json",
        &code_cell(ARCHIVE, &["archived"], json!({})),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));

    // Two patches, both about the clock and the wire rather than about
    // behaviour: a schedule the test can trigger, and both llm cells pointed at
    // the mock. The shipped `${ctx.model}` is an INSTANTIATION substitution; a
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
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
    (h, sink_rx, park_rx)
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(200)
        .build()
}

/// The forced sweep an operator sends: the keeper's `in_sweep` lane.
fn sweep() -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("sweep"));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
        .ttl(200)
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

fn answer_text(m: &Message) -> String {
    body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The substrate instantiates a template by COPYING its directory, and there
/// is no template-in-template reference to lean on -- so the composite carries
/// materialised copies of the four sub-templates, and this pin is what keeps
/// "carries" from decaying into "forked". Every copied `config.json` must be
/// byte-identical to the sub-template it came from; a sub-template change that
/// does not travel into the composite fails here instead of in production.
#[test]
fn the_sub_unit_copies_are_byte_identical_to_their_templates() {
    let root = templates_root();
    let mut checked = 0usize;
    for (sub, unit) in [
        ("session-keeper", "session-keeper"),
        ("collector", "collector"),
        ("summarizer", "summarizer"),
    ] {
        let src = root.join(sub);
        let dst = root.join("talky").join(unit);
        let mut rels = Vec::new();
        collect_configs(&src, &src, &mut rels);
        assert!(!rels.is_empty(), "{sub}: no config.json found");
        for rel in rels {
            let a = std::fs::read(src.join(&rel)).unwrap();
            let b = std::fs::read(dst.join(&rel)).unwrap_or_else(|e| {
                panic!("talky/{unit}/{} missing ({e}) -- the sub-template grew a cell the composite does not carry", rel.display())
            });
            assert!(
                a == b,
                "talky/{unit}/{} drifted from {sub}/{}",
                rel.display(),
                rel.display()
            );
            checked += 1;
        }
    }
    // The dispatcher is a single-cell template: its root config IS the cell.
    let a = std::fs::read(root.join("dispatcher/config.json")).unwrap();
    let b = std::fs::read(root.join("talky/dispatcher/config.json")).unwrap();
    assert!(a == b, "talky/dispatcher drifted from dispatcher");
    checked += 1;
    assert!(checked >= 10, "the pin swept almost nothing: {checked}");
}

fn collect_configs(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            collect_configs(root, &p, out);
        } else if entry.file_name() == "config.json" {
            out.push(p.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

/// The whole point of the composite: one turn runs through keeper, collector,
/// brain, dispatcher, a tool and back to the seam without a single edge being
/// wired by hand -- the parent drew four ports and one tool lane, the template
/// drew the other twenty-two edges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_runs_the_whole_composite_round() {
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("call-1", "weather", r#"{"city":"Berlin"}"#)]),
        canned_chat_completion("It is 21 degrees in Berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn("what is the weather in berlin?")).await;
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");

    assert_eq!(hop_of(&answer, "route"), "answer");
    assert_eq!(answer_text(&answer), "It is 21 degrees in Berlin.");
    // The stamp happened: the answer carries the generation the keeper minted.
    assert!(
        hop_of(&answer, "session_id").starts_with("c-42-"),
        "the session id of the channel rides through: {:?}",
        answer.headers.hop
    );
    // The loopback was taken: iteration 1 means the round re-entered the seam.
    assert_eq!(hop_of(&answer, "iter"), "1", "the tool round re-entered");

    // Two provider calls, and the second one saw the tool result -- that is the
    // fan-in, the seam and the loopback in one assertion.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one round: ask for the tool, then answer");
    let second = meclaw_core::serde_json::to_string(reqs[1].messages().expect("wire messages"))
        .unwrap_or_default();
    assert!(
        second.contains("berlin: 21C"),
        "the tool result reached the brain: {second}"
    );

    h.shutdown().await;
}

/// The close path: the keeper seals the generation, the collector batches it,
/// and the batch fans out -- to the write port the parent wired AND into the
/// summarizer, whose handover reaches the brain as a `system.handover` update
/// without a third provider call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_fans_the_batch_out_and_hands_the_summary_to_the_brain() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("The user lives in Berlin and said so once.", "stop"),
        canned_chat_completion("Still here.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn("remember: my city is berlin")).await;
    recv_bounded(&mut sink_rx).await.expect("the answer");

    // KEEPER_IDLE_MS=0 makes every open generation a candidate, so one sweep
    // closes the session that just spoke.
    h.send(sweep()).await;
    let archived = recv_bounded(&mut park_rx).await.expect("the write batch");
    let text = answer_text(&archived);
    assert!(
        text.starts_with("batch|session=c-42-"),
        "the write exit carries the whole session: {text}"
    );
    assert!(
        text.contains("|turns=2|"),
        "user turn and answer left as one batch: {text}"
    );

    // The same batch entered the summarizer, and its handover reached the brain
    // WITHOUT a provider call of its own: three calls would mean the update
    // triggered an inference.
    let mut reqs = mock.recorded_requests().await;
    for _ in 0..30 {
        if reqs.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        reqs = mock.recorded_requests().await;
    }
    assert_eq!(
        reqs.len(),
        2,
        "one brain call and one summarizer call -- the handover update is silent"
    );
    let sys = meclaw_core::serde_json::to_string(reqs[1].messages().expect("wire messages"))
        .unwrap_or_default();
    assert!(
        sys.contains("never invent"),
        "the second call is the summarizer's: {sys}"
    );

    // And the brain KEPT it. Settle first: the handover update is one hop
    // behind the summarizer's answer, and only a delivered update can show up
    // in the next prompt. This is a settle wait, not a timing discriminator --
    // generous on purpose (30 s convention).
    for _ in 0..150 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if mock.recorded_requests().await.len() >= 2 {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    h.send(turn("still there?")).await;
    recv_bounded(&mut sink_rx).await.expect("the second answer");
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "the second turn is the third provider call");
    let msgs = reqs[2].messages().expect("wire messages");
    let system = msgs
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system message on the wire");
    let sys_text = system["content"].as_str().unwrap_or_default();
    assert!(
        sys_text.contains("The user lives in Berlin and said so once."),
        "the handover of the closed generation reached the next one: {sys_text}"
    );

    h.shutdown().await;
}
