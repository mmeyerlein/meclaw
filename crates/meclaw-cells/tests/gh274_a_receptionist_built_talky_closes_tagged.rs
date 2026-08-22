//! meclaw-os -- GH #274: a talky the RECEPTION built closes with the round it
//! was spoken in, all the way into the episode row.
//!
//! `gh273_a_swept_close_reaches_the_memory.rs` pins the same property for a
//! talky a parent wired by hand. This file asks it of the shape that produces
//! most of the generations a colony will ever have: one agent per channel,
//! built on demand. The reception draws the ingress edge into the talky it
//! instantiates -- so whatever that edge declares is the ONLY thing the
//! generation behind it will ever know about the room and the round.
//!
//! The sweep is the whole point, and here it is the real one: no operator lane
//! and no forced firing, just the keeper's own night timer wound down to a
//! second. A firing carries no `context`, so nothing on the close path can know
//! the round from the message it is handling. Everything the episode row says
//! about its round has to come off the generation row the keeper wrote when the
//! CONVERSATION opened it -- which is what makes the reception's edge the whole
//! question.
//!
//! Nothing here is defaulted: the row is asserted against the two named
//! identities the caller's ingress edge declared, and `["*"]` fails it.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The pinned hive surface (GH #125): the memory hive is private, so what
/// travels is the writer byte for byte, its edge byte for byte, and the store
/// projected onto the one table this write path touches.
fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/memory_drain_colony")
        .join(name)
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
        dir = repo("templates").join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

/// A template directory the colony can scan: cells plus the `template.json`.
/// This is where a mutation looks for what it is asked to instantiate.
fn copy_template(name: &str, dst_root: &std::path::Path) {
    let src = repo("templates").join(name);
    let dst = dst_root.join(name);
    copy_cells_verbatim(&src, &dst);
    std::fs::copy(src.join("template.json"), dst.join("template.json")).unwrap();
    // GH #277: the template travels VERBATIM -- a `cell.type: "ref"` sub-unit
    // stays a ref, because resolving it is the substrate's job and that is what
    // this test drives. So every template it names travels next to it, exactly
    // as the shipped library carries them.
    for referenced in refs_in(&dst) {
        if !dst_root.join(&referenced).is_dir() {
            copy_template(&referenced, dst_root);
        }
    }
}

/// `copy_cells` without the ref hop: the tree as the library holds it.
fn copy_cells_verbatim(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells_verbatim(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// Every template name a `cell.type: "ref"` marker under `dir` points at.
fn refs_in(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            out.extend(refs_in(&p));
        } else if entry.file_name() == "config.json" {
            let raw = std::fs::read_to_string(&p).unwrap();
            let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
            if v["cell"]["type"] == "ref" {
                let r = v["cell"]["template"].as_str().expect("a ref names one");
                out.push(r.split('@').next().unwrap_or_default().to_string());
            }
        }
    }
    out
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

/// The round this conversation is spoken in: one person and one agent, in the
/// affinity vocabulary the audience gate speaks (ADR-0002 E8).
const AUDIENCE: &str = r#"["member:alex","agent:scribe"]"#;
/// The same set as a CEL string literal, for the edge that declares it.
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;
/// The room, stamped by the surface and never parsed out of a session id.
const ROOM: &str = "c-42";

/// The night, wound down to a second: this test wants the keeper's OWN sweep,
/// not an operator lane, because the reception draws no sweep edge and a real
/// deployment never gets one either.
const FAST_NIGHT: &str = "*/1 * * * * *";
/// How long a generation has to be silent before a firing may seal it. Long
/// enough that the turn in flight is never swept out from under itself, short
/// enough that the seal happens while the test is still watching.
const IDLE_MS: &str = "3000";

// ────────────────────────────────────────────────────────── the test-only cells

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
            "purpose": "Test stand-in around the reception.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The surface: turns a harness message into the reception's lane and names the
/// room. A real connector does exactly this much.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn", "chat_id": "c-42"},
                             "messages": d.get("messages", [])}))
"#;

/// Terminal drain for the two hive lanes downstream of the writer, which this
/// test does not measure.
const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The memory hive, reduced to the two cells the WRITE path consists of, wired
/// with the hive's own edge between them.
fn memory_write_path(root: &std::path::Path) {
    let edges: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(fixture("hive_writer_store_edge.json")).unwrap(),
    )
    .unwrap();
    write(
        root,
        "main/memory/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    for (cell, snapshot) in [
        ("writer", "hive_writer_config.json"),
        ("store", "hive_store_config.json"),
    ] {
        let dst = root.join(format!("main/memory/{cell}"));
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::copy(fixture(snapshot), dst.join("config.json")).unwrap();
    }
}

/// The bootstrap tree. Two edges touch the reception and the parent draws both:
/// the ingress that DECLARES the room and the round, and the privileged
/// mutation lane no mutation can mint.
///
/// What is deliberately absent is an edge into the talky. There is none to
/// write -- the talky does not exist yet, and the edge that will carry turns
/// into it is drawn by the RECEPTION, in the mutation that creates it. That is
/// the whole question of this file: a round declared here has to survive into an
/// edge nobody in this config wrote.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./reception",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        {"from": "./reception", "to": "/colony/mutations",
         "condition": "has(hop.route) && hop.route == 'mutate'"},
        {"from": "./drain", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'episode'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at"}}},
        // The refusal lane is read, not dead-lettered: a batch this adapter
        // would not take has to be visible to the assertions below.
        {"from": "./drain", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./memory/writer", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'enqueue'"},
        {"from": "./memory/store", "to": "./void",
         "condition": "context.store_origin == 'episode'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\n\
             KEEPER_IDLE_MS={IDLE_MS}\n\
             KEEPER_NIGHT_CRON={FAST_NIGHT}\n\
             RECEPTIONIST_MODEL=gpt-4o-mock\n\
             RECEPTIONIST_REPLY_TO=./sink\n\
             RECEPTIONIST_WRITE_TO=./drain\n\
             RECEPTIONIST_ERROR_TO=./park\n"
        ),
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
    write(
        root,
        "main/void/config.json",
        &code_cell(VOID, &[], json!({})),
    );
    copy_cells(
        &repo("templates/receptionist"),
        &root.join("main/reception"),
    );
    copy_cells(&repo("templates/memory-drain"), &root.join("main/drain"));
    memory_write_path(root);

    // The talky the reception instantiates lives in the TEMPLATE directory,
    // which is where a mutation looks for it.
    copy_template("talky", &root.join("templates"));
    for rel in [
        "templates/talky/brain/config.json",
        "templates/summarizer/writer/config.json",
    ] {
        patch(root, rel, |v| v["params"]["base_url"] = json!(base_url));
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
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack");
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

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The provenance of the episodes as the hive's writer left it:
/// `(audience_set, channel)` per row.
fn provenance(db: &std::path::Path) -> Vec<(String, String)> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(audience_set, ''), COALESCE(channel, '') FROM episodes ORDER BY turn_id",
    ) else {
        return vec![];
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) else {
        return vec![];
    };
    rows.filter_map(|r| r.ok()).collect()
}

/// The modifier of the ingress edge the RECEPTION drew, read straight out of
/// the live colony's edge table. Nobody in the bootstrap tree wrote this edge,
/// so what it declares is entirely the reception's doing.
async fn drawn_ingress_modifier(h: &ColonyHandle) -> Value {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("read graph");
    let reply = ack_rx.await.expect("graph ack");
    let edge = reply
        .edges
        .into_iter()
        .find(|e| e.from == "/reception" && e.to == "/talky-c-42")
        .unwrap_or_else(|| panic!("the reception drew no ingress edge into its talky"));
    edge.modifier.expect("the ingress edge carries a modifier")
}

/// Waits until the hive's store holds `n` episodes. 30 s failure marker.
async fn await_episodes(db: &std::path::Path, n: usize) -> Vec<(String, String)> {
    for _ in 0..300 {
        let rows = provenance(db);
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("only {:?} episodes within 30s", provenance(db));
}

// ═══════════════════════════════════════════════════════════════════════ pin

/// The pin of GH #274. A channel speaks for the first time, the reception
/// builds it a talky, the keeper's own night sweep ends the day -- and the
/// episode rows land with the ROOM and the ROUND of the conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_talky_the_reception_built_closes_with_the_round_it_was_spoken_in() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("The user lives in Berlin.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("remember: my city is berlin")).await;
    recv_bounded(&mut sink_rx).await.expect("the answer");

    // The promise first. The round DOES reach a generation as plain context
    // that nothing on the path happened to delete -- which is the accident
    // #273 refused to build on. What #274 is about is that the edge says so.
    let modifier = drawn_ingress_modifier(&h).await;
    assert_eq!(
        modifier["set_context"]["audience_set"],
        json!("hop.aud"),
        "the edge the reception drew NAMES the round: {modifier}"
    );

    // From here on nothing is sent. The generation falls silent, the keeper's
    // own timer fires, and the seal reads a row nobody in flight has heard of.
    let rows = await_episodes(&db, 1).await;

    // The writer normalises a participant set the way affinity does
    // (deduplicated, blanks dropped, sorted) and re-serialises it, so the
    // stored form is the sorted one -- the SAME two participants.
    let stored = r#"["agent:scribe", "member:alex"]"#;
    for (audience_set, channel) in &rows {
        assert_eq!(
            audience_set, stored,
            "the episode carries the round the conversation was spoken to, not \
             the sweep's and not an empty one: {rows:?}"
        );
        assert_eq!(channel, ROOM, "and the room it was said in: {rows:?}");
    }
    assert!(
        !AUDIENCE.contains('*') && !stored.contains('*'),
        "nothing here is universal"
    );

    h.shutdown().await;
}
