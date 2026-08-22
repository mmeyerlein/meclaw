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
//!
//! The byte-identity pin over the four sub-unit copies retired with the copies
//! themselves (GH #277): `talky` references its sub-units now, so there is
//! nothing left to drift. Its successor is
//! `meclaw-colony/tests/gh277_composite_instantiation_is_byte_identical.rs`,
//! test `a_cell_inside_talky_is_stamped_with_its_own_template_and_names_talky_above_it`.

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
/// REFERENCE, not a cell -- the referenced template's tree belongs in its
/// place. `talky` names its four sub-units that way, so a tree copied straight
/// off the library follows the same hop the substrate's staging path follows.
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

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION-side substitution, so
/// a tree written straight to disk carries a real one.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1120";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

/// The round these turns are spoken in: one person and one agent, in the
/// affinity vocabulary the audience gate speaks (ADR-0002 E8). Never `["*"]` --
/// a universal set would let the pin below pass over a path that had lost the
/// real one.
const AUDIENCE: &str = r#"["member:alex","agent:scribe"]"#;
/// The same set as a CEL string literal, for the ingress edge that declares it.
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
/// it reports the SIZE of the batch it received, and the provenance of the
/// round the batch was spoken in -- which is what a real consumer of this port
/// (a `memory-drain`) reads off `context` and refuses a batch without.
const ARCHIVE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
ctx = ((envelope.get("header") or {}).get("context") or {})
msgs = d.get("messages", [])
sys.stdout.write(json.dumps({"header": {"route": "archived"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "batch|session=%s|turns=%d|rounds=%d|channel=%s|audience=%s" % (
                                               hop.get("session_id", ""), len(msgs),
                                               len(d.get("rounds", []) or []),
                                               ctx.get("channel", ""),
                                               ctx.get("audience_set", ""))}]}))
"#;

/// The port wiring a parent draws around the composite: ONE ingress, ONE reply
/// exit, ONE write exit, ONE error drain -- plus the per-instance tool lanes.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        // ingress: the surface turn, with the channel promotion the keeper needs
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        // the operator lane of the keeper (a forced sweep)
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'sweep'",
         "modifier": {"set_hop": {"route": "'in_sweep'"}}},
        // the other operator lane (GH #312): a window prune, sent at the
        // composite's OWN path and on its own lane, the way the README wires it
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'prune'",
         "modifier": {"set_hop": {"route": "'in_prune'"}}},
        // ... and the report it answers with, taken off the same path
        {"from": "./talky", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'prune'"},
        // reply exit
        {"from": "./talky/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer' \
          && !has(hop.round_capped) && !has(hop.degraded)"},
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
            &["turn", "sweep", "prune"],
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
    // The age gate of the prune lane, opened all the way: per-INSTANCE tuning,
    // the same knob a live parent sets, not a behaviour patch. The shipped week
    // would put every prune of a test run behind the gate.
    patch(root, "main/talky/collector/assemble/config.json", |v| {
        v["params"]["prune_after_ms"] = json!(0);
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

/// An operator lane, addressed by the route the surface stamps on it: `sweep`
/// for the keeper's forced close, `prune` for the window's age cut.
fn operator(route: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
        .ttl(200)
        .build()
}

/// The forced sweep an operator sends: the keeper's `in_sweep` lane.
fn sweep() -> Message {
    operator("sweep")
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

/// The next message on this drain that travels the named lane. One drain takes
/// several lanes here (the write batch, the error report, the prune report), so
/// a test that wants one of them says which and lets the others pass.
async fn recv_lane(rx: &mut mpsc::Receiver<Message>, route: &str) -> Option<Message> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while let Ok(Some(m)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        if hop_of(&m, "route") == route {
            return Some(m);
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════ pins

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

/// GH #273 -- the pin. A generation that the SWEEP closed reaches the write
/// port with the room and the participant set of the conversation it belonged
/// to: not those of the sweep, and not empty.
///
/// The sweep is a timer firing (or the operator lane that stands in for one
/// here). It carries no `context` at all -- an `emit_to` message is minted, not
/// routed -- so everything the write batch says about its round has to come off
/// the generation row the keeper wrote when the CONVERSATION opened it. The
/// close request carries both keys on its hop, and the composite's close edge
/// promotes them into `context`, where a `memory-drain` reads them.
///
/// This cannot go green through a default: the room is asserted against the
/// channel the surface actually stamped (`c-42`), and the participant set
/// against the two named identities the ingress edge declared. A `["*"]` or an
/// empty set fails both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swept_close_reaches_the_write_port_with_the_round_it_belonged_to() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("The user lives in Berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    h.send(turn("remember: my city is berlin")).await;
    recv_bounded(&mut sink_rx).await.expect("the answer");

    // KEEPER_IDLE_MS=0 makes the generation that just spoke a candidate, so one
    // sweep seals it -- and the sweep knows nothing but the row it read.
    h.send(sweep()).await;
    let archived = recv_bounded(&mut park_rx).await.expect("the write batch");
    let text = answer_text(&archived);

    assert!(
        text.contains("|channel=c-42|"),
        "the batch names the room the conversation was held in, not the \
         sweep's: {text}"
    );
    assert!(
        text.contains(&format!("|audience={AUDIENCE}")),
        "the batch names the round the conversation was spoken to: {text}"
    );
    assert!(!text.contains('*'), "and it is never universalised: {text}");

    h.shutdown().await;
}

/// GH #312 -- the pin. The composite accepts `in_prune`, and the report the
/// prune answers with LEAVES it.
///
/// `in_prune` was accepted (`contract.accepts`) and forwarded to the collector,
/// and the collector's report is unconditional -- the zero case says "pruned
/// nothing" in so many words. But no edge took a `prune` hop off the collector,
/// and no `emits` entry named the lane: every prune request, including the one
/// that found nothing to cut, paid exactly one `no_route` dead letter for its
/// answer. The deletions ran; only the report was lost, which is the shape of
/// defect nobody notices until they need the number.
///
/// Both cases travel here, because they are two different emissions of the same
/// lane: the zero report (no ledger evidence yet) and a real cut (a session
/// batched and behind the age gate). A fix that only routes one of them is not
/// a fix -- the report an operator most needs is the one that says nothing was
/// eligible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prune_report_leaves_the_composite_on_its_own_lane() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("The user lives in Berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;

    // Case one: nothing has ever been batched, so the age gate has no evidence
    // to work on. The collector still answers -- and the answer has to arrive.
    h.send(operator("prune")).await;
    let empty = recv_lane(&mut park_rx, "prune")
        .await
        .expect("the zero report leaves the composite");
    assert_eq!(
        hop_of(&empty, "pruned_turns"),
        "0",
        "nothing was eligible: {:?}",
        empty.headers.hop
    );
    assert_eq!(hop_of(&empty, "pruned_rounds"), "0");
    assert!(
        answer_text(&empty).contains("pruned nothing"),
        "the zero report says so in words: {}",
        answer_text(&empty)
    );

    // Case two: one turn, one close -- now the ledger carries a batched session
    // and, with the gate open, it is behind it.
    h.send(turn("remember: my city is berlin")).await;
    recv_bounded(&mut sink_rx).await.expect("the answer");
    h.send(sweep()).await;
    recv_lane(&mut park_rx, "archived")
        .await
        .expect("the write batch");
    // Settle: the batch leaves the collector before the ledger row it writes is
    // acknowledged. Generous on purpose (30 s convention), not a discriminator.
    tokio::time::sleep(Duration::from_millis(500)).await;

    h.send(operator("prune")).await;
    let cut = recv_lane(&mut park_rx, "prune")
        .await
        .expect("the report of a real cut leaves the composite");
    let turns: i64 = hop_of(&cut, "pruned_turns").parse().unwrap_or(-1);
    assert!(
        turns > 0,
        "the batched session was cut and the report says by how much: {:?}",
        cut.headers.hop
    );
    assert!(
        hop_of(&cut, "session_id").starts_with("c-42-"),
        "and which session it cut: {:?}",
        cut.headers.hop
    );

    h.shutdown().await;
}

/// The other half of GH #312, and the half a runtime test cannot see: the lane
/// is DECLARED, and declared as a pair.
///
/// A door without an `emits` entry is a lane a caller can only find by reading
/// the composite's inside, which is the practice the hive contract exists to
/// end. And an operator lane whose report nobody takes is the defect this issue
/// is about, one wiring later -- so the composite pairs the two and lets the
/// substrate refuse the half-wiring. That the pairing BITES is proven against
/// the real checker in `gh202_shipped_drain_requirements`; what is asserted
/// here is that talky states it at all.
#[test]
fn the_prune_lane_is_declared_and_paired_with_the_ingress_that_opens_it() {
    let raw = std::fs::read_to_string(templates_root().join("talky/config.json")).unwrap();
    let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    let emits = v["params"]["contract"]["emits"].as_array().unwrap();
    assert!(
        emits
            .iter()
            .any(|l| l["route"] == "prune" && l["because"].as_str().unwrap_or_default().len() > 20),
        "the composite emits a prune report and has to say so: {emits:#?}"
    );
    let drains = v["params"]["required_drains"]
        .as_array()
        .expect("talky declares required_drains");
    assert!(
        drains
            .iter()
            .any(|d| d["accepts"] == "in_prune" && d["emits"] == "prune"),
        "sending the prune lane in obliges the caller to take the report back \
         out: {drains:#?}"
    );
}
