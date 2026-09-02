//! GH #372 -- a round whose last brain iteration is a lone ASYNC tool call must
//! still end in an answer on the channel.
//!
//! The measured stall (task 19/20 of wave W5, three of five full harness runs):
//! the front model answered a turn with `remember` and **nothing else** -- no
//! sentence beside the call. The dispatcher sends an interim answer only when
//! there is text to send, so nothing left for the channel; the collector filed
//! the assistant row as ALREADY fired (`in_calls`, the async-only branch), so no
//! guard could win it, no `in_round_sweep` could find it open, and `round_idle_ms`
//! never fires on a quiet channel. The colony stayed alive and silent, and the
//! turn was lost.
//!
//! The invariant this file pins is the one the per-turn contract (#298/#299)
//! makes load-bearing, because it makes call-only iterations common:
//!
//!   **a round always ends in an answer** -- a real one, `round_capped`, or
//!   `degraded` (the closed set of GH #343).
//!
//! Two claims, and they are the whole issue:
//!
//! 1. **The round comes back.** An async-only bundle with no text leaves the
//!    round OPEN: the synthetic acknowledgement completes the fan-in, the
//!    regular guard fires, and the seam re-enters the brain with everything the
//!    round collected. The model gets the iteration it never spent, and its
//!    answer reaches the channel. Bounded by `params.max_iter` like every other
//!    round -- nothing new was invented to stop it.
//! 2. **The async call still travels its own lane.** The re-entry costs the
//!    `remember` block nothing: it reaches `memory/extract-glue`, binds to the
//!    episode of the turn it answered and covers that turn in
//!    `pending_extraction`. The two lanes are independent, and this test would
//!    not notice if the fix had bought claim 1 by swallowing the call.
//!
//! The harness is `w10b_remember_colony.rs`, including its barrier: a `gate`
//! cell holds the tool call until the test has seen the episode, because against
//! a mock wire the two chains are the same length and the ordering would
//! otherwise be a race. No provider is paid -- the only wire here is `MockOpenAI`.

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
use mock_openai::{
    MockOpenAI, canned_chat_completion, canned_content_and_tool_calls, canned_tool_calls,
};
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
/// REFERENCE, not a cell -- the referenced template's tree belongs in its place.
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

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000003720";
/// Never during a test run: the shipped default is the real night.
const NEVER: &str = "0 0 0 1 1 *";

const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "turn")
sys.stdout.write(json.dumps({"header": {"route": route, "chat_id": "c-372"},
                             "messages": d.get("messages", [])}))
"#;

const VOID: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The barrier of `w10b_remember_colony.rs`, unchanged: hold the tool call until
/// the test has seen the episode it must bind to, then forward it byte for byte.
///
/// **The marker path is substituted into the script (GH #526).** It used to be
/// read out of the process environment while the key was only written into
/// `{root}/.env` -- and `.env` feeds `${VAR}` substitution in `config.json`
/// values, not the environment a `code` cell's subprocess inherits. The lookup
/// returned `""`, the `while marker and ...` loop never ran once, and the
/// barrier held nothing back. It fails loudly now rather than quietly not
/// existing.
const GATE: &str = r#"
import sys, json, os, time
d = json.load(sys.stdin)["body"]
marker = "GH372_MARKER_PATH"
if not marker or marker.endswith("_MARKER_PATH"):
    raise SystemExit("gate: no marker path was substituted into this script")
deadline = time.time() + 25
while not os.path.exists(marker) and time.time() < deadline:
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
            "purpose": "Test stand-in around the async-only round colony test.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The memory hive reduced to the three cells this test uses, wired with the
/// hive's OWN edges between them (`templates/memory-hive/config.json`).
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

/// The wiring of `w10b_remember_colony.rs`: the per-turn lane, the inline port
/// and its reject drain, the answer sink.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id"}}},
        // THE PER-TURN LANE, straight at the writer port (GH #523). It ran
        // through `memory-drain` until that issue: the adapter's ledger is a
        // per-session high-water mark over ONE closed batch, and a per-turn
        // cadence hands it two batches of one session at once -- the probe then
        // reads the LAST parked batch and the user turn is lost. Ruling Q11 (GH
        // #298) retracted the edge; `w9a_per_turn_colony.rs` and
        // `member@1.5.0` draw the one below.
        {"from": "./talky", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'turn_write'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'",
                                      "speaker": "'member:user'",
                                      "agent_id": "'agent:assistant'"}}},
        // The close route reaches no memory any more (Q11); it is terminated
        // rather than left unrouted, so the DLQ assertions keep meaning.
        {"from": "./talky", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'write'"},
        {"from": "./talky", "to": "./gate",
         "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'remember'"},
        {"from": "./gate", "to": "./memory/extract-glue",
         "condition": "has(hop.route) && hop.route == 'remember'",
         "modifier": {"set_context": {"store_origin": "'inline'", "mem_phase": "'inline'",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'"}}},
        {"from": "./memory/extract-glue", "to": "/reject",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./talky", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // GH #519: since the writer mints an embedding row beside the episode,
        // the SAME lane leaves the writer too. The hive drains both to
        // `./embed` (`templates/memory-hive/config.json`); this island does not
        // instantiate the embedder, so the leg is terminated here rather than
        // left unrouted -- an unrouted leg is a dead letter, and the DLQ
        // assertions are what makes the "nothing was lost" claim mean anything.
        {"from": "./memory/writer", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'embed'"},
        {"from": "./memory/extract-glue", "to": "./void",
         "condition": "has(hop.route) && (hop.route == 'extract' || hop.route == 'embed')"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str, marker: &std::path::Path) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        // No GH372_MARKER here, and that is the repair of GH #526: the
        // barrier's path is substituted into its SCRIPT below, because `.env`
        // feeds `${VAR}` substitution in config values and not the environment
        // a `code` cell's subprocess sees.
        "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\n\
         DISPATCHER_ASYNC_TOOLS=remember\n",
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
        &code_cell(
            &GATE.replace("GH372_MARKER_PATH", &marker.display().to_string()),
            &["remember"],
            json!({}),
        ),
    );
    write(
        root,
        "main/void/config.json",
        &code_cell(VOID, &[], json!({})),
    );
    copy_cells(&repo("templates/talky"), &root.join("main/talky"));
    memory_hive(root);

    patch(root, "main/talky/collector/assemble/config.json", |v| {
        v["params"]["turn_write"] = json!("1");
    });
    patch(root, "main/talky/session-keeper/night/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    // GH #464 -- the second timer of a shipped composite, and the same two
    // patches for the same two reasons: `${uuid7:*}` is an INSTANTIATION
    // substitution and a tree written straight to disk carries a literal, and a
    // menu tick during a test run would ask a tools hive this colony does not
    // have.
    patch(root, "main/talky/collector/menu-clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
        v["params"]["schedules"][0]["cron"] = json!(NEVER);
    });
    patch(root, "main/talky/brain/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
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

/// The episode the annotation BINDS TO -- not "some episode". The lane writes
/// two rows per turn, and under cargo-parallel load the assistant's can be
/// committed first: a wait for one row then returns the wrong one, and the two
/// ways that goes wrong are exactly the two intermittent failures measured on
/// this pair. Either the row is the assistant's and `find(sender == "user")`
/// finds nothing, or -- worse, because it reads like a lane defect -- the
/// barrier is released while the user's row is still unwritten, the ingress
/// finds no turn to bind to and rejects, and the test dies 30 s later on a fact
/// that was never going to arrive. The condition being waited for is therefore
/// the condition being relied on, spelled out in SQL.
const USER_EPISODE: &str = "SELECT id, turn_id, sender FROM episodes \
                            WHERE sender = 'user'";
const FACTS: &str = "SELECT episode_id, subject, predicate, claim FROM facts";

const REMEMBER_ARGS: &str = r#"{"facts":[{"subject":"user","predicate":"Favorite Editor","claim":"Helix","fact_kind":"world","confidence":90}],"nothing_new":false}"#;

/// The whole issue: a bundle that is nothing but an async `remember`, and the
/// channel still gets an answer -- because the round is NOT over when the model
/// has not spoken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_async_only_round_still_answers() {
    let mock = MockOpenAI::start(vec![
        // Iteration 0: the shape that killed three harness runs -- `content:
        // null`, one async tool call, `finish_reason: tool_calls`. The
        // dispatcher has no sentence to send as an interim answer.
        canned_tool_calls(vec![("call-r1", "remember", REMEMBER_ARGS)]),
        // Iteration 1: the iteration the round never used to reach. The model
        // sees its own call and the acknowledgement, and answers.
        canned_chat_completion("Noted -- Helix it is.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    // The per-turn lane mints the episode the inline block will bind to; the
    // barrier is released once it exists (see the module note).
    let user_rows = await_rows(&db, USER_EPISODE, 1);
    let user_turn = &user_rows[0];
    std::fs::write(&marker, b"go").unwrap();

    // CLAIM 1 -- the round comes back and the channel is answered. Before the
    // fix this receive returned nothing at all: the assistant row was filed as
    // already fired, no guard could win it and the colony fell silent.
    let answer = recv_bounded(&mut sink_rx)
        .await
        .expect("an async-only round must still end in an answer on the channel");
    assert_eq!(
        text_of(&answer),
        "Noted -- Helix it is.",
        "the second iteration's reply, not a stand-in"
    );
    // And it is a plain answer of the closed set (GH #343): the round was not
    // capped and the store refused nothing. The three sorts stay the vocabulary.
    assert_ne!(hop_of(&answer, "round_capped"), "1", "the round had budget");
    assert_ne!(hop_of(&answer, "degraded"), "1", "no store refusal here");
    assert_ne!(
        hop_of(&answer, "interim"),
        "1",
        "an async-only bundle has no interim to send -- this is the real answer"
    );
    let session = hop_of(&answer, "session_id");
    assert!(session.starts_with("c-372-"), "session {session:?}");

    // CLAIM 2 -- the re-entry costs the async call nothing: the block still
    // travelled its own lane, bound to the turn it answered and covered it.
    let facts = await_rows(&db, FACTS, 1);
    assert_eq!(facts.len(), 1, "one fact, not two: {facts:?}");
    let f = &facts[0];
    assert_eq!(
        f[0], user_turn[0],
        "the fact hangs on the episode of the turn it answered"
    );
    assert_eq!(f[1], "user");
    assert_eq!(f[2], "favorite_editor");
    assert_eq!(f[3], "Helix");
    let queue = rows(
        &db,
        "SELECT episode_id, status FROM pending_extraction WHERE status = 'inline'",
    );
    assert_eq!(
        queue,
        vec![vec![user_turn[0].clone(), "inline".to_string()]],
        "the inline pass covers the turn it spoke for"
    );

    assert!(
        reject_rx.try_recv().is_err(),
        "a good block does not travel the reject lane"
    );
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");
    h.shutdown().await;
}

/// GH #378 -- the sibling case: prose and an async call in ONE completion.
///
/// #372 above is the async-call-**without**-text round. This is the
/// async-call-**with**-text round, and it stranded every turn that used it: the
/// dispatcher marked the prose `interim` unconditionally, while the collector
/// -- correctly -- filed the round as over, because nothing is being waited for.
/// The turn therefore ended with an interim answer and no final one, forever.
/// Measured under the shipped contract at 10 of 12 rounds, because that mixed
/// shape is exactly what an async tool description asks a model to produce.
///
/// The rule the fix restores is the dispatcher's own, already written in its
/// header: an async non-handoff call is fire-and-forget and "the model still
/// owes THIS turn an answer". If every call in the bundle is one of those, the
/// sentence beside them **is** that answer, and there is no second inference
/// coming to replace it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_completion_answers_the_turn_it_spoke_for() {
    let mock = MockOpenAI::start(vec![
        // ONE completion carrying both. The round is over when it lands: the
        // only call is async and not a handoff, so nothing will come back and
        // no iteration follows. If a second response were consumed here, this
        // test would be measuring a re-entry rather than the answer.
        canned_content_and_tool_calls(
            "Noted -- Helix it is.",
            vec![("call-r1", "remember", REMEMBER_ARGS)],
        ),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    let user_rows = await_rows(&db, USER_EPISODE, 1);
    let user_turn = &user_rows[0];
    std::fs::write(&marker, b"go").unwrap();

    // CLAIM 1 -- the channel is answered, and the answer is the sentence the
    // model actually said. Before the fix this arrived marked `interim`, which
    // is a promise of a final that never came.
    let answer = recv_bounded(&mut sink_rx)
        .await
        .expect("a mixed completion must still answer the turn");
    assert_eq!(text_of(&answer), "Noted -- Helix it is.");
    assert_ne!(
        hop_of(&answer, "interim"),
        "1",
        "every call in this bundle is async and not a handoff, so nothing is \
         being waited for and no second inference is coming -- the sentence \
         beside the call IS the turn's answer, and marking it interim is what \
         stranded 10 of 12 measured rounds (GH #378)"
    );
    assert_ne!(hop_of(&answer, "round_capped"), "1", "the round had budget");
    assert_ne!(hop_of(&answer, "degraded"), "1", "no store refusal here");

    // CLAIM 2 -- and the async call still did its work. A fix that answered the
    // turn by dropping the annotation would pass claim 1 and be worse than the
    // bug.
    let facts = await_rows(&db, FACTS, 1);
    assert_eq!(facts.len(), 1, "one fact, not two: {facts:?}");
    assert_eq!(
        facts[0][0], user_turn[0],
        "the fact hangs on the episode of the turn it answered"
    );
    assert_eq!(facts[0][2], "favorite_editor");
    assert_eq!(facts[0][3], "Helix");

    assert!(
        reject_rx.try_recv().is_err(),
        "a good block does not travel the reject lane"
    );
    assert_eq!(dlq_count(td.path()), 0, "nothing dead-letters on the way");

    // The prose half of the § 2d drift lock. The mechanism above is the
    // promise; this is the sentence on the dispatcher's public surface that
    // states it. A repair that moved one without the other is the class that
    // put this rule in development-rules in the first place.
    let readme = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/dispatcher/README.md");
    if let Ok(text) = std::fs::read_to_string(&readme) {
        let para = text
            .split("\n\n")
            .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
            .find(|p| p.contains("depends on the bundle beside it"))
            .expect(
                "templates/dispatcher/README.md must still say that `interim` \
                 depends on the bundle — a README that promises it \
                 unconditionally describes the bug (GH #378)",
            );
        for phrase in [
            "non-async",
            "handoff",
            "fire-and-forget",
            "goes out unmarked",
        ] {
            assert!(
                para.contains(phrase),
                "the interim paragraph must still name `{phrase}`: the three \
                 classes are what makes the rule checkable rather than a \
                 slogan\n  {para}"
            );
        }
    }

    h.shutdown().await;
}

/// GH #526, the same lock as in `w10b_remember_colony.rs` and for the same
/// reason: this file's barrier read its marker out of the process environment
/// while the key was only ever written into the root env file -- which feeds
/// `config.json` VALUE substitution, not a `code` cell's subprocess. The wait
/// was skipped, and the ordering this test relies on was a race.
#[test]
fn the_barrier_carries_its_marker_in_its_own_script() {
    assert!(
        !GATE.contains("os.environ"),
        "the barrier reads its marker out of the environment again -- a `code` \
         cell's subprocess does not see the root env file (GH #526)"
    );
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, "http://127.0.0.1:1/v1", &marker);
    let cfg = std::fs::read_to_string(td.path().join("main/gate/config.json")).unwrap();
    let v: Value = meclaw_core::serde_json::from_str(&cfg).unwrap();
    let script = v["params"]["script_inline"]
        .as_str()
        .expect("script_inline");
    assert!(
        script.contains(&marker.display().to_string()),
        "the deployed barrier does not carry the marker path it waits for: {script}"
    );
}
