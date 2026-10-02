//! GH #888, the colony case -- the curator's ledger rebuilds, byte for byte,
//! what the provider received, and a cold cache is rebuilt exactly once.
//!
//! Measured AT THE RECEIVER: the request bodies a stub provider recorded. The
//! shipped `curator` hive is booted in a test colony in front of a REAL `llm`
//! cell (the brain) pointed at the stub; the hive's own summarizer points at
//! the same stub. A driver plays the collector (`in_curate`), and every output
//! of the brain comes back on the tap (`in_llm`) through one code cell that
//! stamps `cache_expires_at` -- the stamp the `llm` cell itself learns in
//! strand C of the same wave; the tap lane is the same either way.
//!
//! Pinned:
//!
//! 1. for every call the ledger names -- two turns, one of them with a tool
//!    round, and one after the rebuild -- `call_blocks` -> `blocks` gives the
//!    `messages` of the recorded request exactly (the system message rebuilt
//!    from the system leaves the way the `llm` cell concatenates them, the
//!    turns mapped the way it maps them; no `model_prompt` is configured);
//! 2. the stamp two seconds ahead on the last call of the second turn orders
//!    exactly one rebuild: one summarizer request, one `summaries` row whose
//!    sources are the first turn's wall rows, and the earlier orders never
//!    strike;
//! 3. the next request carries the summary as `system.history.summary`.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;

const SESSION: &str = "s-888";
/// The audience of the driver's rounds (GH #925), as the TEXT a colony carries
/// on the context. The rebuild starts on a strike of the clock, a fresh root;
/// the round rides with the order (`emit_headers`, OR-BD.A.6), and without one
/// the rebuild keeps the old window and makes no summary.
const ROUND: &str = r#"["member:e"]"#;
const SUMMARY: &str = "The person said hello and was greeted.";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    repo("templates/curator/config.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).expect("write");
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
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
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

/// Emits exactly the `{"header": ..., ...}` it is handed; the context is the
/// injected message's own and rides on.
const DRIVER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps(json.loads(d["messages"][0]["text"])))
"#;

/// The tap: every output of the brain, handed back with a `cache_expires_at`.
/// Two seconds ahead on the final answer of the second turn, an hour ahead on
/// everything else -- so the one strike the test waits for is decided by the
/// stamp and not by how fast a loaded host runs the turns before it.
const STAMPER: &str = r#"
import sys, json, datetime
doc = json.load(sys.stdin)
header = doc["envelope"]["header"]
hop, ctx, body = header.get("hop") or {}, header.get("context") or {}, doc["body"]
out = dict(hop)
out["route"] = "tap"
soon = ctx.get("turn_id") == "t2" and hop.get("finish_reason") == "stop"
at = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=2 if soon else 3600)
out["cache_expires_at"] = at.strftime("%Y-%m-%dT%H:%M:%SZ")
sys.stdout.write(json.dumps({"header": out, "messages": body.get("messages") or []}))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

const SYSTEM_ORDER: [&str; 3] = ["identity", "instructions", "history"];

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").expect("env file");
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./driver", "to": "./curator",
             "condition": "has(hop.route) && hop.route == 'curate'",
             "modifier": {"set_hop": {"route": "'in_curate'"}}},
            {"from": "./curator", "to": "./brain",
             "condition": "has(hop.route) && hop.route == 'brain'",
             "modifier": {"set_context": {"curator_call": "hop.curator_call",
                                          "turn_id": "hop.turn_id",
                                          "session_id": "hop.session_id",
                                          "iter": "hop.iter"}}},
            {"from": "./brain", "to": "./stamper", "condition": "has(hop.finish_reason)"},
            {"from": "./stamper", "to": "./curator",
             "condition": "has(hop.route) && hop.route == 'tap'",
             "modifier": {"set_hop": {"route": "'in_llm'"}}},
            {"from": "./curator", "to": "/sink",
             "condition": "has(hop.route) && hop.route != 'brain'"}
        ]}}}),
    );
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: plays the collector."),
    );
    write(
        root,
        "main/stamper/config.json",
        &double(STAMPER, "Test tap: stamps the cache expiry."),
    );
    copy_cells(&repo("templates/curator"), &root.join("main/curator"));
    patch(root, "main/curator/summarizer/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
        v["params"]["api_key"] = json!("sk-test");
    });
    patch(root, "main/curator/policy/config.json", |v| {
        v["params"]["keep_recent"] = json!(1);
    });
    // The brain: a plain `llm` cell, the summarizer's own config pointed at the
    // stub with the system order a conversation brain behind a curator runs.
    let mut brain = read_json(&repo("templates/curator/summarizer/config.json"));
    brain["params"]["base_url"] = json!(base_url);
    brain["params"]["model"] = json!("gpt-4o-mock");
    brain["params"]["api_key"] = json!("sk-test");
    brain["params"]["system_order"] = json!(SYSTEM_ORDER);
    write(root, "main/brain/config.json", &brain);
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    tokio::sync::mpsc::Receiver<meclaw_core::Message>,
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
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped curator must boot");
    (h, rx)
}

/// One round at the curator's door, as the collector hands it.
fn round(turn_id: &str, iter: u32, messages: Value) -> meclaw_core::Message {
    let ctx: Map<String, Value> = json!({"session_id": SESSION, "turn_id": turn_id,
                                         "iter": iter.to_string(), "audience_set": ROUND})
    .as_object()
    .cloned()
    .unwrap();
    let spec = json!({"header": {"route": "curate", "session_id": SESSION, "turn_id": turn_id,
                                 "iter": iter.to_string(), "phase": ""},
                      "messages": messages,
                      "system": {"instructions": {"mode": {"text": "Answer briefly."}}}});
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": spec.to_string()}]})))
        .context(ctx)
        .ttl(400)
        .build()
}

fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

async fn requests(mock: &MockOpenAI, n: usize) -> Vec<Value> {
    for _ in 0..600 {
        let reqs = mock.recorded_requests().await;
        if reqs.len() >= n {
            return reqs.into_iter().map(|r| r.body).collect();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the provider was called fewer than {n} time(s) within 60 s");
}

/// The ledger's `cell.db`, read-only: opening it read-write before the store
/// woke would create an empty file, and a store that finds its file has no
/// seed to load.
fn try_ledger(td: &tempfile::TempDir) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        td.path().join("main/curator/ledger/cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
}

fn ledger(td: &tempfile::TempDir) -> rusqlite::Connection {
    try_ledger(td).expect("the ledger")
}

/// Poll the ledger until `sql` counts at least `n`, or give up after 30 s.
async fn until_count(td: &tempfile::TempDir, sql: &str, n: i64) {
    for _ in 0..300 {
        let got: i64 = try_ledger(td)
            .and_then(|c| c.query_row(sql, [], |r| r.get(0)))
            .unwrap_or(0);
        if got >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("`{sql}` did not reach {n} within 30 s");
}

/// Collect the text of every leaf below `node`, alphabetically, skipping
/// empty texts -- `concat_system_prompt` of the `llm` cell.
fn walk(node: &Value, out: &mut Vec<String>) {
    let Some(obj) = node.as_object() else { return };
    if let Some(t) = obj.get("text") {
        if let Some(t) = t.as_str().filter(|t| !t.is_empty()) {
            out.push(t.to_string());
        }
        return;
    }
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for k in keys {
        walk(&obj[k], out);
    }
}

fn system_prompt(tree: &Map<String, Value>) -> String {
    let mut keys: Vec<String> = tree.keys().filter(|k| *k != "tools").cloned().collect();
    let mut ordered = Vec::new();
    for k in SYSTEM_ORDER {
        if let Some(pos) = keys.iter().position(|x| x == k) {
            ordered.push(keys.remove(pos));
        }
    }
    keys.sort();
    ordered.extend(keys);
    let mut parts = Vec::new();
    for k in &ordered {
        walk(&tree[k], &mut parts);
    }
    parts.join("\n\n")
}

/// The turns as the `llm` cell maps them for a chat completion, consecutive
/// tool calls merged into one assistant message.
fn wire(turns: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut prev_call = false;
    for t in turns {
        let text = t["text"].as_str().unwrap_or("");
        let id = t["id"].as_str().unwrap_or("");
        let call = t["origin"] == "assistant" && t["type"] == "tool_call";
        if call {
            let function: Value = meclaw_core::serde_json::from_str(text).expect("a call");
            let entry = json!({"id": id, "type": "function", "function": function});
            if prev_call {
                out.last_mut().unwrap()["tool_calls"]
                    .as_array_mut()
                    .unwrap()
                    .push(entry);
            } else {
                out.push(json!({"role": "assistant", "content": null, "tool_calls": [entry]}));
            }
        } else if t["type"] == "tool_result" {
            out.push(json!({"role": "tool", "tool_call_id": id, "content": text}));
        } else {
            let role = if t["origin"] == "assistant" {
                "assistant"
            } else {
                "user"
            };
            out.push(json!({"role": role, "content": text}));
        }
        prev_call = call;
    }
    out
}

/// What the ledger says call `id` carried, as the provider receives it.
fn rebuilt(td: &tempfile::TempDir, id: &str) -> Vec<Value> {
    let conn = ledger(td);
    let mut st = conn
        .prepare(
            "SELECT b.kind, b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
             WHERE cb.call_id = ?1 ORDER BY cb.pos",
        )
        .unwrap();
    let rows: Vec<(String, String)> = st
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut tree = Map::new();
    let mut turns = Vec::new();
    for (kind, body) in rows {
        let el: Value = meclaw_core::serde_json::from_str(&body).expect("a block");
        if kind == "system" {
            let path = el["path"].as_str().unwrap().to_string();
            let mut at = &mut tree;
            let parts: Vec<&str> = path.split('.').collect();
            for seg in &parts[..parts.len() - 1] {
                at = at
                    .entry(seg.to_string())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .unwrap();
            }
            at.insert(
                parts[parts.len() - 1].to_string(),
                json!({"text": el["text"]}),
            );
        } else {
            turns.push(el);
        }
    }
    let mut out = Vec::new();
    let system = system_prompt(&tree);
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    out.extend(wire(&turns));
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ledger_rebuilds_what_the_provider_received() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Hello there.", "stop"),
        canned_tool_calls(vec![("call_1", "get_weather", "{\"city\":\"Berlin\"}")]),
        canned_chat_completion("It is sunny.", "stop"),
        canned_chat_completion(SUMMARY, "stop"),
        canned_chat_completion("You are welcome.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url);
    let (h, _sink) = boot(&td).await;
    let answered = "SELECT COUNT(*) FROM calls WHERE model != ''";

    // Turn 1.
    h.send(round("t1", 0, json!([user("hello")]))).await;
    requests(&mock, 1).await;
    until_count(&td, answered, 1).await;
    until_count(&td, "SELECT COUNT(*) FROM wall WHERE final = 1", 1).await;

    // Turn 2, a tool round: the brain asks for a tool, the collector comes back
    // with the whole round.
    h.send(round("t2", 0, json!([user("weather?")]))).await;
    requests(&mock, 2).await;
    until_count(&td, answered, 2).await;
    let call = json!({"origin": "assistant", "type": "tool_call", "id": "call_1",
                      "text": json!({"name": "get_weather",
                                     "arguments": "{\"city\":\"Berlin\"}"}).to_string()});
    let result = json!({"origin": "tool", "type": "tool_result", "id": "call_1",
                        "text": "sunny, 21 degrees"});
    h.send(round("t2", 1, json!([user("weather?"), call, result])))
        .await;
    requests(&mock, 3).await;

    // The stamp two seconds ahead strikes once: one summary, in the summary
    // slot of the round (`history.summary:<round key>`, GH #943).
    until_count(&td, "SELECT COUNT(*) FROM summaries", 1).await;
    until_count(
        &td,
        "SELECT COUNT(*) FROM slots WHERE path LIKE 'history.summary:%'",
        1,
    )
    .await;

    // Turn 3, after the rebuild.
    h.send(round("t3", 0, json!([user("thanks")]))).await;
    let reqs = requests(&mock, 5).await;
    until_count(&td, answered, 4).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        5,
        "exactly one rebuild"
    );

    let is_summary = |r: &Value| {
        r["messages"][0]["content"]
            .as_str()
            .is_some_and(|s| s.contains("You condense the earlier part of an exchange"))
    };
    assert!(
        is_summary(&reqs[3]),
        "the fourth request is the summarizer's: {}",
        reqs[3]
    );
    let brain: Vec<&Value> = reqs.iter().filter(|r| !is_summary(r)).collect();
    assert_eq!(brain.len(), 4);

    // 1. Every call the ledger names, rebuilt from its blocks, is the request.
    let conn = ledger(&td);
    let mut st = conn
        .prepare("SELECT call_id FROM calls ORDER BY started_at")
        .unwrap();
    let ids: Vec<String> = st
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(ids.len(), 4, "four calls in the ledger");
    for (id, req) in ids.iter().zip(&brain) {
        let want = rebuilt(&td, id);
        let got = req["messages"].as_array().expect("messages").clone();
        assert_eq!(want, got, "call {id}: the ledger and the provider disagree");
        assert_eq!(
            want[0]["content"].as_str(),
            got[0]["content"].as_str(),
            "the system message, byte for byte"
        );
    }

    // 2. One summary, over the first turn.
    let (sources, covers): (String, i64) = conn
        .query_row("SELECT sources, covers_to_seq FROM summaries", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let mut st = conn
        .prepare("SELECT hash, seq FROM wall WHERE turn_id = 't1' ORDER BY seq")
        .unwrap();
    let t1: Vec<(String, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let sources: Value = meclaw_core::serde_json::from_str(&sources).unwrap();
    assert_eq!(
        sources,
        json!(t1.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>()),
        "the summary names the first turn's rows as its sources"
    );
    assert_eq!(covers, t1.last().unwrap().1);
    let summ_transcript = reqs[3]["messages"][1]["content"].as_str().unwrap_or("");
    assert_eq!(summ_transcript, "user: hello\nassistant: Hello there.");

    // 3. The request after the rebuild carries the summary, last in the system part.
    let system = brain[3]["messages"][0]["content"]
        .as_str()
        .expect("a system message");
    assert!(
        system.starts_with("Answer briefly.\n\n[earlier exchange, summarised"),
        "{system}"
    );
    assert!(system.ends_with(SUMMARY), "{system}");
    let turns: Vec<&str> = brain[3]["messages"].as_array().unwrap()[1..]
        .iter()
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert_eq!(
        turns,
        vec!["weather?", "sunny, 21 degrees", "It is sunny.", "thanks"],
        "the first turn left the messages; the second stayed whole"
    );
    h.shutdown().await;
}
