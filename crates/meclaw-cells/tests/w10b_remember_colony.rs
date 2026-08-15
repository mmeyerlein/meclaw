//! meclaw-os W10b -- the `remember` tool in a running colony (wave 10, track B).
//!
//! The script pins live in `w10b_inline_gate.rs`. This file asks the question
//! the track is about, of a colony that carries the shipped `talky@1`, the
//! shipped `memory-drain@1` and the memory hive's REAL write and extraction
//! path (`writer`, `store`, `extract-glue` -- the private templates, which is
//! why this file stays private):
//!
//!   surface -> talky -> brain -> split --tool 'remember'--> memory/extract-glue
//!                            \--route 'answer' (interim)--> the channel
//!                    -> route turn_write -> drain -> memory/writer -> episodes
//!
//! Two claims, and they are the whole track:
//!
//! 1. **A remembered turn becomes a fact candidate on the turn it answered.**
//!    The block names no episode -- it cannot -- and the hive binds it to the
//!    newest `user` episode of the session, the row the per-turn lane minted
//!    under `turn_id = "<session_id>#<index>"`.
//! 2. **Extraction never costs the answer.** A `remember` call with a broken
//!    payload leaves through `inline-reject`, writes NOTHING, and the answer
//!    reaches the channel exactly as it was written. That is guard 1 of the
//!    inline design, and it is the only guarantee that makes putting an
//!    extraction on the answering call defensible at all.
//!
//! **The barrier.** In production the ordering is free: the per-turn episode is
//! a handful of in-process store hops while the answer carrying the `remember`
//! call is a model generation, seconds away. Against a mock wire that answers in
//! a millisecond the two chains are the same length, so this test makes the
//! ordering explicit instead of hoping for it: a `gate` cell holds the tool call
//! until the test has SEEN the episode. It is a test fixture and nothing else --
//! the lane's behaviour when the episode is genuinely missing is the reject, and
//! `w10b_inline_gate.rs` pins that directly.
//!
//! No provider is paid: the only wire in this tree is `MockOpenAI`.

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
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
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

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000c10b0";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "turn")
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-10b"},
                             "messages": d.get("messages", [])}))
"#;

const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// THE BARRIER (see the module note). Holds the tool call until the test has
/// seen the episode it must bind to, then forwards it byte for byte -- header
/// and body, so the port edge downstream sees exactly what the dispatcher
/// emitted. A test fixture, never a template.
const GATE: &str = r#"
import sys, json, os, time
d = json.load(sys.stdin)["body"]
marker = os.environ.get("W10B_MARKER", "")
deadline = time.time() + 25
while marker and not os.path.exists(marker) and time.time() < deadline:
    time.sleep(0.02)
sys.stdout.write(json.dumps([{"header": {"route": "remember"},
                              "messages": d.get("messages", [])}]))
"#;

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code", "message_timeout": 40000},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 30000},
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
            "purpose": "Test stand-in around the remember-tool colony test.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The memory hive reduced to the three cells this track uses, wired with the
/// hive's OWN edges between them (`templates/memory-hive/config.json`). The
/// night lane, the recall lane and the two model cells are simply not
/// instantiated -- an island of three cells is still the hive's write and
/// extraction path, byte for byte.
fn memory_hive(root: &std::path::Path) {
    let edges = json!([
        {"from": "./writer", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'wstore'",
         "modifier": {"set_context": {"store_origin": "'episode'", "mem_phase": "'episode'"}}},
        {"from": "./writer", "to": "./extract-glue",
         "condition": "has(hop.route) && hop.route == 'enqueue'",
         "modifier": {"set_context": {"store_origin": "'extract'", "mem_phase": "'enqueue'"}}},
        {"from": "./store", "to": "./extract-glue",
         "condition": "context.store_origin == 'episode'",
         "modifier": {"set_context": {"mem_phase": "'episode'"}}},
        {"from": "./store", "to": "./extract-glue",
         "condition": "context.store_origin == 'extract'",
         "modifier": {"set_context": {"mem_phase": "context.mem_phase",
                                      "batch_id": "context.batch_id"}}},
        {"from": "./extract-glue", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'xstore'",
         "modifier": {"set_context": {"store_origin": "'extract'", "mem_phase": "hop.phase",
                                      "batch_id": "hop.batch_id"}}}
    ]);
    write(
        root,
        "main/memory/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    for cell in ["writer", "store", "extract-glue"] {
        let dst = root.join(format!("main/memory/{cell}"));
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::copy(
            repo(&format!("templates/memory-hive/{cell}/config.json")),
            dst.join("config.json"),
        )
        .unwrap();
    }
}

/// The wiring a parent draws for W-B: the per-turn lane of wave 9 (unchanged),
/// plus the TWO edges the inline port needs -- the way in and the reject drain.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky/keeper/stamp",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id"}}},
        // Wave 9: the day after every stored turn, into the drain.
        {"from": "./talky/collector/assemble", "to": "./drain/drain",
         "condition": "has(hop.route) && hop.route == 'turn_write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id"}}},
        {"from": "./talky/collector/assemble", "to": "./drain/drain",
         "condition": "has(hop.route) && hop.route == 'write'",
         "modifier": {"set_hop": {"route": "'in_batch'"},
                      "set_context": {"session_id": "hop.session_id"}}},
        {"from": "./drain/drain", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'episode'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at"}}},
        // WAVE 10b, edge 1 of 2: the async tool call into the inline ingress.
        // In production this edge goes straight from `split`; here it takes the
        // test barrier on the way (module note).
        {"from": "./talky/split", "to": "./gate",
         "condition": "has(hop.tool_name) && hop.tool_name == 'remember'"},
        {"from": "./gate", "to": "./memory/extract-glue",
         "condition": "has(hop.route) && hop.route == 'remember'",
         "modifier": {"set_context": {"store_origin": "'inline'", "mem_phase": "'inline'"}}},
        // WAVE 10b, edge 2 of 2: the reject drain. Without it a discarded block
        // is an unrouted dead end, and nobody ever learns the memory was not
        // written (the defect the review found in the running colony).
        {"from": "./memory/extract-glue", "to": "/reject",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./talky/collector/assemble", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./drain/ledger", "to": "./void"},
        {"from": "./talky/errors", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // The two lanes this track does not measure: the batched extractor and
        // the embedding queue. Terminal, so the DLQ assertion keeps meaning.
        {"from": "./memory/extract-glue", "to": "./void",
         "condition": "has(hop.route) && (hop.route == 'extract' || hop.route == 'embed')"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str, marker: &std::path::Path) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n\
             DISPATCHER_ASYNC_TOOLS=remember\nW10B_MARKER={}\n",
            marker.display()
        ),
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
        "main/gate/config.json",
        &code_cell(GATE, &["remember"], json!({})),
    );
    write(
        root,
        "main/void/config.json",
        &code_cell(VOID, &[], json!({})),
    );
    copy_cells(&repo("templates/talky"), &root.join("main/talky"));
    copy_cells(&repo("templates/memory-drain"), &root.join("main/drain"));
    memory_hive(root);

    // The per-turn lane is off by default; a parent that wires it says so HERE,
    // in the instance's own params. A colony-global `.env` key is not the
    // mechanism any more (`collector@1.2.0`, wave 13).
    patch(root, "main/talky/collector/assemble/config.json", |v| {
        v["params"]["turn_write"] = json!("1");
    });
    patch(root, "main/talky/keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    for rel in [
        "main/talky/brain/config.json",
        "main/talky/summary/writer/config.json",
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
    let (reject_tx, reject_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/reject"), move || {
        CaptureCell::new(reject_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, reject_rx)
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

fn text_of(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v["messages"][0]["text"].as_str().unwrap_or("").to_string()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    let Ok(conn) = rusqlite::Connection::open(db) else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(sql) else {
        return vec![];
    };
    let n = stmt.column_count();
    let Ok(mapped) = stmt.query_map([], |r| {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(r.get::<_, Option<String>>(i)?.unwrap_or_default());
        }
        Ok(out)
    }) else {
        return vec![];
    };
    mapped.map(|r| r.expect("row")).collect()
}

/// Polls a query until it returns at least `n` rows. 30 s failure marker,
/// robust under cargo parallel load.
fn await_rows(db: &std::path::Path, sql: &str, n: usize) -> Vec<Vec<String>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let r = rows(db, sql);
        if r.len() >= n {
            return r;
        }
        if std::time::Instant::now() > deadline {
            panic!("{} of {n} rows after 30s for {sql:?}: {r:?}", r.len());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dlq_count(root: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count")
}

const EPISODES: &str = "SELECT id, turn_id, sender FROM episodes";
const FACTS: &str = "SELECT episode_id, subject, predicate, claim, fact_kind, \
                     IFNULL(valid_until,'') FROM facts";

/// Claim 1: one turn, one `remember` call, and the fact hangs on the episode of
/// the turn it answered -- while the answer is already in the channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remembered_turn_is_a_fact_candidate_on_the_turn_it_answered() {
    let mock = MockOpenAI::start(vec![canned_content_and_tool_calls(
        "Noted -- Helix it is.",
        vec![(
            "call-r1",
            "remember",
            r#"{"facts":[{"subject":"user","predicate":"Favorite Editor","claim":"Helix","fact_kind":"world","confidence":90}]}"#,
        )],
    )])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    // THE ANSWER IS FIRST AND UNTOUCHED. It travelled the dispatcher's interim
    // lane, in the same response that carried the tool call -- no second
    // inference, and the extraction is not on its way.
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");
    assert_eq!(text_of(&answer), "Noted -- Helix it is.");
    let session = hop_of(&answer, "session_id");
    assert!(session.starts_with("c-10b-"), "session {session:?}");

    // The per-turn lane of wave 9 has minted the episode. Releasing the barrier
    // here is what makes the ordering explicit (module note).
    let episodes = await_rows(&db, EPISODES, 1);
    let user_turn = episodes
        .iter()
        .find(|r| r[2] == "user")
        .expect("the user turn is an episode");
    assert_eq!(
        user_turn[1],
        format!("{session}#0"),
        "under the drain's own deterministic id"
    );
    std::fs::write(&marker, b"go").unwrap();

    // CLAIM 1: the block named no turn and still landed on the right one.
    let facts = await_rows(&db, FACTS, 1);
    assert_eq!(facts.len(), 1, "one fact, not two: {facts:?}");
    let f = &facts[0];
    assert_eq!(
        f[0], user_turn[0],
        "the fact hangs on the episode of the turn it answered"
    );
    assert_eq!(f[1], "user");
    assert_eq!(
        f[2], "favorite_editor",
        "and it arrives on the KEY form of the axis, not on a fourth spelling"
    );
    assert_eq!(f[3], "Helix");
    assert_eq!(f[4], "world");
    assert_eq!(
        f[5], "",
        "a candidate closes nothing -- that is the night's job"
    );

    // GitHub #52: the turn the block spoke for is out of the batch queue, under
    // the status that says WHO handled it.
    let queue = rows(
        &db,
        "SELECT episode_id, status FROM pending_extraction WHERE status = 'inline'",
    );
    assert_eq!(
        queue,
        vec![vec![user_turn[0].clone(), "inline".to_string()]],
        "the batched lane buys no second opinion on this turn"
    );

    assert!(
        reject_rx.try_recv().is_err(),
        "a good block does not travel the reject lane"
    );
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");
    h.shutdown().await;
}

/// Claim 2: extraction never costs the answer. A broken block leaves through
/// `inline-reject`, writes nothing -- and the channel got its sentence anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_remember_block_costs_the_answer_nothing() {
    let mock = MockOpenAI::start(vec![
        canned_content_and_tool_calls(
            "Got it.",
            vec![("call-r1", "remember", "{\"facts\": [ this is not json")],
        ),
        canned_chat_completion("ignored", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    // THE ANSWER, unchanged and unaffected -- it is on the channel before the
    // extraction has even been routed.
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");
    assert_eq!(text_of(&answer), "Got it.");

    let episodes = await_rows(&db, EPISODES, 1);
    let user_turn = episodes
        .iter()
        .find(|r| r[2] == "user")
        .expect("the user turn is an episode");
    std::fs::write(&marker, b"go").unwrap();

    // THE REJECT LANE FIRES -- a positive receipt, not an absence. This is the
    // edge the running colony was missing: without it the block would be an
    // unrouted dead end and nobody would ever learn the memory was not written.
    let reject = recv_bounded(&mut reject_rx)
        .await
        .expect("a broken block leaves through inline-reject");
    assert_eq!(hop_of(&reject, "route"), "reject");
    assert!(
        text_of(&reject).contains("not JSON"),
        "and it says why: {:?}",
        text_of(&reject)
    );

    // NOTHING in the store: no fact, and the turn is still the batch lane's.
    assert!(
        rows(&db, FACTS).is_empty(),
        "a rejected block writes no fact"
    );
    let pending = rows(
        &db,
        "SELECT status FROM pending_extraction WHERE status = 'inline'",
    );
    assert!(
        pending.is_empty(),
        "and covers no turn: the batch lane keeps {user_turn:?}"
    );
    assert_eq!(
        dlq_count(td.path()),
        0,
        "the reject is drained, not dead-lettered"
    );
    h.shutdown().await;
}
