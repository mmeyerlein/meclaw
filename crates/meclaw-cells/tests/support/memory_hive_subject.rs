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
    probe_with(
        config,
        &json!({"envelope": {}, "body": {}, "params": {}}),
        program,
    )
}

/// `probe` inside a session: a lane that parks a hop without a session
/// (`close-glue`) stops before its later functions are defined, so a probe of
/// one of those needs the session on the envelope.
pub fn probe_in_session(config: &str, program: &str) -> String {
    probe_with(
        config,
        &json!({"envelope": {"header": {"context": {"session_id": "s-probe"}}},
                "body": {}, "params": {}}),
        program,
    )
}

fn probe_with(config: &str, input: &Value, program: &str) -> String {
    let script = meclaw_testing::shipped_script(config);
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
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
        meclaw_core::serde_json::to_string(&input.to_string()).expect("input"),
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

// ──────────────────────────── GH #1423 (M7-C): both voices at the close, bare

/// The session every M7-C probe closes.
pub const M7C_SESSION: &str = "s-m7c";
/// The member's turn that passes a claim on, and the assistant's reply to it
/// (`M7A_EPISODE`, so the reply's block also arrives at the M7-A ingress).
pub const M7C_SAID: &str = "ep-said";
pub const M7C_REPLY: &str = M7A_EPISODE;

/// Run close-glue's shipped script with its shipped params to its end (a
/// phase it does not know: every helper defined, nothing emitted), then
/// `program`, which ends in an `emit` or prints JSON. No colony, no model.
pub fn m7c_probe(program: &str) -> Value {
    let script = meclaw_testing::shipped_script(CLOSE_GLUE);
    let stdin = json!({
        "envelope": {"header": {"context": {"mem_phase": "m7c-probe",
                                            "session_id": M7C_SESSION}}},
        "body": {},
        "params": read_json(std::path::Path::new(CLOSE_GLUE))["params"].clone()
    });
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
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
        meclaw_core::serde_json::to_string(&stdin.to_string()).expect("stdin"),
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
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the probe printed no JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One turn of the round `M7A_AUDIENCE`: `sender` `user` (the member),
/// `assistant` (the companion) or `peer`.
pub fn m7c_turn(id: &str, sender: &str, content: &str, at: &str) -> Value {
    json!({"id": id, "session_id": M7C_SESSION, "turn_id": id, "sender": sender,
           "speaker": "", "content": content, "happened_at": at, "recorded_at": at,
           "channel": "direct:e", "audience_set": M7A_AUDIENCE})
}

/// One open fact of the round on the axis member / has_mole, said in `episode`
/// by `source` (`""` the member's own side).
pub fn m7c_fact(id: &str, episode: &str, claim: &str, source: &str) -> Value {
    json!({"id": id, "episode_id": episode, "subject": "member",
           "canonical_subject": "member", "predicate": "has_mole",
           "canonical_predicate": "has_mole", "claim": claim, "canonical_claim": claim,
           "fact_kind": "world", "confidence": 70, "valid_from": "2026-11-05T10:00:00Z",
           "recorded_at": "2026-11-05T10:00:01Z", "source": source,
           "audience_set": M7A_AUDIENCE})
}

/// The member passes the doctor's word on, the companion qualifies it.
pub fn m7c_turns() -> Vec<Value> {
    vec![
        m7c_turn(
            M7C_SAID,
            "user",
            "The doctor says the mole is harmless.",
            "2026-11-05T10:00:00Z",
        ),
        m7c_turn(
            M7C_REPLY,
            "assistant",
            "I would still have it looked at if it changes.",
            "2026-11-05T10:00:05Z",
        ),
    ]
}

/// A text as a Python string literal (JSON's spelling is one).
fn m7c_py(text: &str) -> String {
    meclaw_core::serde_json::to_string(text).expect("text")
}

/// The close pass of ONE round (`M7A_AUDIENCE`) over `turns` and `facts`
/// with the closer's `verdict` -- every emission, judged by the lane's own
/// declared contract.
pub fn m7c_close(turns: &[Value], facts: &[Value], verdict: Value) -> Vec<Value> {
    let program = format!(
        "apply_all([{{\"audience\": json.loads({aud}), \"turns\": json.loads({t})}}], 0, \
         {{0: json.loads({v})}}, json.loads({f}), [], [], 0)",
        aud = m7c_py(M7A_AUDIENCE),
        t = m7c_py(&Value::Array(turns.to_vec()).to_string()),
        v = m7c_py(&verdict.to_string()),
        f = m7c_py(&Value::Array(facts.to_vec()).to_string()),
    );
    let out = match m7c_probe(&program) {
        Value::Array(a) => a,
        other => vec![other],
    };
    assert_the_declaration_admits(CLOSE_GLUE, &out);
    out
}

/// The closer call of that round: (instructions, the prompt document).
pub fn m7c_ask(turns: &[Value], facts: &[Value]) -> (String, Value) {
    let program = format!(
        "ask_group([{{\"audience\": json.loads({aud}), \"turns\": json.loads({t})}}], 0, \
         json.loads({f}), [], [])",
        aud = m7c_py(M7A_AUDIENCE),
        t = m7c_py(&Value::Array(turns.to_vec()).to_string()),
        f = m7c_py(&Value::Array(facts.to_vec()).to_string()),
    );
    let out = m7c_probe(&program);
    let call = &out[0];
    let instructions = call["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions")
        .to_string();
    let doc: Value = meclaw_core::serde_json::from_str(
        call["messages"][0]["text"].as_str().expect("prompt text"),
    )
    .expect("the prompt is JSON");
    (instructions, doc)
}

/// The header of the pass's `close_report`.
pub fn m7c_report(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "close_report")
        .map(|m| m["header"].clone())
        .unwrap_or_else(|| panic!("no close_report: {out:?}"))
}

/// The blocks the pass hands the ingress (`close_write`), parsed.
pub fn m7c_blocks(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "close_write")
        .map(|m| {
            meclaw_core::serde_json::from_str(
                m["messages"][0]["text"].as_str().expect("block text"),
            )
            .expect("block json")
        })
        .collect()
}

/// The store ops the pass writes itself on `facts`, in order.
pub fn m7c_fact_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .map(op_args)
        .filter(|a| a["table"] == "facts")
        .collect()
}

// ─────────────────────────────────── GH #1421 (M7-A): the stance road, bare

/// The ingress the three new `facts` columns enter through.
pub const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";
/// The close lane, where a mute is written.
pub const CLOSE_GLUE: &str = "../../templates/memory-hive/close-glue/config.json";
/// The store whose `params.schema` declares the columns.
pub const STORE: &str = "../../templates/memory-hive/store/config.json";
/// The audience of every M7-A block: one member, one agent.
pub const M7A_AUDIENCE: &str = r#"["agent:a","member:e"]"#;
/// The turn every M7-A block speaks for.
pub const M7A_EPISODE: &str = "ep-m7a";

/// The arguments of the store op a lane message carries (`Null` for none).
pub fn op_args(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

/// One block of section `memory` arriving at the ingress, naming its turn --
/// per turn (`close_pass` false) or from the close pass (edge-stamped `"1"`).
/// Run on the shipped script with no colony and no model.
pub fn m7a_arrive(facts: Value, close_pass: bool) -> Vec<Value> {
    let mut context = json!({"mem_phase": "inline", "batch_id": "", "session_id": "s-m7a",
                             "audience_set": M7A_AUDIENCE, "channel": "direct:e"});
    if close_pass {
        context["close_pass"] = json!("1");
    }
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(EXTRACT),
        &json!({
            "header": {"context": context,
                       "hop": {"route": "sidecar", "section": "memory"}},
            "section": "memory",
            "payload": {"episode_id": M7A_EPISODE, "audience_set": M7A_AUDIENCE,
                        "facts": facts},
            "messages": []
        }),
    );
    // Every receipt is one the lane declares (`reject_reason` values).
    assert_the_declaration_admits(EXTRACT, &out);
    out
}

/// The facts the arrival staged for the dedup (`scratch` row of kind
/// `payload`), as the apply phase will read them back.
pub fn m7a_staged(out: &[Value]) -> Vec<Value> {
    out.iter()
        .map(op_args)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
        .map(|a| {
            let p: Value =
                meclaw_core::serde_json::from_str(a["row"]["payload"].as_str().unwrap_or("{}"))
                    .expect("the staged payload is JSON");
            p["facts"].as_array().cloned().unwrap_or_default()
        })
        .unwrap_or_default()
}

/// `(reject_reason, text)` of every receipt on `reject`.
pub fn m7a_rejects(out: &[Value]) -> Vec<(String, String)> {
    out.iter()
        .filter(|m| m["header"]["route"] == "reject")
        .map(|m| {
            (
                m["header"]["reject_reason"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                m["messages"][0]["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// The apply phase over `staged` facts, the turn `M7A_EPISODE` spoken by
/// `source` (`""` the member's own side, `agent:self` the agent's answer),
/// and an optional close-pass `window` -- what the store is asked to write.
pub fn m7a_apply(staged: &[Value], source: &str, window: Option<Value>) -> Vec<Value> {
    let eps = json!({M7A_EPISODE: {"happened_at": "2026-11-05T10:00:00Z",
                                   "session_id": "s-m7a", "channel": "direct:e",
                                   "audience_set": M7A_AUDIENCE, "source": source,
                                   "unattributed": false}});
    let mut rows = vec![
        json!({"key": "inline:m7a", "kind": "payload",
               "payload": json!({"facts": staged, "entities": [], "edges": []}).to_string()}),
        json!({"key": "inline:m7a", "kind": "known", "payload": "[]"}),
        json!({"key": "inline:m7a", "kind": "episodes", "payload": eps.to_string()}),
    ];
    if let Some(w) = window {
        rows.push(json!({"key": "inline:m7a", "kind": "window", "payload": w.to_string()}));
    }
    let context = json!({"mem_phase": "apply", "batch_id": "inline:m7a", "session_id": "s-m7a",
                         "audience_set": M7A_AUDIENCE, "channel": "direct:e"});
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(EXTRACT),
        &json!({
            "header": {"context": context, "hop": {"operation": "select", "rows_affected": 1}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": Value::Array(rows).to_string()}]
        }),
    )
}

/// The `facts` rows an apply emission inserts.
pub fn m7a_fact_rows(out: &[Value]) -> Vec<Value> {
    out.iter()
        .map(op_args)
        .filter(|a| a["operation"] == "insert" && a["table"] == "facts")
        .map(|a| a["row"].clone())
        .collect()
}

/// One fact of a block, `extra` merged over the plain item.
pub fn m7a_fact(claim: &str, extra: Value) -> Value {
    let mut f = json!({"subject": "member", "predicate": "has_mole", "claim": claim,
                       "fact_kind": "world", "episode_id": M7A_EPISODE});
    if let Some(m) = extra.as_object() {
        for (k, v) in m {
            f[k] = v.clone();
        }
    }
    f
}

// ───────────────────────────────────────────── § M7-D road (GH #1424, reading)

pub const DREAM_GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";

/// The round every M7-D row is recorded in, and the one a request asks in.
pub const M7D_ROUND: &str = r#"["agent:a","member:e"]"#;
/// A round the asking one is not a subset of.
pub const M7D_OTHER_ROUND: &str = r#"["agent:b","member:e"]"#;

/// The shipped params of a cell without its script, `over` merged on top --
/// what the colony hands the script, with the knobs a test turns.
pub fn m7d_params(config: &str, over: Value) -> Value {
    let mut p = read_json(std::path::Path::new(config))["params"].clone();
    let map = p.as_object_mut().expect("params");
    map.remove("script_inline");
    if let Some(o) = over.as_object() {
        for (k, v) in o {
            map.insert(k.clone(), v.clone());
        }
    }
    p
}

/// A fact row as the tier-1 hydration select returns it, `extra` merged over.
pub fn m7d_fact(id: &str, predicate: &str, claim: &str, day: &str, extra: Value) -> Value {
    let at = format!("{day}T09:00:00Z");
    let mut f = json!({"id": id, "session_id": "s-1", "subject": "user", "predicate": predicate,
                       "claim": claim, "canonical_subject": "user",
                       "canonical_predicate": predicate, "canonical_claim": claim,
                       "valid_from": at, "valid_until": null, "recorded_at": at,
                       "expired_at": null, "superseded_by": null,
                       "episode_id": format!("ep-of-{id}"), "fact_kind": "state",
                       "confidence": 90, "channel": "direct:e", "audience_set": M7D_ROUND,
                       "source": "", "stance": "", "via": "", "answers": ""});
    if let Some(m) = extra.as_object() {
        for (k, v) in m {
            f[k] = v.clone();
        }
    }
    f
}

fn m7d_fused(ids: &[&str]) -> Value {
    let candidates: Vec<Value> = ids
        .iter()
        .enumerate()
        .map(|(n, id)| {
            let score = 0.1 - (n as f64) * 0.001;
            json!({"kind": "fact", "id": id, "score": score, "legs": ["keyword"]})
        })
        .collect();
    json!({"candidates": candidates, "legs_present": ["keyword"],
           "leg_sizes": {"keyword": ids.len()}, "leg_capped": {}, "semantic_degraded": false})
}

fn m7d_context(tier: &str) -> Value {
    json!({"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": tier,
           "recall_query": "is the mole harmless?", "recall_as_of": "2026-11-06T00:00:00Z",
           "recall_window_from": "", "recall_window_to": "",
           "audience_now": M7D_ROUND, "channel": "direct:e"})
}

/// The `t1-emit` phase of a request over the single-op reply (the shape the
/// tier-2 downgrade reads), the fused ranking in `ids` order.
pub fn m7d_emit(facts: Value, ids: &[&str], params: Value, tier: &str) -> Vec<Value> {
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &m7d_emit_doc(facts, ids, params, tier),
    )
}

fn m7d_emit_doc(facts: Value, ids: &[&str], params: Value, tier: &str) -> Value {
    let rows = json!([
        {"request_id": "r1", "leg": "fused", "payload": m7d_fused(ids).to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": "[]", "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    json!({
        "header": {"context": m7d_context(tier), "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}],
        "params": params
    })
}

/// What the script said on stderr (its journal) while running [`m7d_emit`].
pub fn m7d_emit_journal(facts: Value, ids: &[&str], params: Value, tier: &str) -> String {
    let doc = meclaw_testing::code_stdin(&m7d_emit_doc(facts, ids, params, tier));
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(RECALL),
        &doc.to_string(),
    );
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `t1-emit` bundle reply with the open replies the hydration fetched for
/// the hits (`r-hyd-reply`) beside the hits themselves.
pub fn m7d_emit_with_replies(
    facts: Value,
    replies: Value,
    ids: &[&str],
    params: Value,
) -> Vec<Value> {
    let read = json!([{"request_id": "r1", "leg": "fused",
                       "payload": m7d_fused(ids).to_string(), "fired": 1}]);
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &json!({
            "header": {"context": m7d_context("1"), "hop": {"operation": "bundle"}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-read",
                 "text": read.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-fact",
                 "text": facts.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-reply",
                 "text": replies.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-axis", "text": "[]"}
            ],
            "params": params
        }),
    )
}

/// The fusion of a `t1-legs` reply whose only voting leg is the semantic one,
/// `sem_rows` its companion page in leg order -- the fused ranking the
/// hydration bundle parks (`r-hyd-fused`), as the ids of its candidates.
pub fn m7d_fused_ids(sem_rows: Value, params: Value) -> Vec<String> {
    let rows = sem_rows.as_array().cloned().unwrap_or_default();
    let hits: Vec<Value> = rows
        .iter()
        .map(|r| json!({"kind": "fact", "id": r["id"]}))
        .collect();
    let legs = json!({"kw-ep": [], "kw-fact": [], "temporal": [], "beliefs": [],
                      "anchors": [], "axis": {}, "model": {"model_id": "m", "dim": 1024}});
    let read = json!([
        {"request_id": "r1", "leg": "legs", "payload": legs.to_string(), "fired": 0},
        {"request_id": "r1", "leg": "sem", "payload": Value::Array(hits).to_string(), "fired": 0}
    ]);
    let mut ctx = m7d_context("1");
    ctx["mem_phase"] = json!("t1-legs");
    let doc = json!({
        "header": {"context": ctx,
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-legs-graph",
             "text": json!({"paths": []}).to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-sem-aud",
             "text": sem_rows.to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-legs-read",
             "text": read.to_string()}
        ],
        "params": params
    });
    let out = meclaw_testing::emit_all(&meclaw_testing::shipped_script(RECALL), &doc);
    let park = out
        .iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .find(|t| t["id"] == "r-hyd-fused")
        .unwrap_or_else(|| panic!("no fused park: {out:?}"));
    let args: Value = meclaw_core::serde_json::from_str(park["text"].as_str().expect("args"))
        .expect("args parse");
    let fused: Value =
        meclaw_core::serde_json::from_str(args["row"]["payload"].as_str().expect("payload"))
            .expect("payload parses");
    fused["candidates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| c["id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The `t1-emit` phase over the bundle reply a live request reads -- the one
/// that gates every hydrated row against the asking round.
pub fn m7d_emit_gated(facts: Value, ids: &[&str], params: Value) -> Vec<Value> {
    let read = json!([{"request_id": "r1", "leg": "fused",
                       "payload": m7d_fused(ids).to_string(), "fired": 1}]);
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &json!({
            "header": {"context": m7d_context("1"), "hop": {"operation": "bundle"}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-read",
                 "text": read.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-fact",
                 "text": facts.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-hyd-axis", "text": "[]"}
            ],
            "params": params
        }),
    )
}

/// The bundle message of a `t1-emit`.
pub fn m7d_bundle_msg(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .cloned()
        .unwrap_or_else(|| panic!("no bundle: {out:?}"))
}

/// The text the model reads.
pub fn m7d_text(out: &[Value]) -> String {
    m7d_bundle_msg(out)["messages"][0]["text"]
        .as_str()
        .expect("rendered text")
        .to_string()
}

/// The JSON candidates beside the text.
pub fn m7d_candidates(out: &[Value]) -> Vec<Value> {
    let raw = m7d_bundle_msg(out)["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle json")
        .to_string();
    let b: Value = meclaw_core::serde_json::from_str(&raw).expect("bundle json parses");
    b["candidates"].as_array().cloned().unwrap_or_default()
}

/// The night's apply over `beliefs`, with the fact page the belief-audience
/// park reads (`facts`) carried over exactly as the night parks it.
pub fn m7d_night(beliefs: Value, facts: Value, params: Value) -> Vec<Value> {
    let gather = json!({
        "header": {"context": {"mem_phase": "belief-audience-park", "dream_run": "run-1",
                               "dream_to": "2026-11-06T03:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": facts.to_string()}],
        "params": params.clone()
    });
    let carried = meclaw_testing::emit_all(&meclaw_testing::shipped_script(DREAM_GLUE), &gather)
        .iter()
        .map(op_args)
        .find(|a| a["operation"] == "insert" && a["row"].get("payload").is_some())
        .map(|a| json!({"kind": a["row"]["kind"], "payload": a["row"]["payload"]}))
        .expect("the night carries what it knows about the facts");
    let scratch = json!([
        {"kind": "verdicts", "payload": json!({"beliefs": beliefs}).to_string()},
        {"kind": "beliefs", "payload": "[]"},
        carried,
    ]);
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(DREAM_GLUE),
        &json!({
            "header": {"context": {"mem_phase": "apply-run", "dream_run": "run-1",
                                   "dream_to": "2026-11-06T03:00:00Z"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}],
            "params": params
        }),
    )
}
