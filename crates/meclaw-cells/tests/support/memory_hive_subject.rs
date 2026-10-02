//! What every GH #948 lock needs: the shipped memory hive booted alone with
//! its rim captured, a store seeded with facts, the two new requests, and the
//! scripts run bare for their pure functions.
//!
//! The colony is the receiver the issue is stated at: the door edge, the
//! writer's and recall's store round trips and the exit edges are the shipped
//! files, the store is the real store cell on a real `cell.db`, and every
//! answer is read where the caller would read it -- on the hive's rim. Free of
//! a provider by construction: no lane of #948 reaches a model, every timer is
//! parked, and every `llm` cell points at a port nobody listens on.
//!
//! R2b: everything read at run time lies under `templates/memory-hive`, which
//! travels with the public export (the memory-hive tests read nothing else,
//! `make_export.py` history of GH #137); a tree without it is skipped, never
//! judged (GH #49).
#![allow(dead_code)]

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const DEADLINE: Duration = Duration::from_secs(30);
pub const HIVE: &str = "/memory-hive";
pub const WRITER: &str = "../../templates/memory-hive/writer/config.json";
pub const RECALL: &str = "../../templates/memory-hive/recall/config.json";
pub const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";

pub const ROUND_EA: &str = r#"["agent:a","member:e"]"#;
pub const ROUND_EB: &str = r#"["agent:b","member:e"]"#;

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

pub fn shipped() -> bool {
    [
        "templates/memory-hive/config.json",
        "templates/memory-hive/writer/config.json",
        "templates/memory-hive/recall/config.json",
        "templates/memory-hive/store/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

pub fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

// ─────────────────────────────────────────────────────────── the bare scripts

/// Run a shipped script with a probe program appended -- the script runs over
/// an empty message first (it parks), then the program calls its functions.
pub fn probe(config: &str, program: &str) -> String {
    let script = meclaw_testing::shipped_script(config);
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}\n"
        ),
        meclaw_core::serde_json::to_string(&script).expect("script"),
        program
    );
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
        sink.write_all(src.as_bytes()).expect("write");
    }
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The source of one top-level `def` (or of one top-level assignment) of a
/// script, its own first line included.
pub fn block_of(script: &str, head: &str) -> String {
    let at = script
        .find(&format!("\n{head}"))
        .unwrap_or_else(|| panic!("no top-level `{head}`"));
    let rest = &script[at + 1..];
    let mut lines = rest.lines();
    let mut out = vec![lines.next().expect("the head line").to_string()];
    for line in lines {
        let continues = line.is_empty()
            || line.starts_with(' ')
            || line.starts_with('\t')
            || line.starts_with(')');
        if !continues {
            break;
        }
        out.push(line.to_string());
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Every emission of a cell judged by the cell's OWN declared `emits`
/// contract, compiled and applied the way the substrate applies it in a debug
/// build (`resolve_validate_emits`).
pub fn assert_the_declaration_admits(config: &str, out: &[Value]) {
    let cfg = read_json(std::path::Path::new(config));
    let block: meclaw_colony::config::ContractBlock =
        meclaw_core::serde_json::from_value(cfg["contract"].clone())
            .expect("the contract block parses");
    let compiled =
        meclaw_core::CompiledEmits::compile(&block.emits).expect("the emits schemas compile");
    assert!(!out.is_empty(), "nothing was emitted at all");
    for m in out {
        meclaw_core::validate_emits(m, &compiled).unwrap_or_else(|e| {
            panic!("{config}: the cell's own contract refuses what it emits: {e} -- {m}")
        });
    }
}

// ─────────────────────────────────────────────────────────────── the colony

/// One fact row as the seed file carries it; `canonical_subject` is the store
/// normal form the store itself would have derived from `subject`.
pub fn fact(id: &str, subject: &str, canonical: &str, audience: &str, at: &str) -> Value {
    json!({
        "id": id,
        "episode_id": format!("ep-{id}"),
        "session_id": "s-948",
        "channel": "c-948",
        "audience_set": audience,
        "subject": subject,
        "canonical_subject": canonical,
        "predicate": "has_note",
        "canonical_predicate": "has_note",
        "claim": format!("note {id}"),
        "canonical_claim": format!("note {id}"),
        "claim_hash": "",
        "fact_kind": "world",
        "valid_from": at,
        "valid_until": null,
        "recorded_at": at,
        "expired_at": null,
        "superseded_by": null,
        "closure_source": "",
        "confidence": 70,
        "source": "",
    })
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_tree(&from, &dst.join(name));
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

/// Every `llm` cell on a port nobody listens on, every timer parked: nothing
/// in #948 may reach a model, and a call that tried would fail loudly.
fn quiet(main: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut n: u64 = 0;
    for f in files {
        let mut cfg = read_json(&f);
        match cfg["cell"]["type"].as_str() {
            Some("llm") => {
                cfg["params"]["base_url"] = json!("http://127.0.0.1:9");
                cfg["params"]["model"] = json!("no-model");
                cfg["params"]["api_key"] = json!("sk-test");
            }
            Some("timer") => {
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
                            json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0948_0000 + n));
                    }
                    if s.get("cron").is_some() {
                        s["cron"] = json!(NEVER_CRON);
                    }
                }
            }
            _ => continue,
        }
        write_json(&f, &cfg);
    }
}

/// Every `${VAR}` of the tree bound to a dummy.
fn write_env(root: &std::path::Path, main: &std::path::Path) {
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
    vars.insert("MEMORY_LLM_BASE_URL".into(), "http://127.0.0.1:9".into());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        "http://127.0.0.1:9/v1/embeddings".into(),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes the memory hive declares at its own path.
fn rim_emits() -> Vec<String> {
    read_json(&repo("templates/memory-hive/config.json"))["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The memory hive alone, every lane of its rim drained to `/out`, the store
/// seeded with `facts`. The alias table is store-owned and not declared in the
/// schema, so the store's seed loader never reads a file for it: a binding the
/// night made is written with [`store_op`] once the store is awake.
pub fn build(td: &tempfile::TempDir, facts: &[Value]) {
    let root = td.path();
    let main = root.join("main");
    copy_tree(&repo("templates/memory-hive"), &main.join("memory-hive"));
    let edges: Vec<Value> = rim_emits()
        .into_iter()
        .map(|lane| {
            json!({"from": "./memory-hive", "to": "/out",
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect();
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet(&main);
    write_env(root, &main);
    let store = main.join("memory-hive/store");
    let schema = read_json(&store.join("config.json"))["params"]["schema"].clone();
    let body: String = std::iter::once(json!({"schema": schema["facts"].clone()}))
        .chain(facts.iter().cloned())
        .map(|r| meclaw_core::serde_json::to_string(&r).expect("serialise") + "\n")
        .collect();
    std::fs::create_dir_all(store.join("seed")).expect("mkdir seed");
    std::fs::write(store.join("seed/facts.jsonl"), body).expect("seed facts");
}

pub async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
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
    let (tx, rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/out"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped memory hive must boot");
    (h, rx)
}

pub fn db(td: &tempfile::TempDir) -> std::path::PathBuf {
    td.path().join("main/memory-hive/store/cell.db")
}

/// `in_alias` as another part of the colony sends it: no round (the alias
/// table has none), the correlation token on the hop.
pub fn in_alias(body: Value, tag: &str) -> Message {
    let mut b = as_map(&body);
    b.entry("messages").or_insert(json!([]));
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_alias", "alias_tag": tag})))
        .context(Map::new())
        .body(Body::Inline(Value::Object(b)))
        .build()
}

/// `in_query {subject}` as the asking hive sends it: its round in
/// `audience_now` (None = no round at all), its reply-to token.
pub fn in_subject(body: Value, round: Option<&str>) -> Message {
    let mut b = as_map(&body);
    b.entry("messages").or_insert(json!([]));
    let mut ctx = json!({"recall_caller": "objects"});
    if let Some(r) = round {
        ctx["audience_now"] = json!(r);
    }
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_query"})))
        .context(as_map(&ctx))
        .body(Body::Inline(Value::Object(b)))
        .build()
}

pub fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

pub fn hop_str(m: &Message, key: &str) -> String {
    match m.headers.hop.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// The next message on the rim whose route is `route` and that `pick` accepts;
/// everything else that arrives first is kept in `seen`.
pub async fn next_on(
    rx: &mut mpsc::Receiver<Message>,
    seen: &mut Vec<Message>,
    route: &str,
    pick: impl Fn(&Message) -> bool,
) -> Message {
    if let Some(i) = seen
        .iter()
        .position(|m| hop_str(m, "route") == route && pick(m))
    {
        return seen.remove(i);
    }
    let deadline = Instant::now() + DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Some(m)) = tokio::time::timeout(left, rx.recv()).await else {
            let routes: Vec<String> = seen.iter().map(|m| hop_str(m, "route")).collect();
            panic!("no `{route}` on the rim within {DEADLINE:?}; seen: {routes:?}");
        };
        if hop_str(&m, "route") == route && pick(&m) {
            return m;
        }
        seen.push(m);
    }
}

/// The answer to one `in_alias`, matched by its tag.
pub async fn ack(rx: &mut mpsc::Receiver<Message>, seen: &mut Vec<Message>, tag: &str) -> Message {
    let tag = tag.to_string();
    next_on(rx, seen, "alias_ack", move |m| {
        hop_str(m, "alias_tag") == tag
    })
    .await
}

/// The answer to one subject question: its bundle (or its refusal), matched by
/// the subject it names.
pub async fn subject_answer(
    rx: &mut mpsc::Receiver<Message>,
    seen: &mut Vec<Message>,
    subject: &str,
) -> Value {
    let subject = subject.to_string();
    let m = next_on(rx, seen, "bundle", move |m| {
        bundle_json(m).is_some_and(|b| b["subject"] == subject.as_str())
    })
    .await;
    assert_eq!(
        hop_str(&m, "recall_caller"),
        "objects",
        "the reply-to token rides home"
    );
    bundle_json(&m).expect("a bundle")
}

/// The machine half of a bundle: `system.memory.bundle.text`, parsed.
pub fn bundle_json(m: &Message) -> Option<Value> {
    let text = body_of(m)["system"]["memory"]["bundle"]["text"].as_str()?;
    meclaw_core::serde_json::from_str(text).ok()
}

/// The claims of a subject answer, in the order the bundle lists them.
pub fn claims(bundle: &Value) -> Vec<String> {
    bundle["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| c["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

pub fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .unwrap_or_default()
}

/// `id -> canonical_subject` of every fact the store holds.
pub fn canonical_subjects(db: &std::path::Path) -> BTreeMap<String, String> {
    rows(db, "SELECT id, COALESCE(canonical_subject, '') FROM facts")
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}

/// One op through the store's OWN dispatcher, with the shipped store's
/// canonical bindings, on the colony's `cell.db` -- the code path the store
/// cell runs, on a second connection. Used for what no lane of the hive lets a
/// test say directly: a fact written after a binding (once the colony has
/// stopped), and a binding the nightly identity round made (while it idles).
pub fn store_op(db: &std::path::Path, args: Value) -> meclaw_cells::store::ops::OpOutcome {
    let params = read_json(&repo("templates/memory-hive/store/config.json"))["params"].clone();
    let parsed =
        meclaw_cells::store::params::StoreParams::parse(&params).expect("the store params parse");
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    conn.busy_timeout(DEADLINE).expect("busy timeout");
    // The connection-scoped functions the store factory registers on its own
    // connection (`store/factory.rs`): the facts table carries an FTS index
    // that names the stemming tokenizer, and a normalising binding derives
    // through `meclaw_norm`.
    meclaw_cells::store::query::hamming::register(&conn).expect("hamming");
    meclaw_cells::store::query::normalize::register(&conn).expect("meclaw_norm");
    meclaw_cells::store::query::fts_tokenizer::register(&conn).expect("fts tokenizer");
    meclaw_cells::store::ops::dispatch_with(&conn, &args, &parsed.canonical)
        .expect("the op is well formed")
}
