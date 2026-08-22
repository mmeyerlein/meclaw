//! meclaw-os -- per-turn episodes in a running colony (wave 9, track A).
//!
//! The script pins live in `w9a_per_turn_episodes.rs`. This file asks the
//! question the track is actually about, and it asks it of a COLONY that
//! contains the shipped `talky`, the shipped `memory-drain` and the memory
//! hive's real write path (writer + `episodes` surface, the pinned snapshots of
//! GH #125, drift-locked by `x2_hive_fixture_drift.rs`):
//!
//!   surface -> talky (keeper, collector, brain) -> route turn_write -> drain
//!           -> the hive's turn-write port -> episodes
//!
//! Two claims, and they are the whole track:
//!
//! 1. FRESHNESS -- the turn and the answer are `episodes` rows while the
//!    session is still open. Until now the first row appeared at the night
//!    close, which is a freshness hole of up to 24 hours and a north-star
//!    breach ("no question may be answered wrongly").
//! 2. THE NET STAYS -- the close batch runs into the SAME drain afterwards and
//!    writes NOTHING, because the ledger recognises its own ids. The count gate
//!    is the completeness proof: `turn_count` of the close batch equals the
//!    episodes the per-turn lane already wrote, so a drain that writes zero is
//!    the success signal.
//!
//! The write path is model-free by spec; the only provider calls in this tree
//! are the talky's own brain and summarizer, and both talk to the mock wire.

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
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The pinned hive surface (GH #125), shared with `memory_drain_colony.rs`.
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
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1121";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

/// The surface a parent draws: it promotes the channel the keeper mints its
/// session id from.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "turn")
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-9a"},
                             "messages": d.get("messages", [])}))
"#;

/// The two lanes this test does not measure (the hive's extraction queue and
/// the store's episode echo) plus the talky's error drain. Terminal on
/// purpose, so the DLQ assertion keeps meaning.
const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
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
            "purpose": "Test stand-in around the per-turn episode colony test.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The memory hive, reduced to the two cells the WRITE path consists of and
/// wired with the hive's own edge between them -- the same projection
/// `memory_drain_colony.rs` uses, out of the same snapshots.
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

/// The wiring a parent draws for W-A. Two lanes reach the SAME drain entry:
/// the per-turn one (`turn_write`, new) and the close one (`write`, the net).
/// One minter, one ledger -- that is the whole reason the seam sits here and
/// not on a second edge into the hive.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id"}}},
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'sweep'",
         "modifier": {"set_hop": {"route": "'in_sweep'"}}},
        // THE per-turn lane.
        // Every end names a HIVE, never a cell inside one (overview § Die
        // Hive-Grenze): both `talky` and `drain` declare their lanes and their
        // doors, and reaching past those doors delivers twice — once to the
        // cell and once to a hive path this graph has no exit for.
        // The batch edges, and the edges that owe the batch its provenance
        // (#244/#269). `audience_set` says who was in the round and
        // `speaker`/`agent_id` who said it; `channel` is not set here -- it
        // travels from the connector seam above (`hop.chat_id` ->
        // `context.channel`), exactly as it does in a real colony. This colony
        // is one person talking to one agent in one room, so the round is those
        // two and nothing wider. A `["*"]` here would make the suite blind to
        // the very leak the gate exists to stop.
        //
        // They are declared HERE, at the door the batch enters by, and not on
        // the port edge one hop later: the drain refuses a batch whose round it
        // does not know, because a batch it consumed and could not deliver
        // would be marked drained and lost (#269).
        {"from": "./talky", "to": "./drain",
         "condition": "has(hop.route) && hop.route == 'turn_write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'",
                                      "speaker": "'member:user'",
                                      "agent_id": "'agent:assistant'"}}},
        // The safety net, unchanged: the close batch into the same entry.
        {"from": "./talky", "to": "./drain",
         "condition": "has(hop.route) && hop.route == 'write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'",
                                      "speaker": "'member:user'",
                                      "agent_id": "'agent:assistant'"}}},
        // The port edge carries the four keys the drain documents; the
        // provenance simply rides along in `context` from the door above.
        {"from": "./drain", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'episode'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at"}}},
        // The drain's refusal lane, drained so a mis-wired batch is read
        // rather than dead-lettered.
        {"from": "./drain", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./drain/ledger", "to": "/park"},
        {"from": "./talky", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'error'"},
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
        "main/void/config.json",
        &code_cell(VOID, &[], json!({})),
    );
    copy_cells(&repo("templates/talky"), &root.join("main/talky"));
    copy_cells(&repo("templates/memory-drain"), &root.join("main/drain"));
    memory_write_path(root);

    // The per-turn lane is off by default; a parent that wires it says so HERE,
    // in the instance's own params. A colony-global `.env` key is not the
    // mechanism any more (`collector@1.2.0`).
    patch(root, "main/talky/collector/assemble/config.json", |v| {
        v["params"]["turn_write"] = json!("1");
    });
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

/// The forced sweep an operator sends: the keeper's `in_sweep` lane, which
/// closes the generation and produces the close batch.
fn sweep() -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("sweep"));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
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

/// One episodes row, as the hive's writer left it.
#[derive(Debug, PartialEq, Eq)]
struct Episode {
    turn_id: String,
    sender: String,
    content: String,
}

fn episodes(db: &std::path::Path) -> Vec<Episode> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare("SELECT turn_id, sender, content FROM episodes") else {
        return vec![];
    };
    let rows = stmt
        .query_map([], |r| {
            Ok(Episode {
                turn_id: r.get(0)?,
                sender: r.get(1)?,
                content: r.get(2)?,
            })
        })
        .expect("query episodes");
    let mut out: Vec<Episode> = rows.map(|r| r.expect("row")).collect();
    out.sort_by_key(|e| index_of(&e.turn_id));
    out
}

/// `"<session>#<index>"` -> index. A row whose id does not carry one sorts
/// last, loudly.
fn index_of(turn_id: &str) -> i64 {
    turn_id
        .rsplit_once('#')
        .and_then(|(_, i)| i.parse::<i64>().ok())
        .unwrap_or(i64::MAX)
}

/// Polls the hive store's own `cell.db` until it holds `n` episodes.
/// 30 s failure marker, robust under cargo parallel load.
fn await_episodes(db: &std::path::Path, n: usize) -> Vec<Episode> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = episodes(db);
        if rows.len() >= n {
            return rows;
        }
        if std::time::Instant::now() > deadline {
            panic!("{} of {n} episodes after 30s: {rows:?}", rows.len());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for `n` ledger replies -- the positive receipt a drain chain leaves
/// behind. A chain that drains something is insert/select/insert; a chain with
/// nothing to drain stops after the select.
async fn chain_ops(rx: &mut mpsc::Receiver<Message>, n: usize) -> Vec<String> {
    let mut ops = Vec::new();
    while ops.len() < n {
        let m = recv_bounded(rx)
            .await
            .unwrap_or_else(|| panic!("only {} of {n} ledger replies arrived: {ops:?}", ops.len()));
        ops.push(hop_of(&m, "operation"));
    }
    ops
}

fn dlq_count(root: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count")
}

/// The track, end to end. One turn, one answer -- and both are in the memory
/// hive BEFORE anything closes. Then the close runs the same day into the same
/// drain and writes nothing, which is what makes it a net instead of a second
/// memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_is_an_episode_before_the_session_ever_closes() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("The user named their editor.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, mut park_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my editor is helix")).await;
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");
    let session = hop_of(&answer, "session_id");
    assert!(session.starts_with("c-9a-"), "session {session:?}");

    // FRESHNESS: the exchange is memory while the session is still open. Two
    // drain chains ran -- one for the turn, one for the answer.
    let rows = await_episodes(&db, 2);
    assert_eq!(
        rows,
        vec![
            Episode {
                turn_id: format!("{session}#0"),
                sender: "user".to_string(),
                content: "my editor is helix".to_string(),
            },
            Episode {
                turn_id: format!("{session}#1"),
                sender: "assistant".to_string(),
                content: "Noted.".to_string(),
            }
        ],
        "the turn and the answer, each once, under the drain's deterministic ids"
    );

    // Both chains parked, probed and marked: six ledger replies, no more.
    assert_eq!(
        chain_ops(&mut park_rx, 6).await,
        vec!["insert", "select", "insert", "insert", "select", "insert"],
        "one park/probe/mark per turn"
    );

    // THE NET: the close batch travels the same edge into the same drain.
    h.send(sweep()).await;
    assert_eq!(
        chain_ops(&mut park_rx, 2).await,
        vec!["insert", "select"],
        "the close chain parks and probes -- and stops there, with nothing to mark"
    );
    assert_eq!(
        episodes(&db).len(),
        2,
        "the row count did not move: the net caught what was already home"
    );
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");

    h.shutdown().await;
}
