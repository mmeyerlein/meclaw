//! meclaw-os W10b -- inline extraction in a running colony (wave 10, track B).
//!
//! Rebuilt in W5.7 (GitHub #379). The track used to drive a `remember` TOOL
//! CALL; per-turn extraction is a fenced block inside the answer now, cut back
//! out by `talky/splitter`. The two claims below did not change -- only the
//! delivery did, which is the whole point of the flip.
//!
//! The script pins live in `w10b_inline_gate.rs`. This file asks the question
//! the track is about, of a colony that carries the shipped `talky` and the
//! memory hive's REAL write and extraction path (`writer`, `store`,
//! `extract-glue` -- the private templates, which is why this file stays
//! private):
//!
//!   surface -> talky -> brain -> splitter --route 'extraction'--> memory/extract-glue
//!                                        \--> dispatcher --route 'answer'--> the channel
//!                    -> route turn_write -> memory/writer -> episodes
//!
//! `memory-drain` used to sit in the middle of that last line and does not any
//! more (GH #523): its ledger is a per-session high-water mark over ONE closed
//! batch, and a per-turn cadence hands it two batches of one session at a time.
//! `memory_drain.rs` and `memory_drain_colony.rs` measure the adapter on the
//! bulk-import shape it is actually for.
//!
//! Two claims, and they are the whole track:
//!
//! 1. **An annotated turn becomes a fact candidate on the turn it answered, and
//!    the person never sees the annotation.** The block names no episode -- it
//!    cannot -- and the hive binds it to the newest `user` episode of the
//!    session, the row the per-turn lane minted under
//!    `turn_id = "<session_id>#<index>"`. The answer that reaches the channel
//!    carries no fence.
//! 2. **Extraction never costs the answer.** A block with a broken payload is
//!    NOT cut: the splitter flags it and leaves the answer alone, nothing is
//!    written, and the turn stays in the queue for the close pass. That is
//!    guard 1 of the inline design, and it is the only guarantee that makes
//!    putting an extraction on the answering call defensible at all.
//!
//!    It is also where the flip changed a visible behaviour, and the assertion
//!    says so out loud: under the tool form a broken payload left through
//!    `inline-reject` and the person's answer was untouched. Under the sidecar
//!    the payload is INSIDE the answer, so leaving the answer untouched means
//!    the fence travels with it. Half-cutting on a block nobody could read was
//!    the worse of the two, and the close pass reads the turn either way. (Every
//!    measured V2 run left the malformed bucket at zero.)
//!
//! **The barrier.** In production the ordering is free: the per-turn episode is
//! a handful of in-process store hops while the answer carrying the annotation
//! is a model generation, seconds away. Against a mock wire that answers in a
//! millisecond the two chains are the same length, so this test makes the
//! ordering explicit instead of hoping for it: a `gate` cell holds the sidecar
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
use mock_openai::{MockOpenAI, canned_chat_completion};
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

/// THE BARRIER (see the module note). Holds the sidecar until the test has
/// seen the episode it must bind to, then forwards it byte for byte -- header
/// and body, so the port edge downstream sees exactly what the splitter
/// emitted. A test fixture, never a template.
///
/// ITS DEADLINE OUTLIVES THE OBSERVER'S, and that ordering is not cosmetic: the
/// marker is written after the observer below returns, and the observer is
/// allowed 30 s under cargo-parallel load. A barrier that gave up first would
/// release the sidecar while the observer was still waiting entirely
/// legitimately -- so the barrier waits longer than the wait it is synchronised
/// with, the cell's own operation timeout (`external_timeout_ms`) outlasts the
/// barrier, and `message_timeout` outlasts that (CLAUDE.md rule 12, B generous
/// and A precise). Whoever moves one of the three moves all three.
///
/// **THE MARKER PATH IS SUBSTITUTED INTO THE SCRIPT, never read from the
/// environment (GH #526).** Until this issue the script asked
/// `os.environ.get("W10B_MARKER")` while the test only wrote the key into
/// `{root}/.env` -- and `.env` is what the substrate substitutes into
/// `config.json` values, not what a `code` cell's subprocess inherits (the
/// child inherits the TEST process's environment, `code::child` `env_clear:
/// false`). So the lookup returned `""`, the `while marker and ...` loop never
/// ran once, and the barrier this module describes at length has never held
/// anything back. What that cost is measured in the issue: the sidecar raced the
/// per-turn episode, and under CPU contention it won -- the inline ingress found
/// no `user` episode to bind to, refused the block ("no episode for this
/// session"), and the test died 30 s later on a fact that was never going to
/// arrive. A barrier that cannot fail loudly is not a barrier, so the
/// substituted path is asserted before the wait.
const GATE: &str = r#"
import sys, json, os, time
d = json.load(sys.stdin)["body"]
marker = "W10B_MARKER_PATH"
if not marker or marker.endswith("_MARKER_PATH"):
    raise SystemExit("gate: no marker path was substituted into this script")
deadline = time.time() + 45
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
        {"from": "./surface", "to": "./talky",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id"}}},
        // THE PER-TURN LANE, and since ruling Q11 (GH #298) the whole write
        // path: ONE edge from the talky's `turn_write` route into the hive's
        // writer port, with nothing between them -- the shape `w9a` measures
        // and the shape `member@1.5.0` ships (GH #527).
        //
        // **It used to run through `memory-drain` and that is retracted (GH
        // #523).** The adapter turns ONE CLOSED SESSION into N episodes and its
        // ledger is a per-session high-water mark: one parked `batch` row, one
        // `drained_upto`. Two per-turn batches of the same session in flight at
        // once break both halves of that -- the probe reads the LAST parked
        // batch, and under load the assistant's batch is parked before the
        // user's probe runs. Measured: two `assistant` episodes under
        // `<session>#0`, no `user` episode at all, and an empty dead-letter
        // queue, because nothing was refused -- the wrong thing was written.
        // `templates/memory-drain/README.md` says not to draw this edge; this
        // file was the last place still drawing it.
        //
        // The three keys the collector mints per turn (`turn_id`,
        // `happened_at`, `session_id`) are promoted here, together with the
        // provenance the writer refuses to guess (#244/#269): `audience_set`
        // says who was in the round and `speaker`/`agent_id` who said it;
        // `channel` is not set here -- it travels from the connector seam above
        // (`hop.chat_id` -> `context.channel`), exactly as it does in a real
        // colony. One person, one agent, one room: that is this colony's whole
        // cast, and a `["*"]` here would blind the suite to the leak the gate
        // stops.
        {"from": "./talky", "to": "./memory/writer",
         "condition": "has(hop.route) && hop.route == 'turn_write'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "turn_id": "hop.turn_id",
                                      "happened_at": "hop.happened_at",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'",
                                      "speaker": "'member:user'",
                                      "agent_id": "'agent:assistant'"}}},
        // The close route reaches no memory any more (Q11). `KEEPER_IDLE_MS=0`
        // means it fires during the run, so it is terminated rather than left
        // unrouted -- an unrouted emission is a dead letter, and the DLQ
        // assertions are what makes this file's "nothing was lost" claim mean
        // anything. It is NOT wired into the memory: the per-turn lane has
        // already written these very turns, and a second writer over them would
        // mint a second episode per turn under the same `turn_id`.
        {"from": "./talky", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'write'"},
        // WAVE 10b, edge 1 of 2: the extraction sidecar into the inline ingress.
        // Since talky@4.1.0 this is a ROUTE, not a tool name (GH #379). In
        // production the edge goes straight to the memory; here it takes the
        // test barrier on the way (module note).
        {"from": "./talky", "to": "./gate",
         "condition": "has(hop.route) && hop.route == 'extraction'"},
        // The inline ingress mints facts directly, so the same round travels
        // with it (#244). Same audience as the write lane above: the block is
        // written INSIDE the turn those two exchanged.
        {"from": "./gate", "to": "./memory/extract-glue",
         "condition": "has(hop.route) && hop.route == 'remember'",
         "modifier": {"set_context": {"store_origin": "'inline'", "mem_phase": "'inline'",
                                      "audience_set": "'[\"member:user\",\"agent:assistant\"]'"}}},
        // WAVE 10b, edge 2 of 2: the reject drain. Without it a discarded block
        // is an unrouted dead end, and nobody ever learns the memory was not
        // written (the defect the review found in the running colony).
        {"from": "./memory/extract-glue", "to": "/reject",
         "condition": "has(hop.route) && hop.route == 'reject'"},
        {"from": "./talky", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./talky", "to": "./void",
         "condition": "has(hop.route) && hop.route == 'error'"},
        // The two lanes this track does not measure: the batched extractor and
        // the embedding queue. Terminal, so the DLQ assertion keeps meaning.
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
        // No DISPATCHER_ASYNC_TOOLS: the annotation is not a tool call any
        // more, so there is nothing for the dispatcher to classify (#379).
        //
        // No W10B_MARKER either, and that is the repair of GH #526: the
        // barrier's path is substituted into its SCRIPT below. The root env
        // file feeds `${VAR}` substitution in `config.json` values; it is not
        // the environment a `code` cell's subprocess sees, so a key parked here
        // was invisible to the gate that read it.
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
    write(root, "main/gate/config.json", &{
        // The barrier waits up to 45 s (see GATE). Under the helper's own
        // 30 s operation timeout the substrate would cut it off mid-wait, and
        // an ordering guarantee would become a cell error. Both deadlines
        // therefore grow with it.
        let mut c = code_cell(
            &GATE.replace("W10B_MARKER_PATH", &marker.display().to_string()),
            &["remember"],
            json!({}),
        );
        c["cell"]["message_timeout"] = json!(70000);
        c["params"]["external_timeout_ms"] = json!(60000);
        c
    });
    write(
        root,
        "main/void/config.json",
        &code_cell(VOID, &[], json!({})),
    );
    copy_cells(&repo("templates/talky"), &root.join("main/talky"));
    memory_hive(root);

    // The per-turn lane is off by default; a parent that wires it says so HERE,
    // in the instance's own params. A colony-global `.env` key is not the
    // mechanism any more (`collector@1.2.0`, wave 13).
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

/// The same wait, with the state a failure needs to be READABLE (GH #526).
///
/// A poll that gives up says only that the row is not there, and on this pair
/// the question is always *why*: which episodes the per-turn lane wrote and
/// under which `sender`, whether the turn left the queue, whether the ingress
/// parked a block it never came back for, whether it REFUSED one, and where in
/// the memory hive the chain stopped. All of that is already an observable of
/// this colony -- the store's own tables, the dead-letter table and the central
/// message log -- and none of it was printed, so every failing run cost a rerun
/// with a hand-added dump. The failure text carries it now, and it is what took
/// GH #526 from "flaky" to a named mechanism in one run.
fn await_rows_or_dump(
    db: &std::path::Path,
    sql: &str,
    n: usize,
    root: &std::path::Path,
) -> Vec<Vec<String>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let r = rows(db, sql);
        if r.len() >= n {
            return r;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "{} of {n} rows after 30s for {sql:?}: {r:?}\n\
                 EPISODES = {:?}\n\
                 QUEUE    = {:?}\n\
                 SCRATCH  = {:?}\n\
                 DLQ      = {:?}\n\
                 REFUSED  = {:#?}\n\
                 MEMORY   = {:#?}",
                r.len(),
                rows(db, "SELECT id, turn_id, sender, session_id FROM episodes"),
                rows(db, "SELECT episode_id, status FROM pending_extraction"),
                rows(db, "SELECT key, kind FROM scratch"),
                rows(
                    &root.join("colony.db"),
                    "SELECT reason, target FROM dead_letters"
                ),
                refusals(root),
                memory_lane(root),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What travelled the reject lane, with the reason on it. A refused block and a
/// slow one look identical from the `facts` table, and this is the column that
/// tells them apart.
fn refusals(root: &std::path::Path) -> Vec<String> {
    rows(
        &root.join("colony.db"),
        "SELECT json_extract(headers, '$.hop.reject_reason'), body_payload \
         FROM message_log WHERE to_path = '/reject' ORDER BY created_at ASC, id ASC",
    )
    .into_iter()
    .map(|r| r.join(" | ").chars().take(400).collect())
    .collect()
}

/// The memory hive's own leg of the central message log, in order: which cell
/// handed what to which, on which phase. A chain that stopped shows up as a
/// phase with no answer under it.
fn memory_lane(root: &std::path::Path) -> Vec<String> {
    rows(
        &root.join("colony.db"),
        "SELECT from_path, to_path, \
         json_extract(headers, '$.hop.route'), \
         json_extract(headers, '$.hop.operation'), \
         json_extract(headers, '$.hop.error_code'), \
         json_extract(headers, '$.context.mem_phase') \
         FROM message_log \
         WHERE from_path LIKE '/memory%' OR to_path LIKE '/memory%' \
         ORDER BY created_at ASC, id ASC",
    )
    .into_iter()
    .map(|r| r.join(" | "))
    .collect()
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
const FACTS: &str = "SELECT episode_id, subject, predicate, claim, fact_kind, \
                     IFNULL(valid_until,'') FROM facts";

/// The annotation the model writes into its own answer: the payload the tool
/// call used to carry, in the fence the shipped contract asks for.
const SIDECAR: &str = r#"{"facts":[{"subject":"user","predicate":"Favorite Editor","claim":"Helix","fact_kind":"world","confidence":90}],"topic":{"movement":"start","name":"editors"}}"#;

/// Claim 1: one turn, one annotation, and the fact hangs on the episode of the
/// turn it answered -- while the answer is already in the channel, without the
/// block in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_annotated_turn_is_a_fact_candidate_on_the_turn_it_answered() {
    let answer_text = format!("Noted -- Helix it is.\n\n```memory\n{SIDECAR}\n```");
    let mock = MockOpenAI::start(vec![canned_chat_completion(&answer_text, "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    // THE ANSWER IS FIRST AND IT IS FENCE-FREE. It came out of the SAME
    // completion that carried the annotation -- no second inference -- and the
    // splitter took the instrument out before the dispatcher ever saw it.
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");
    assert_eq!(
        text_of(&answer),
        "Noted -- Helix it is.",
        "the person is handed the prose and never the block"
    );
    assert!(
        !text_of(&answer).contains("```memory"),
        "and the fence is not in it: {:?}",
        text_of(&answer)
    );
    let session = hop_of(&answer, "session_id");
    assert!(session.starts_with("c-10b-"), "session {session:?}");

    // The per-turn lane of wave 9 has minted the episode. Releasing the barrier
    // here is what makes the ordering explicit (module note).
    let user_rows = await_rows(&db, USER_EPISODE, 1);
    let user_turn = &user_rows[0];
    assert_eq!(
        user_turn[1],
        format!("{session}#0"),
        "under the collector's own deterministic id"
    );
    std::fs::write(&marker, b"go").unwrap();

    // CLAIM 1: the block named no turn and still landed on the right one.
    let facts = await_rows_or_dump(&db, FACTS, 1, td.path());
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

/// Claim 2: extraction never costs the answer. A block nobody can read writes
/// nothing, covers no turn -- and the channel got its sentence, and only its
/// sentence.
///
/// The shape of the guarantee moved with the delivery and the assertions say so.
/// Under the tool form the broken payload travelled its own lane and left
/// through `inline-reject`. Under the sidecar it is INSIDE the answer, so
/// "leave the answer alone" and "get the block out" looked like two ways of
/// making one decision, and the splitter chose the first: a parser that could
/// not read the block does not get to edit the sentence around it.
///
/// **RETRACTED (GH #534).** They were never one decision. The cut is the span
/// the parser already located, so the sentence around it is the same sentence
/// either way -- "leave the answer alone" bought nothing and cost a reader raw
/// JSON in a chat window when a model dropped one closing brace. The block comes
/// out; nothing unreadable travels; the memory writes nothing; the turn stays in
/// the queue and the close pass reads it. Every measured V2 run left this bucket
/// at zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_annotation_costs_the_answer_nothing() {
    let broken = "Got it.\n\n```memory\n{\"facts\": [ this is not json\n```";
    let mock = MockOpenAI::start(vec![
        canned_chat_completion(broken, "stop"),
        canned_chat_completion("ignored", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let marker = td.path().join("episode-seen");
    build_tree(&td, &mock.base_url, &marker);
    let (h, mut sink_rx, mut reject_rx) = boot(&td).await;
    let db = td.path().join("main/memory/store/cell.db");

    h.send(turn("my favourite editor is Helix")).await;

    // THE ANSWER ARRIVES -- that is the guarantee, and it holds. And it arrives
    // WITHOUT the block: this assertion used to require the opposite (GH #534).
    let answer = recv_bounded(&mut sink_rx).await.expect("the answer");
    assert_eq!(
        text_of(&answer),
        "Got it.",
        "the sentence reached the channel, and nothing else did"
    );

    let user_rows = await_rows(&db, USER_EPISODE, 1);
    let user_turn = &user_rows[0];
    std::fs::write(&marker, b"go").unwrap();

    // NOTHING in the store: no fact, and the turn is still the close pass's.
    assert!(
        rows(&db, FACTS).is_empty(),
        "an unreadable block writes no fact"
    );
    let pending = rows(
        &db,
        "SELECT status FROM pending_extraction WHERE status = 'inline'",
    );
    assert!(
        pending.is_empty(),
        "and covers no turn: the close pass keeps {user_turn:?}"
    );

    // THE REJECT LANE STAYS EMPTY, and that is the difference the flip made:
    // the block never left the composite, so there is nothing for the hive to
    // refuse. What carries the miss is `hop.sidecar == "malformed"` on the
    // answer message, which `gh379_the_splitter_cuts_the_sidecar` pins directly.
    assert!(
        reject_rx.try_recv().is_err(),
        "the hive was never asked, so it refused nothing"
    );
    assert_eq!(
        dlq_count(td.path()),
        0,
        "and nothing dead-letters on the way"
    );
    h.shutdown().await;
}

/// GH #526 -- THE BARRIER IS A BARRIER, and this is the assertion that says so.
///
/// The failure it locks out is not a wrong value, it is a silent nothing: the
/// script asked the process environment for a key the test only ever wrote into
/// the root env file, the lookup returned `""`, and `while marker and ...`
/// skipped the wait entirely. Both halves of the module note above -- "a `gate`
/// cell holds the sidecar until the test has SEEN the episode" and the three
/// nested deadlines -- described a mechanism that was not there, and the only
/// symptom was an intermittent 30 s failure four function calls further down.
///
/// So the path is checked where it has to be for the barrier to exist at all:
/// in the script of the cell the colony actually deploys.
#[test]
fn the_barrier_carries_its_marker_in_its_own_script() {
    assert!(
        !GATE.contains("os.environ"),
        "the barrier reads its marker out of the environment again. A `code` \
         cell's subprocess inherits the TEST process's environment \
         (`code::child`, `env_clear: false`); the root env file is what the \
         substrate substitutes into `config.json` VALUES. A key written there \
         is invisible here, the wait is skipped, and the ordering this file \
         relies on becomes a race that only fails under load (GH #526)"
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
        "the deployed barrier does not carry the marker path it is supposed to \
         wait for: {script}"
    );

    // And it fails LOUDLY rather than quietly not existing: the unsubstituted
    // script exits non-zero, which a `code` cell reports as a cell error instead
    // of letting the sidecar past.
    let doc = json!({"envelope": {"header": {"hop": {}, "context": {}}},
                     "body": {"messages": []}});
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        let src = format!(
            "import sys, io\n_s = {}\nsys.stdin = io.StringIO({})\n\
             exec(compile(_s, 'gate', 'exec'), globals())\n",
            meclaw_core::serde_json::to_string(GATE).unwrap(),
            meclaw_core::serde_json::to_string(&doc.to_string()).unwrap(),
        );
        sink.write_all(src.as_bytes()).expect("write");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        !out.status.success(),
        "an unsubstituted barrier let the sidecar straight through instead of \
         refusing: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
