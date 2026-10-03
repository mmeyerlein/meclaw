//! A kept result travels with its round through a living colony, and a read at
//! the hive's rim answers only that round (GH #976, PE-DP-9 / D1 review E M-5).
//!
//! The pure locks (`gh976_a_result_is_kept_with_its_round`) prove what each
//! script says. This one measures at the receivers: the rows the stores really
//! hold, and the one `answer` that really leaves the hive.
//!
//! ```text
//! /keeper    -> /digest/shelf      a formatted digest, as ./format delivers it
//!                                  (context.digest_round = the run's round)
//! /archiver  -> /research/archive  the planner's final text, as the planner
//!                                  delivers it (context question, turn_id,
//!                                  audience_set)
//! /asker     -> /digest, /research `in_read` at the hive path (op last, op_id,
//!                                  context.audience_set = the asking round)
//! /digest, /research --answer--> /sink   what leaves the rim
//! ```
//!
//! Both hives are the SHIPPED templates, copied cell by cell. Only the cells
//! that would reach a network are stood in for by inert code cells with the
//! shipped hop contract (`daily-digest/notifier` and `/fetcher`;
//! `research-assistant/planner`, `/proxy`, `/reader`, `/searcher`): no message
//! of these locks reaches them, and the test reaches no provider. The digest
//! clock stays, with a literal schedule id and a cron that cannot fire during
//! a run (29 February, midnight).
//!
//! The relays write the context the way the hive's own edges would: an edge of
//! the boot graph promotes their hop fields into the context, which is how a
//! caller hands a hive its round. Edges straight into a hive's inner cell are
//! legal in a BOOT graph only (the seal is warned about there, not enforced);
//! they stand for `./format` and `./planner`, the cells that deliver in a
//! running hive.
//!
//! Waiting is on observed events only: the rows are polled out of the store's
//! own `cell.db` until they are there, the answers are awaited at the sink, and
//! a missing answer names the colony's dead letters.

use std::sync::Arc;
use std::time::{Duration, Instant};

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use tokio::sync::mpsc;

/// One member round and another, spelled unsorted on purpose.
const R1: &str = r#"["m-b","m-a"]"#;
const R1_KEPT: &str = r#"["m-a", "m-b"]"#;
const R2: &str = r#"["m-c"]"#;
const R2_KEPT: &str = r#"["m-c"]"#;

/// A cron that cannot fire during a test run.
const NEVER_CRON: &str = "0 0 0 29 2 *";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn library_ships() -> bool {
    [
        "templates/daily-digest/shelf/config.json",
        "templates/daily-digest/store/config.json",
        "templates/research-assistant/shelf/config.json",
        "templates/research-assistant/archive/config.json",
        "templates/research-assistant/memory/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

// ──────────────────────────────────────────────────────────────── the tree

/// The shipped template, copied cell by cell: `config.json` files and the seeds
/// next to them travel, which is what instantiation copies.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
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
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn read_json(p: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn code_cell(script: &str, hop: Value, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": false}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": purpose,
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// An inert stand-in for a cell that would reach a network: it answers nothing
/// and keeps the shipped hop contract, so every edge out of it reads the keys
/// it read before.
fn stand_in(root: &std::path::Path, rel: &str) {
    let shipped = read_json(&root.join(rel));
    let hop = shipped["contract"]["emits"]["hop"].clone();
    let hop = if hop.is_object() { hop } else { json!({}) };
    write(
        root,
        rel,
        &code_cell(
            "import sys\nsys.stdout.write('[]')\n",
            hop,
            "Test stand-in for a cell that would reach a network.",
        ),
    );
}

fn string_field() -> Value {
    json!({"type": "string", "required": false})
}

/// The keeper: hands `./shelf` a formatted digest the way `./format` does,
/// with the run's round on the hop for the boot edge to promote.
const KEEPER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages") or []
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
sys.stdout.write(json.dumps({
    "header": {"route": "keep", "round": str(a.get("round") or "")},
    "messages": [{"origin": "assistant", "type": "text", "text": str(a.get("text") or "")}]}))
"#;

/// The archiver: hands `./archive` the planner's final text with the turn's
/// question, id and round on the hop.
const ARCHIVER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages") or []
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
sys.stdout.write(json.dumps({
    "header": {"route": "archive", "question": str(a.get("question") or ""),
               "turn_id": str(a.get("turn_id") or ""), "round": str(a.get("round") or "")},
    "messages": [{"origin": "assistant", "type": "text", "text": str(a.get("text") or "")}]}))
"#;

/// The asker: a read at a hive's path, its round on the hop.
const ASKER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages") or []
a = json.loads(str(msgs[-1].get("text", "{}")) if msgs else "{}")
sys.stdout.write(json.dumps({
    "header": {"route": "read", "hive": str(a.get("hive") or ""), "op": str(a.get("op") or ""),
               "op_id": str(a.get("op_id") or ""), "round": str(a.get("round") or "")},
    "messages": []}))
"#;

/// The read edges into one hive: with a round it becomes `context.audience_set`
/// (the builder's word), without one the context carries none.
fn read_edges(hive: &str) -> [Value; 2] {
    let base = format!("has(hop.route) && hop.route == 'read' && hop.hive == '{hive}'");
    [
        json!({"from": "./asker", "to": format!("./{hive}"),
               "condition": format!("{base} && hop.round != ''"),
               "modifier": {"set_hop": {"route": "'in_read'"},
                            "set_context": {"audience_set": "hop.round"}}}),
        json!({"from": "./asker", "to": format!("./{hive}"),
               "condition": format!("{base} && hop.round == ''"),
               "modifier": {"set_hop": {"route": "'in_read'"}}}),
    ]
}

fn main_config() -> Value {
    let mut edges = vec![
        // ./format's delivery into the digest shelf, with and without a round.
        json!({"from": "./keeper", "to": "./digest/shelf",
               "condition": "has(hop.route) && hop.route == 'keep' && hop.round != ''",
               "modifier": {"set_context": {"digest_round": "hop.round"}}}),
        json!({"from": "./keeper", "to": "./digest/shelf",
               "condition": "has(hop.route) && hop.route == 'keep' && hop.round == ''"}),
        // The planner's final answer into the archive, with and without a round.
        json!({"from": "./archiver", "to": "./research/archive",
               "condition": "has(hop.route) && hop.route == 'archive' && hop.round != ''",
               "modifier": {"set_context": {"question": "hop.question", "turn_id": "hop.turn_id",
                                            "audience_set": "hop.round"}}}),
        json!({"from": "./archiver", "to": "./research/archive",
               "condition": "has(hop.route) && hop.route == 'archive' && hop.round == ''",
               "modifier": {"set_context": {"question": "hop.question", "turn_id": "hop.turn_id"}}}),
        // What leaves the rims.
        json!({"from": "./digest", "to": "/sink",
               "condition": "has(hop.route) && hop.route == 'answer'"}),
        json!({"from": "./research", "to": "/sink",
               "condition": "has(hop.route) && hop.route == 'answer'"}),
    ];
    edges.extend(read_edges("digest"));
    edges.extend(read_edges("research"));
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/keeper/config.json",
        &code_cell(
            KEEPER,
            json!({"route": {"type": "string", "values": ["keep"], "required": false},
                   "round": string_field()}),
            "Test relay: a formatted digest, as ./format delivers it.",
        ),
    );
    write(
        root,
        "main/archiver/config.json",
        &code_cell(
            ARCHIVER,
            json!({"route": {"type": "string", "values": ["archive"], "required": false},
                   "question": string_field(), "turn_id": string_field(),
                   "round": string_field()}),
            "Test relay: the planner's final answer, as the planner delivers it.",
        ),
    );
    write(
        root,
        "main/asker/config.json",
        &code_cell(
            ASKER,
            json!({"route": {"type": "string", "values": ["read"], "required": false},
                   "hive": string_field(), "op": string_field(), "op_id": string_field(),
                   "round": string_field()}),
            "Test relay: a read at a hive's path.",
        ),
    );

    copy_cells(&repo("templates/daily-digest"), &root.join("main/digest"));
    // `${uuid7:…}` is minted by instantiation; a raw copy needs a literal. The
    // cron is one that cannot fire while the test runs.
    patch(root, "main/digest/clock/config.json", |v| {
        v["params"]["schedules"][0]["schedule_id"] = json!("01916f00-0000-7000-8000-0000000009d6");
        v["params"]["schedules"][0]["cron"] = json!(NEVER_CRON);
    });
    stand_in(root, "main/digest/notifier/config.json");
    stand_in(root, "main/digest/fetcher/config.json");

    copy_cells(
        &repo("templates/research-assistant"),
        &root.join("main/research"),
    );
    for cell in ["planner", "proxy", "reader", "searcher"] {
        stand_in(root, &format!("main/research/{cell}/config.json"));
    }
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

// ───────────────────────────────────────────────────────── send and observe

fn send_json(cell: &str, v: &Value) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": serde_json::to_string(v).unwrap()}
        ]})))
        .ttl(400)
        .build()
}

fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn body_of(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("an inline body expected"),
    }
}

/// The rows `sql` reads out of a store's own `cell.db`, `cols` columns each as
/// text (`None` for NULL); empty while the file or the table is not there yet.
fn rows(db: &std::path::Path, sql: &str, cols: usize) -> Vec<Vec<Option<String>>> {
    let Ok(c) = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut st) = c.prepare(sql) else {
        return Vec::new();
    };
    st.query_map([], |r| {
        let mut out = Vec::with_capacity(cols);
        for i in 0..cols {
            let v: rusqlite::types::Value = r.get(i)?;
            out.push(match v {
                rusqlite::types::Value::Null => None,
                rusqlite::types::Value::Integer(n) => Some(n.to_string()),
                rusqlite::types::Value::Real(f) => Some(f.to_string()),
                rusqlite::types::Value::Text(s) => Some(s),
                rusqlite::types::Value::Blob(_) => Some("<blob>".to_string()),
            });
        }
        Ok(out)
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Poll the store's `cell.db` until `want` rows answer `sql` -- an observed
/// event, bounded, never a fixed window.
async fn wait_rows(
    db: &std::path::Path,
    sql: &str,
    cols: usize,
    want: usize,
) -> Vec<Vec<Option<String>>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let got = rows(db, sql, cols);
        if got.len() >= want {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "{} never held {want} rows for `{sql}`; it holds {got:?}",
            db.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The one `answer` for `op_id` at the sink. A missing answer names what the
/// colony dead-lettered meanwhile, so a red run says where the read died.
async fn answer_for(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, op_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen: Vec<String> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(m)) => {
                let route = hop_str(&m, "route");
                let id = hop_str(&m, "op_id");
                if route == "answer" && id == op_id {
                    return body_of(&m);
                }
                seen.push(format!("{route}/{id}: {}", body_of(&m)));
            }
            _ => {
                let dead: Vec<String> = h
                    .drain_dead_letters()
                    .await
                    .iter()
                    .map(|d| {
                        format!(
                            "{} -> {} ({:?})",
                            d.sender_path.as_str(),
                            d.original_target.as_str(),
                            d.reason
                        )
                    })
                    .collect();
                panic!(
                    "no answer for {op_id} left the rim; the sink saw {seen:?}; \
                     dead letters: {dead:?}"
                );
            }
        }
    }
}

/// The `key` of every entry of an answer's list, in the answer's order.
fn leads(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("a list expected: {list}"))
        .iter()
        .map(|d| d[key].as_str().unwrap_or_default().to_string())
        .collect()
}

// ─────────────────────────────────────────────────────────────── the locks

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_digest_is_kept_with_its_round_and_shown_only_to_it() {
    if !library_ships() || !have_python() {
        return;
    }
    let td = tempfile::tempdir().unwrap();
    build_tree(&td);
    let (h, mut rx) = boot(&td).await;
    let db = td.path().join("main/digest/store/cell.db");
    let all = "SELECT id, audience_set, items FROM digests ORDER BY at, rowid";

    // Three finished digests: round one, round two, and a run without a round.
    // Each is awaited in the store before the next, so their `at` order is the
    // order they were sent in.
    h.send(send_json(
        "/keeper",
        &json!({"round": R1, "text": "Daily digest:\nRound one news. More of it."}),
    ))
    .await;
    wait_rows(&db, all, 3, 1).await;
    h.send(send_json(
        "/keeper",
        &json!({"round": R2, "text": "Daily digest:\nRound two news."}),
    ))
    .await;
    wait_rows(&db, all, 3, 2).await;
    h.send(send_json(
        "/keeper",
        &json!({"round": "", "text": "Daily digest:\nNobody's news."}),
    ))
    .await;
    let kept = wait_rows(&db, all, 3, 3).await;

    let rounds: Vec<Option<String>> = kept.iter().map(|r| r[1].clone()).collect();
    assert_eq!(
        rounds,
        vec![
            Some(R1_KEPT.to_string()),
            Some(R2_KEPT.to_string()),
            Some(String::new())
        ],
        "every digest is kept with the canonical round of its run, none widened: {kept:?}"
    );
    let r1_id = kept[0][0].clone().expect("an id");
    let r2_id = kept[1][0].clone().expect("an id");

    // A read of round one at the rim: exactly the round-one digest.
    h.send(send_json(
        "/asker",
        &json!({"hive": "digest", "op": "last", "op_id": "res:t1", "round": R1}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:t1").await;
    assert_eq!(a["ok"], true, "{a}");
    assert_eq!(a["op"], "last", "{a}");
    assert_eq!(leads(&a["digests"], "id"), vec![r1_id.clone()], "{a}");
    assert_eq!(a["digests"][0]["lead"], "Round one news.", "{a}");
    assert_eq!(
        a["digests"][0]["audience_set"],
        json!(["m-a", "m-b"]),
        "{a}"
    );

    // Round two: exactly the round-two digest.
    h.send(send_json(
        "/asker",
        &json!({"hive": "digest", "op": "last", "op_id": "res:t2", "round": R2}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:t2").await;
    assert_eq!(a["ok"], true, "{a}");
    assert_eq!(leads(&a["digests"], "id"), vec![r2_id], "{a}");

    // No round: no row is read.
    h.send(send_json(
        "/asker",
        &json!({"hive": "digest", "op": "last", "op_id": "res:t3", "round": ""}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:t3").await;
    assert_eq!(a["ok"], false, "{a}");
    assert_eq!(a["error"]["code"], "no_round", "{a}");
    assert!(
        a.get("digests").is_none(),
        "a read without a round reads no row: {a}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_research_answer_is_kept_with_its_round_and_shown_only_to_it() {
    if !library_ships() || !have_python() {
        return;
    }
    let td = tempfile::tempdir().unwrap();
    build_tree(&td);
    let (h, mut rx) = boot(&td).await;
    let db = td.path().join("main/research/memory/cell.db");
    let ours = "SELECT question_id, audience_set, at FROM answers \
                WHERE question_id = 'turn-r1' ORDER BY rowid";

    h.send(send_json(
        "/archiver",
        &json!({"question": "What changed today?", "turn_id": "turn-r1", "round": R1,
                "text": "Three things changed today."}),
    ))
    .await;
    let kept = wait_rows(&db, ours, 3, 1).await;
    assert_eq!(kept.len(), 1, "one answer kept: {kept:?}");
    assert_eq!(kept[0][1].as_deref(), Some(R1_KEPT), "{kept:?}");
    assert!(
        kept[0][2]
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|at| at > 0),
        "at is a positive int: {kept:?}"
    );

    // A read of round one at the rim: that answer, with its question id.
    h.send(send_json(
        "/asker",
        &json!({"hive": "research", "op": "last", "op_id": "res:q1", "round": R1}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:q1").await;
    assert_eq!(a["ok"], true, "{a}");
    assert_eq!(leads(&a["answers"], "question_id"), vec!["turn-r1"], "{a}");
    assert!(
        a["answers"][0]["at"].as_i64().is_some_and(|at| at > 0),
        "{a}"
    );
    assert_eq!(
        a["answers"][0]["answer"], "Three things changed today.",
        "{a}"
    );
    assert_eq!(
        a["answers"][0]["audience_set"],
        json!(["m-a", "m-b"]),
        "{a}"
    );

    // Another round sees nothing of it.
    h.send(send_json(
        "/asker",
        &json!({"hive": "research", "op": "last", "op_id": "res:q2", "round": R2}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:q2").await;
    assert_eq!(a["ok"], true, "{a}");
    assert_eq!(a["answers"], json!([]), "another round reads nothing: {a}");

    // And a read without a round reads no row.
    h.send(send_json(
        "/asker",
        &json!({"hive": "research", "op": "last", "op_id": "res:q3", "round": ""}),
    ))
    .await;
    let a = answer_for(&h, &mut rx, "res:q3").await;
    assert_eq!(a["error"]["code"], "no_round", "{a}");

    // The insert receipt of `turn-r1` left `./memory` before the select of
    // `res:q1` was even taken (one mailbox, in order), and that select's answer
    // came back through the rim above: the receipt has been routed. It reaches
    // `./archive` over `./memory -> ./archive` and ends there -- nothing from
    // `.../memory` is dead-lettered (formerly `no_route`, there is no reply_to
    // fallback).
    let from_memory: Vec<String> = h
        .drain_dead_letters()
        .await
        .iter()
        .filter(|d| d.sender_path.as_str().ends_with("/memory"))
        .map(|d| {
            format!(
                "{} -> {} ({:?})",
                d.sender_path.as_str(),
                d.original_target.as_str(),
                d.reason
            )
        })
        .collect();
    assert!(
        from_memory.is_empty(),
        "the archive's insert receipt reaches ./archive, no dead letter (GH #976, OR-NL-158): \
         {from_memory:?}"
    );

    h.shutdown().await;
}
