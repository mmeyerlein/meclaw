//! meclaw-os -- GH #273: a session closed by the SWEEP still carries the round
//! it belonged to, all the way into the episode row.
//!
//! `talky_composite.rs` pins the same property at the write port, which is
//! where the composite's promise ends. This file spends the extra tree to ask
//! the question the issue actually asks: does the row LAND? So it boots the
//! shipped `talky`, the shipped `memory-drain` and the pinned write path of the
//! memory hive (the same fixture `memory_drain_colony.rs` uses, GH #125), lets
//! a conversation happen, and then ends it the only way the template ever ends
//! one -- a sweep.
//!
//! The sweep is the whole point. It is a firing, not a turn: it carries no
//! `context`, so nothing on the close path can know the room or the round from
//! the message it is handling. Everything the episode row says about its round
//! has to come off the generation row the keeper wrote when the CONVERSATION
//! opened it.
//!
//! Since #269 a batch without a round is refused at the drain's own door rather
//! than half-written, so the failure mode this pin guards against is visible
//! either way -- but visible is not landed. Nothing here is defaulted: the row
//! is asserted against the two named identities the ingress edge declared, and
//! `["*"]` fails it.

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

/// A fixed schedule id. `${uuid7:*}` is an INSTANTIATION substitution, so a
/// tree written straight to disk carries a real one.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c1273";
/// Never during a test run: the night is triggered here by the operator lane.
const NEVER: &str = "0 0 0 1 1 *";

/// The round this conversation is spoken in: one person and one agent, in the
/// affinity vocabulary the audience gate speaks (ADR-0002 E8).
const AUDIENCE: &str = r#"["member:alex","agent:scribe"]"#;
/// The same set as a CEL string literal, for the edge that declares it.
const AUDIENCE_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;
/// The room, stamped by the surface and never parsed out of a session id.
const ROOM: &str = "c-42";

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
            "purpose": "Test stand-in around the talky composite.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The surface: turns a harness message into the ingress lane and names the
/// room. A real connector does exactly this much.
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

/// The port wiring a parent draws: the ingress that DECLARES the round, the
/// reply exit, the write exit into the drain, the drain's two lanes.
///
/// The round is declared exactly once, at the door a turn enters by, and
/// nowhere else on the path. No edge below it re-states it, so a row that shows
/// the right set shows it because the value travelled -- not because a second
/// place invented the same literal.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": AUDIENCE_CEL}}},
        // The operator lane that stands in for the night timer: same cell, same
        // pass, and just as contextless.
        {"from": "./surface", "to": "./talky/session-keeper",
         "condition": "has(hop.route) && hop.route == 'sweep'",
         "modifier": {"set_hop": {"route": "'in_sweep'"}}},
        {"from": "./talky/collector", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer' \
          && !has(hop.round_capped) && !has(hop.degraded)"},
        {"from": "./talky/collector", "to": "./drain",
         "condition": "has(hop.route) && hop.route == 'write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id"}}},
        {"from": "./drain", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'episode'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at"}}},
        // The refusal lane is read, not dead-lettered: a batch this adapter
        // would not take has to be visible to the assertions below.
        {"from": "./drain", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./talky/errors", "to": "/park",
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

/// The forced sweep: the keeper's `in_sweep` lane, contextless like a firing.
fn sweep() -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("sweep"));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": []})))
        .hop(hop)
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

/// The pin of GH #273. One conversation, ended by a sweep, drained: the episode
/// rows land with the ROOM and the ROUND of the conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_day_the_sweep_closed_lands_with_the_round_it_was_spoken_in() {
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

    // KEEPER_IDLE_MS=0 makes the generation that just spoke a candidate, so one
    // sweep seals it. From here on nothing in flight has heard of this
    // conversation: the sweep read a row and the drain reads a batch.
    h.send(sweep()).await;

    // Two turns -- what the user said and what the agent answered.
    let rows = await_episodes(&db, 2).await;
    assert_eq!(rows.len(), 2, "the whole day landed: {rows:?}");

    // The writer normalises a participant set the way affinity does
    // (deduplicated, blanks dropped, sorted) and re-serialises it, so the
    // stored form is the sorted one -- the SAME two participants.
    let stored = r#"["agent:scribe", "member:alex"]"#;
    for (audience_set, channel) in &rows {
        assert_eq!(
            audience_set, stored,
            "the episode carries the round the conversation was spoken to, \
             not the sweep's and not an empty one: {rows:?}"
        );
        assert_eq!(channel, ROOM, "and the room it was said in: {rows:?}");
    }
    assert!(
        !AUDIENCE.contains('*') && !stored.contains('*'),
        "nothing here is universal"
    );

    h.shutdown().await;
}

/// GH #311 — the RECIPE a caller wires from names every key this chain needs.
///
/// The test above wires the close edge itself, from `main_config()`, and it has
/// always promoted all three keys. A caller does not have this file; they have
/// `templates/session-keeper/template.json`, whose `PORTS` slot named
/// `hop.session_id` and `hop.channel` and stopped there — one key short of
/// `contract.emits[close]`, of the template's own README, and of what the drain
/// enforces. Wiring from that slot closes generations that `memory-drain`
/// refuses with `missing_audience` (`memory_drain_colony.rs`, the refusal is
/// pinned there), and the refusal is silent to the person who wrote the edge:
/// the keeper emitted correctly, the drain rejected correctly, and the recipe
/// between them was wrong.
///
/// So the recipe is held against the contract rather than against a list here:
/// every `hop.<key>` the close port's own `because` names must appear in the
/// slot a caller copies from.
#[test]
fn the_close_port_recipe_names_every_key_the_contract_emits() {
    let read = |rel: &str| -> Value {
        let raw = std::fs::read_to_string(repo(rel)).unwrap();
        meclaw_core::serde_json::from_str(&raw).unwrap()
    };
    let config = read("templates/session-keeper/config.json");
    let close = config["params"]["contract"]["emits"]
        .as_array()
        .expect("the close port is declared")
        .iter()
        .find(|e| e["route"] == "close")
        .expect("contract.emits names the close route")["because"]
        .as_str()
        .unwrap()
        .to_string();

    let template = read("templates/session-keeper/template.json");
    let slot = template["description"]["examples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|e| e.trim_start().starts_with("PORTS"))
        .expect("the template has a PORTS slot");

    // The keys the contract names, read off the contract's own sentence.
    let mut keys: Vec<&str> = close
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .filter(|t| t.starts_with("hop."))
        .collect();
    keys.sort();
    keys.dedup();
    assert!(
        keys.len() >= 3,
        "the close contract stopped naming its keys: {close}"
    );
    for key in keys {
        assert!(
            slot.contains(key),
            "the PORTS slot of session-keeper does not name '{key}', which \
             contract.emits[close] does. A caller wiring from the slot promotes \
             one key too few and the drain refuses the batch (GH #311/#273).\n\
             slot: {slot}"
        );
    }
}
