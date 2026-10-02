//! A graph space on a booted colony, for the locks of GH #945. Two layouts:
//!
//! - **member** -- the member reduced to the two holders this road touches:
//!   its `./file-space` and its `./graph-space`, both the SHIPPED templates
//!   resolved the way the mutation door lays a `ref` out, and the member's own
//!   edges between them drawn VERBATIM off `templates/member/config.json` (the
//!   three of GH #945: `source_changed` with `restore_ttl`, `pull` restamped to
//!   `in_read`, the `gs:` answer restamped to `in_pulled`). Everything else
//!   that leaves either hive is drained to a capture: the space's `answer` and
//!   `derived` to `fsink`, its `tool_result` to `tsink`, its `source_changed`
//!   additionally to `seen` (a second edge, so the run can say an announcement
//!   happened), the graph's `answer` to `gsink`, every other lane to `park`.
//!   Files are written through the space's `in_tool` door as the core would
//!   (`context.tool_caller` 'cogny'), so the index must arrive although the
//!   writer's answers travel under `caller` 'tools'. The space's `./embed`
//!   talks to a local embeddings stub, every `llm` cell to a local chat stub.
//! - **alone** -- the graph space by itself; its `pull` goes to the capture
//!   `pulls` and the test plays the source, answering on `in_pulled` in any
//!   order it chooses.
//!
//! Measured at the receivers: the graph space's own `cell.db`, the colony's
//! `message_log`, the captures. No paid provider is reachable by
//! construction. Guarded like every template-reading test (GH #49).
#![allow(dead_code)]

use crate::mock_openai::{MockOpenAI, canned_chat_completion};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
pub const DEADLINE: Duration = Duration::from_secs(60);
/// How long the colony must stay silent before a run calls it quiet.
pub const QUIET: Duration = Duration::from_millis(1500);
pub const BACKGROUND_REPLY: &str = "A short account of the file.";

// ═══════════════════════════════════════════════════════════════ the shipped tree

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file boots or reads. One missing = skipped (GH #49).
pub fn shipped() -> bool {
    ["graph-space", "file-space", "member"]
        .iter()
        .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
        && repo("templates/member/graph-space/config.json").is_file()
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

pub fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write");
}

/// The shipped template, copied the way instantiation lays it out: a ref
/// marker is replaced by the referenced template's tree and its
/// `override_params` are applied to the cells they name.
pub fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: a ref names a template", marker.display()));
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every `llm` cell of the tree at the local chat stub.
fn point_llms_at(main: &std::path::Path, url: &str) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!("background-stub");
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// Every timer of the tree out of the run's way.
fn quiet_timers(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "timer" {
            continue;
        }
        let Some(schedules) = cfg["params"]["schedules"].as_array_mut() else {
            continue;
        };
        for s in schedules.iter_mut() {
            n += 1;
            if s["schedule_id"]
                .as_str()
                .is_some_and(|id| id.contains("${"))
            {
                s["schedule_id"] =
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0945_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` without a default bound to a
/// dummy, the embeddings endpoint the space's `./embed` reads bound to the stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, llm: &str, embed: &str) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    vars.insert("MEMORY_LLM_BASE_URL".into(), llm.to_string());
    vars.insert("MEMORY_EMBED_ENDPOINT".into(), embed.to_string());
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// An OpenAI-compatible embeddings endpoint on 127.0.0.1: one vector of the
/// asked `dimensions` per input, a pure function of the input's length.
pub struct EmbedStub {
    pub url: String,
    pub calls: Arc<AtomicUsize>,
}

pub fn embed_stub() -> EmbedStub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the embeddings stub");
    let url = format!(
        "http://{}/v1/embeddings",
        listener.local_addr().expect("a bound address")
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Ok(half) = conn.try_clone() else { continue };
            let mut reader = BufReader::new(half);
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let req: Value = sj::from_slice(&body).unwrap_or(Value::Null);
            let dim = req["dimensions"].as_u64().unwrap_or(64) as usize;
            let data: Vec<Value> = req["input"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let n = t.as_str().map_or(0, str::len);
                    let v: Vec<f64> = (0..dim)
                        .map(|k| if (k + n) % 3 == 0 { 1.0 } else { -1.0 })
                        .collect();
                    json!({"index": i, "embedding": v})
                })
                .collect();
            seen.fetch_add(1, Ordering::SeqCst);
            let out = json!({"data": data, "model": "embed-stub",
                             "usage": {"prompt_tokens": 3}})
            .to_string();
            let _ = write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    EmbedStub { url, calls }
}

// ══════════════════════════════════════════════════════════════════ the layouts

/// The lanes a template declares at its own path.
pub fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The routes a code cell of the graph space declares it emits.
pub fn cell_routes(cell: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/graph-space/{cell}/config.json")))["contract"]["emits"]
        ["hop"]["route"]["values"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The member's own edges between its file space and its graph space, read
/// off the shipped member and drawn as they stand. Exactly three (GH #945).
/// The librarian's two edges to and from the graph space (GH #950) and the
/// objects' three (GH #951: `source_changed`, the `ob-` pull, the `gs:`
/// answer) are their own roads and not drawn here -- but every `./graph-space`
/// edge of the member runs to `./file-space`, `./librarian` or `./objects` and
/// nowhere else: the graph space answers what it holds without a round filter
/// for files (an object's nodes it filters by the object's round), so a fourth
/// neighbour would read the space past its door (GH #945, GH #950, OR-BC-72).
pub fn member_graph_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    let all: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for e in &all {
        let other = if e["from"] == json!("./graph-space") {
            &e["to"]
        } else if e["to"] == json!("./graph-space") {
            &e["from"]
        } else {
            continue;
        };
        assert!(
            other == &json!("./file-space")
                || other == &json!("./librarian")
                || other == &json!("./objects"),
            "every `./graph-space` edge runs to the file space, the librarian or the objects: {e}"
        );
    }
    let edges: Vec<Value> = all
        .into_iter()
        .filter(|e| {
            let (from, to) = (&e["from"], &e["to"]);
            (from == &json!("./graph-space") && to == &json!("./file-space"))
                || (from == &json!("./file-space") && to == &json!("./graph-space"))
        })
        .collect();
    assert_eq!(
        edges.len(),
        3,
        "the member draws three edges between `./file-space` and `./graph-space` -- \
         `source_changed` in, `pull` out, the `gs:` answer back: {edges:#?}"
    );
    edges
}

fn drain(from: &str, cond: &str, to: &str) -> Value {
    json!({"from": from, "to": to, "condition": cond})
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Member,
    Alone,
}

pub struct Stubs {
    pub llm: MockOpenAI,
    pub embed: EmbedStub,
}

pub async fn stubs() -> Stubs {
    let llm = MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    Stubs {
        llm,
        embed: embed_stub(),
    }
}

pub fn build(td: &tempfile::TempDir, layout: Layout, stubs: &Stubs) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/member/graph-space"),
        &main.join("graph-space"),
        0,
    );
    let mut edges: Vec<Value> = Vec::new();
    for lane in rim_emits("graph-space") {
        match lane.as_str() {
            "answer" => edges.push(drain(
                "./graph-space",
                "has(hop.route) && hop.route == 'answer'",
                "/gsink",
            )),
            "pull" if layout == Layout::Alone => edges.push(drain(
                "./graph-space",
                "has(hop.route) && hop.route == 'pull'",
                "/pulls",
            )),
            "pull" => {}
            other => edges.push(drain(
                "./graph-space",
                &format!("has(hop.route) && hop.route == '{other}'"),
                "/park",
            )),
        }
    }
    if layout == Layout::Member {
        copy_resolved(
            &repo("templates/member/file-space"),
            &main.join("file-space"),
            0,
        );
        edges.extend(member_graph_edges());
        for lane in rim_emits("file-space") {
            let cond = format!("has(hop.route) && hop.route == '{lane}'");
            match lane.as_str() {
                "answer" => edges.push(drain(
                    "./file-space",
                    "has(hop.route) && hop.route == 'answer' && \
                     !(has(hop.op_id) && hop.op_id.startsWith('gs:'))",
                    "/fsink",
                )),
                "derived" => edges.push(drain("./file-space", &cond, "/fsink")),
                "tool_result" => edges.push(drain("./file-space", &cond, "/tsink")),
                "source_changed" => edges.push(drain("./file-space", &cond, "/seen")),
                _ => edges.push(drain("./file-space", &cond, "/park")),
            }
        }
    }
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    point_llms_at(&main, &stubs.llm.base_url);
    write_env(root, &main, &stubs.llm.base_url, &stubs.embed.url);
}

pub struct Ports {
    pub fsink: mpsc::Receiver<Message>,
    pub tsink: mpsc::Receiver<Message>,
    pub gsink: mpsc::Receiver<Message>,
    pub seen: mpsc::Receiver<Message>,
    pub pulls: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    pub park: mpsc::Receiver<Message>,
}

pub async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
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
    let mut rx = Vec::new();
    for name in ["/fsink", "/tsink", "/gsink", "/seen", "/pulls", "/park"] {
        let (tx, r) = mpsc::channel::<Message>(4096);
        h.spawn(Path::new(name), move || CaptureCell::new(tx.clone()))
            .await;
        rx.push(r);
    }
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped graph space (and file space) must boot");
    let mut it = rx.into_iter();
    let mut next = || it.next().expect("a receiver");
    let ports = Ports {
        fsink: next(),
        tsink: next(),
        gsink: next(),
        seen: next(),
        pulls: next(),
        park: next(),
    };
    (h, ports)
}

// ════════════════════════════════════════════════════════════════ the messages

pub fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn message(to: &str, hop: Value, ctx: Value, body: Value) -> Message {
    MessageBuilder::new(Path::new(to))
        .hop(map(hop))
        .context(map(ctx))
        .body(Body::Inline(body))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// One tool call at the space's `in_tool`, stamped as the core's.
pub fn tool_call(name: &str, id: &str, args: &Value) -> Message {
    message(
        "/file-space",
        json!({"route": "in_tool", "tool_name": name, "tool_call_id": id}),
        json!({"tool_caller": "cogny"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": args.to_string()}]}),
    )
}

/// A question at the graph space's `in_graph`.
pub fn question(op: &str, op_id: &str, args: Value) -> Message {
    message(
        "/graph-space",
        json!({"route": "in_graph", "op": op, "op_id": op_id}),
        json!({}),
        json!({"op": op, "args": args, "messages": []}),
    )
}

/// An announcement at the graph space, as a source sends it.
pub fn announce(
    source: &str,
    version: &str,
    path: &str,
    nodes: u64,
    links: u64,
    tomb: bool,
) -> Message {
    message(
        "/graph-space",
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": source, "version": version, "path": path, "fmt": "", "parser": "test",
               "mark": "", "nodes": nodes, "links": links, "tomb": if tomb { "1" } else { "" },
               "messages": []}),
    )
}

/// A removal, in the file space's own form: `{source, path, tomb: true}` and
/// no version (OR-BC-55 (1)).
pub fn removal(source: &str, path: &str) -> Message {
    message(
        "/graph-space",
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": source, "path": path, "tomb": true, "messages": []}),
    )
}

/// A source's answer to one pull, as the member's edge restamps it.
pub fn pulled(pull: &Message, body: Value) -> Message {
    let op_id = hop_str(pull, "op_id");
    let op = hop_str(pull, "op");
    let mut b = body;
    b["ok"] = json!(true);
    b["op"] = json!(op);
    b["op_id"] = json!(op_id);
    b["messages"] = json!([]);
    message(
        "/graph-space",
        json!({"route": "in_pulled", "op": op, "op_id": op_id}),
        json!({}),
        b,
    )
}

pub fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

pub fn hop_str(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The JSON a tool result carries as the text of its one turn.
pub fn tool_answer(m: &Message) -> Value {
    let t = body_of(m)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    sj::from_str(&t).unwrap_or_else(|e| panic!("a tool result's text is JSON ({e}): {t}"))
}

// ═══════════════════════════════════════════════════════════════════ the readers

/// One routing hop as the colony logged it.
#[derive(Debug, Clone)]
pub struct Logged {
    pub rowid: i64,
    pub id: String,
    pub parent: Option<String>,
    pub from: String,
    pub to: String,
    pub hop: Value,
    pub ttl: i64,
}

impl Logged {
    pub fn route(&self) -> String {
        self.hop["route"].as_str().unwrap_or_default().to_string()
    }
    pub fn say(&self) -> String {
        format!(
            "ttl={:3} {} -> {} [{}]",
            self.ttl,
            self.from,
            self.to,
            self.route()
        )
    }
}

pub fn message_log(root: &std::path::Path) -> Vec<Logged> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT rowid, id, parent_message_id, from_path, to_path, headers, ttl \
             FROM message_log ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            r.get::<_, i64>(6)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(rowid, id, parent, from, to, headers, ttl)| {
        let h: Value = sj::from_str(&headers).unwrap_or(Value::Null);
        Logged {
            rowid,
            id,
            parent,
            from,
            to,
            hop: h["hop"].clone(),
            ttl,
        }
    })
    .collect()
}

/// `(error_code, sender, target, head)` of every dead letter of the run.
pub fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT error_code, sender_path, resolved_target, message_json \
             FROM dead_letters ORDER BY id",
        )
        .expect("dead_letters");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(code, sender, target, msg)| (code, sender, target, msg.chars().take(600).collect()))
    .collect()
}

/// The graph space's own store.
pub fn graph_db(root: &std::path::Path) -> std::path::PathBuf {
    root.join("main/graph-space/store/cell.db")
}

/// Every row of one query against a `cell.db`, each column as text; a store
/// that has not woken yet has no table, which is the honest "nothing yet".
pub fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    let out: Vec<Vec<String>> = st
        .query_map([], |r| {
            Ok((0..n)
                .map(|i| match r.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Text(t)) => {
                        String::from_utf8_lossy(t).to_string()
                    }
                    Ok(rusqlite::types::ValueRef::Integer(x)) => x.to_string(),
                    Ok(rusqlite::types::ValueRef::Real(x)) => x.to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<String>>())
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    out
}

/// Wait until `cond` holds; on the deadline, panic with what the store and
/// the colony hold.
pub async fn wait_for(root: &std::path::Path, what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !cond() {
        if Instant::now() >= deadline {
            panic!(
                "{what}: not within {DEADLINE:?}. Edges: {:#?}\nSources: {:#?}\nDead letters: {:#?}",
                rows(
                    &graph_db(root),
                    "SELECT from_addr, kind, target_name, state, to_addr, class FROM edges"
                ),
                rows(
                    &graph_db(root),
                    "SELECT source, path, module, version, announced, tomb FROM sources"
                ),
                dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Wait until the colony logged nothing new for `QUIET`.
pub async fn quiet(root: &std::path::Path) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = message_log(root).len();
    let mut since = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = message_log(root).len();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= QUIET {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the colony did not go quiet within {DEADLINE:?}"
        );
    }
}

/// The next message on `rx` that `hit` accepts; the others are kept in `seen`.
pub async fn next_matching(
    rx: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    what: &str,
    hit: impl Fn(&Message) -> bool,
    seen: &mut Vec<Message>,
) -> Message {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(m)) => {
                if hit(&m) {
                    return m;
                }
                seen.push(m);
            }
            _ => panic!(
                "{what}: nothing arrived within {DEADLINE:?}. Seen: {:#?}. Dead letters: {:#?}",
                seen.iter()
                    .map(|m| (Value::Object(m.headers.hop.clone()), body_of(m).clone()))
                    .collect::<Vec<_>>(),
                dead_letters(root)
            ),
        }
    }
}

/// Ask the graph one question and return the body of its one answer.
pub async fn ask(
    h: &ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    op: &str,
    op_id: &str,
    args: Value,
) -> Value {
    h.send(question(op, op_id, args)).await;
    let mut seen = Vec::new();
    let m = next_matching(
        &mut ports.gsink,
        root,
        &format!("the answer to `{op}` ({op_id})"),
        |m| hop_str(m, "route") == "answer" && hop_str(m, "op_id") == op_id,
        &mut seen,
    )
    .await;
    assert_eq!(hop_str(&m, "op"), op, "the answer mirrors `op`");
    body_of(&m).clone()
}

/// Write one file through the space's tool door as the core; returns
/// `(fh-id, version)` off the tool result.
pub async fn create(
    h: &ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    path: &str,
    text: &str,
) -> (String, String) {
    let id = format!("create-{}", path.replace('/', "-"));
    h.send(tool_call(
        "file_create",
        &id,
        &json!({"path": path, "text": text}),
    ))
    .await;
    let mut seen = Vec::new();
    let m = next_matching(
        &mut ports.tsink,
        root,
        &format!("the tool result of creating {path}"),
        |m| hop_str(m, "route") == "tool_result" && hop_str(m, "tool_call_id") == id,
        &mut seen,
    )
    .await;
    let a = tool_answer(&m);
    assert_eq!(a["ok"], json!(true), "{path} was not created: {a}");
    let file = a["file"].as_str().unwrap_or_default().to_string();
    let version = a["version"].as_str().unwrap_or_default().to_string();
    assert!(file.starts_with("fh-"), "{a}");
    (file, version)
}

/// Every delivery out of a code cell of the graph space carries a route its
/// contract declares (`contract.emits` in the run, not on paper).
pub fn emissions_within_contract(log: &[Logged]) {
    for cell in ["index", "query"] {
        let declared = cell_routes(cell);
        let from = format!("/graph-space/{cell}");
        for r in log.iter().filter(|r| r.from == from) {
            assert!(
                declared.contains(&r.route()),
                "{from} emitted `{}`, which its contract does not declare ({declared:?}): {}",
                r.route(),
                r.say()
            );
        }
    }
}

/// Walk the parent chain of `start` back to the first delivery `seam`
/// accepts; the chain from the seam to `start`, or None. A cell's emission
/// names the input it handled as its parent, and a hive transit names the
/// message it forwards (the walk of `support/gh929_member_road.rs`).
pub fn chain_to_seam<'a>(
    log: &'a [Logged],
    start: &'a Logged,
    seam: impl Fn(&Logged) -> bool,
) -> Option<Vec<&'a Logged>> {
    let by_id: BTreeMap<&str, &Logged> = log.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = vec![start];
    let mut at = start;
    while !seam(at) {
        at = at.parent.as_deref().and_then(|p| by_id.get(p)).copied()?;
        out.push(at);
        if out.len() > 4 * MESSAGE_DEFAULT_TTL as usize {
            return None;
        }
    }
    out.reverse();
    Some(out)
}
