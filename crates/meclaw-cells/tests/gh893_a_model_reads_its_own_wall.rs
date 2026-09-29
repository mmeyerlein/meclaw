//! GH #893 (R-27-2 point 4) -- a model reads its own wall.
//!
//! The curator keeps every block a model was sent on its wall (GH #888), and
//! since GH #893 the model can read it back: `history_search`,
//! `history_read` and `history_outline`, served inside the curator
//! (`curator/history`) out of that model's own ledger. `thread_recall` (gone
//! with GH #889) is what these replace. What a config pin cannot see is that
//! the three halves meet on a booted tree: the menu the collector composes
//! offers them, the dispatcher's call reaches the curator and never leaves the
//! composite, and the result arrives in the model's next request under the id
//! of the call. So this file boots the SHIPPED `talky` and `cogny` side by
//! side, each with a wall sown into its curator's ledger, and measures at the
//! receiver -- the stub provider's recorded requests:
//!
//! 1. **The menu offers them.** The first request of each brain carries the
//!    three schemas in its `tools` (the curator answers the collector's menu
//!    question, OR-KY-G1).
//! 2. **talky reads its wall.** The stub brain calls `history_search`; the
//!    second request carries the `tool_result` under that call id, and it
//!    names the sown block by its id and its words.
//! 3. **The call never leaves.** Nothing reaches the parent's tool lane.
//! 4. **cogny reads only its own.** The same search in the core's brain finds
//!    the core's block and not one word of the talky's wall, although both
//!    match it.
//! 5. **No dead letter** in the run.
//!
//! Free of a real provider by construction: each brain talks to its own local
//! stub, every other `llm` cell (the curators' summarizers) to a closed local
//! port.
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
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// An endpoint nothing listens on: a call there fails at once and costs nothing.
const CLOSED_PORT: &str = "http://127.0.0.1:9/v1";
const BRAIN_MODEL: &str = "brain-stub";
const HISTORY_TOOLS: [&str; 3] = ["history_search", "history_read", "history_outline"];

/// What each wall holds. Both sentences match the one query, so a search that
/// crossed from one wall into the other would show it.
const TALKY_SAID: &str = "The lighthouse keeper is called Ada Quill.";
const COGNY_SAID: &str = "The harbour master is called Brann Holt.";
const QUERY: &str = r#"{"query":"is called"}"#;

const TALKY_CALL: &str = "call-893-talky";
const COGNY_CALL: &str = "call-893-cogny";
const CHANNEL: &str = "chat:893";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;
const CONSULT_ID: &str = "k-893";
const CONSULT_SESSION: &str = "s-893";
const QUESTION: &str = r#"{"question":"who is called what?"}"#;

/// No dead letter is expected in this run.
const EXPECTED_DEAD_LETTERS: &[(&str, &str, &str)] = &[];
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
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

/// The shipped template, copied the way instantiation lays it out: every
/// `config.json` and the seeds beside them. A `cell.type: "ref"` directory is a
/// REFERENCE (GH #277) -- the referenced template's tree takes its place, and
/// the marker's own `override_params` are then applied to the cells they name,
/// addressed by the cell's path inside the referenced template (GH #140), which
/// is what the mutation door does to a staged tree. That is how `cogny`'s
/// curator arrives with `writer.turn_write "0"` and `talky`'s collector with its
/// tool scope, exactly as a grown colony carries them.
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
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

/// Every `config.json` below `dir`, seeds excluded.
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

/// Every timer of the tree, out of the run's way: a cron moves to a date no run
/// reaches, and a `${uuid7:*}` schedule id -- an instantiation-side
/// substitution -- becomes a fixed one. Swept rather than named, so a clock a
/// template adds is quieted without this file learning its name.
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0893_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The lanes a template declares at its own path, in declaration order. A lane
/// carrying `at` docks somewhere else (ADR-0020) and is not the rim's.
fn rim_emits(template: &str) -> Vec<String> {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    cfg["params"]["contract"]["emits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|l| match &l["at"] {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        })
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// `(error_code, sender, resolved_target, hop.route)` of every dead letter the
/// colony recorded -- what `/colony/dead_letters` serves -- minus the ones
/// [`EXPECTED_DEAD_LETTERS`] names.
fn unexpected_dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
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
    .map(|(code, sender, target, msg)| {
        let m: Value = meclaw_core::serde_json::from_str(&msg).unwrap_or(Value::Null);
        (code, sender, target, text_of(&m["headers"]["hop"], "route"))
    })
    .filter(|(code, _, target, _)| {
        !EXPECTED_DEAD_LETTERS
            .iter()
            .any(|(c, suffix, _why)| code.as_str() == *c && target.ends_with(*suffix))
    })
    .collect()
}

/// Every template this road boots. One missing = skipped (GH #49).
fn shipped() -> bool {
    [
        "talky",
        "cogny",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A wall of one row, sown as seed files of a copied curator's ledger -- the
/// block under the sha256 of its canonical JSON, the row as `./intake` writes
/// it. The header of each file is the shipped schema of its table, so a column
/// the ledger gains is a column the seed names. Returns the block's hash.
fn sow(ledger: &std::path::Path, session: &str, turn: &str, text: &str) -> String {
    let schema =
        read_json(&repo("templates/curator/ledger/config.json"))["params"]["schema"].clone();
    let el = json!({"origin": "user", "type": "text", "text": text});
    let body =
        meclaw_core::serde_json::to_string(&el).expect("canonical: sorted keys, no whitespace");
    let hash = sha256_hex(&body);
    let when = "2026-09-20T10:00:00.000000Z";
    let block = json!({"hash": hash, "kind": "user", "chars": text.chars().count(), "body": body,
                       "first_seen": when});
    let row = json!({"seq": 1_000_i64, "session_id": session, "turn_id": turn, "iter": 0,
                     "kind": "user", "hash": hash, "nth": 0, "final": 1, "episode_idx": 0,
                     "at": when});
    for (table, line) in [("blocks", block), ("wall", row)] {
        let header = json!({"schema": schema[table].clone()});
        std::fs::write(
            ledger.join("seed").join(format!("{table}.jsonl")),
            format!("{header}\n{line}\n"),
        )
        .expect("write a seed file");
    }
    hash
}

/// Every `llm` cell of the tree at a local endpoint: the two brains at their
/// stubs, everything else (the curators' summarizers) at a closed port.
fn point_llms(main: &std::path::Path, brains: &BTreeMap<&str, String>) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    for f in files {
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] != "llm" {
            continue;
        }
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(main)
            .expect("under main")
            .to_string_lossy()
            .replace('\\', "/");
        let url = brains
            .get(rel.as_str())
            .cloned()
            .unwrap_or_else(|| CLOSED_PORT.to_string());
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(BRAIN_MODEL);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// The run's environment: every `${VAR}` the tree names without a default is
/// bound to a dummy, so nothing reads the host's.
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
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// One drain per rim lane of `composite`: `answer` to its sink, `tool` to the
/// probe, everything else to the park.
fn rim(composite: &str) -> Vec<Value> {
    let path = format!("./{composite}");
    let mut edges = vec![json!({"from": ".", "to": path.clone(),
                                "condition": "has(hop.route) && hop.route == 'mutation_committed'"})];
    for lane in rim_emits(composite) {
        let to = match lane.as_str() {
            "answer" => format!("/sink_{composite}"),
            "tool" => "/tool_port".to_string(),
            _ => "/park".to_string(),
        };
        edges.push(json!({"from": path.clone(), "to": to,
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    edges
}

/// The shipped talky and cogny under one parent, each wall sown. Returns the
/// hashes of the two sown blocks.
fn build(td: &tempfile::TempDir, talky_url: &str, cogny_url: &str) -> (String, String) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/talky"), &main.join("talky"), 0);
    copy_resolved(&repo("templates/cogny"), &main.join("cogny"), 0);
    // GH #553: the menu is asked for on the mutation receipt, and the boot is
    // the first receipt (ruling O-0904-2).
    std::fs::write(
        root.join("colony.json"),
        r#"{"schema_version": 1, "mutation_receipts": {"to": "/"}}"#,
    )
    .expect("colony.json");
    let mut edges = rim("talky");
    edges.extend(rim("cogny"));
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let talky = sow(
        &main.join("talky/curator/ledger"),
        "s-old",
        "t-old",
        TALKY_SAID,
    );
    let cogny = sow(
        &main.join("cogny/curator/ledger"),
        "s-core",
        "t-core",
        COGNY_SAID,
    );
    quiet_timers(&main);
    let brains = BTreeMap::from([
        ("talky/brain", talky_url.to_string()),
        ("cogny/brain", cogny_url.to_string()),
    ]);
    point_llms(&main, &brains);
    write_env(root, &main);
    (talky, cogny)
}

struct Ports {
    talky: mpsc::Receiver<Message>,
    cogny: mpsc::Receiver<Message>,
    tool: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    _park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
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
    for path in ["/sink_talky", "/sink_cogny", "/tool_port", "/park"] {
        let (tx, r) = mpsc::channel::<Message>(256);
        h.spawn(Path::new(path), move || CaptureCell::new(tx.clone()))
            .await;
        rx.push(r);
    }
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped talky and cogny, their curators with sown walls, must boot");
    let park = rx.pop().expect("park");
    let tool = rx.pop().expect("tool");
    let cogny = rx.pop().expect("cogny");
    let talky = rx.pop().expect("talky");
    (
        h,
        Ports {
            talky,
            cogny,
            tool,
            _park: park,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

/// A person's turn at the talky's door, with the round the channel stamps.
fn talky_turn() -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "who keeps the lighthouse again?"}]})))
        .ttl(400)
        .build()
}

/// A consult at cogny's door, in the shape a talky's dispatcher sends it and
/// with the context the documented ingress edge sets.
fn consult() -> Message {
    MessageBuilder::new(Path::new("/cogny"))
        .hop(map(json!({"route": "in_turn", "consult_id": CONSULT_ID,
                        "session_id": CONSULT_SESSION, "tool_name": "consult_cogny"})))
        .context(map(
            json!({"consult_id": CONSULT_ID, "session_id": CONSULT_SESSION,
                            "col_phase": ""}),
        ))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": CONSULT_ID, "text": QUESTION}]})))
        .ttl(400)
        .build()
}

/// The tool names the curator's ledger holds as the collector's menu -- one
/// slot per declaration or the whole family in one block (both are read).
fn ledger_tools(root: &std::path::Path, composite: &str) -> Vec<String> {
    let p = root.join(format!("main/{composite}/curator/ledger/cell.db"));
    // Read-only and never created here: the store seeds its tables only on a
    // FRESH birth (`OpenStatus::Created`), and a poll that created the file
    // before the ledger woke turned the sown wall into a resumed, empty one --
    // the talky's wall came up without its seed in two runs of two.
    if !p.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(
        "SELECT s.path, b.body FROM slots s LEFT JOIN blocks b ON b.hash = s.hash \
         WHERE s.path = 'tools' OR s.path LIKE 'tools.%'",
    ) else {
        return Vec::new();
    };
    let rows: Vec<(String, Option<String>)> = match st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    }) {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => return Vec::new(),
    };
    let mut names = Vec::new();
    for (path, body) in rows {
        if let Some(rest) = path.strip_prefix("tools.") {
            names.push(rest.split('.').next().unwrap_or(rest).to_string());
        } else if let Some(Value::Object(tree)) = body
            .as_deref()
            .and_then(|b| meclaw_core::serde_json::from_str::<Value>(b).ok())
        {
            names.extend(tree.keys().filter(|k| !k.starts_with('$')).cloned());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Poll until the three history tools stand in the curator's menu -- the boot
/// receipt asks, the curator answers. 30 s is the failure marker.
async fn await_menu(root: &std::path::Path, composite: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let names = ledger_tools(root, composite);
        if HISTORY_TOOLS.iter().all(|t| names.iter().any(|n| n == t)) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "{composite}: the curator's menu never offered the history tools -- the menu \
                 answer (`CURATOR_OFFER`, OR-KY-G1) did not reach the collector. It holds \
                 {names:?}. Dead letters: {:#?}",
                unexpected_dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn answer_or_explain(
    rx: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    what: &str,
) -> Message {
    match tokio::time::timeout(DEADLINE, rx.recv())
        .await
        .ok()
        .flatten()
    {
        Some(m) => m,
        None => panic!(
            "{what}: no answer within {DEADLINE:?}. Dead letters so far: {:#?}",
            unexpected_dead_letters(root)
        ),
    }
}

/// The names of the tools one provider request offered.
fn offered(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .map(|ts| {
            ts.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The content of the `tool` message answering `call_id` in one request.
fn tool_content(req: &OpenAiRequestSnapshot, call_id: &str) -> String {
    let wire = req.messages().expect("wire messages");
    let m = wire
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == call_id)
        .unwrap_or_else(|| {
            panic!(
                "no tool result under `{call_id}` reached the brain: {}",
                meclaw_core::serde_json::to_string(wire).unwrap_or_default()
            )
        });
    m["content"].as_str().unwrap_or_default().to_string()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The shipped talky and cogny, each with a wall of its own: both menus offer
/// the three tools, the talky's `history_search` is answered out of its own
/// curator under the call id and never leaves the composite, and the same
/// search in the core finds the core's block and nothing of the talky's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_reads_its_own_wall_and_never_another() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let talky_brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![(TALKY_CALL, "history_search", QUERY)]),
        canned_chat_completion("Ada Quill keeps it.", "stop"),
    ])
    .await;
    let cogny_brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![(COGNY_CALL, "history_search", QUERY)]),
        canned_chat_completion("Brann Holt is the harbour master.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    let (talky_block, cogny_block) = build(&td, &talky_brain.base_url, &cogny_brain.base_url);
    let (h, mut ports) = boot(&td).await;
    await_menu(td.path(), "talky").await;
    await_menu(td.path(), "cogny").await;

    h.send(talky_turn()).await;
    answer_or_explain(&mut ports.talky, td.path(), "talky").await;
    h.send(consult()).await;
    answer_or_explain(&mut ports.cogny, td.path(), "cogny").await;
    // The negative probe is asked only after both rounds closed: every edge of
    // the dispatcher was decided long before.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let leaked = ports.tool.try_recv().ok();
    let dead = unexpected_dead_letters(td.path());
    let talky_reqs = talky_brain.recorded_requests().await;
    let cogny_reqs = cogny_brain.recorded_requests().await;
    h.shutdown().await;

    // 1. The menu offers them, to both brains.
    for (who, reqs) in [("talky", &talky_reqs), ("cogny", &cogny_reqs)] {
        assert_eq!(reqs.len(), 2, "{who}: one tool round is two provider calls");
        let names = offered(&reqs[0]);
        for t in HISTORY_TOOLS {
            assert!(
                names.iter().any(|n| n == t),
                "{who}: `{t}` is not offered: {names:?}"
            );
        }
    }

    // 2. talky reads its wall: the result under the call id, naming the sown
    //    block by its id and its words.
    let text = tool_content(&talky_reqs[1], TALKY_CALL);
    // The talky role shows a tool result under its short id (GH #892,
    // OR-KY.T.2): `[#<12 hex>] ` and then the text the curator answered.
    let answered = text
        .strip_prefix("[#")
        .and_then(|r| r.get(12..))
        .and_then(|r| r.strip_prefix("] "))
        .unwrap_or_else(|| panic!("talky's window shows the result under its short id: {text}"));
    let result: Value = meclaw_core::serde_json::from_str(answered).expect("the result is JSON");
    assert_eq!(result["tool"], "history_search", "{text}");
    let sown_id = format!("#{}", &talky_block[..12]);
    assert!(
        result["hits"]
            .as_array()
            .is_some_and(|hs| hs.iter().any(|h| {
                h["id"] == sown_id.as_str()
                    && h["excerpt"]
                        .as_str()
                        .is_some_and(|e| e.contains(TALKY_SAID))
            })),
        "the sown block is a hit, by its id and its words: {text}"
    );
    assert!(
        !text.contains("Brann Holt"),
        "the talky read the core's wall: {text}"
    );

    // 3. The call never left the composite.
    assert!(
        leaked.is_none(),
        "a history call left on the tool lane -- the curator serves it inside: {:?}",
        leaked.map(|m| m.headers.hop)
    );

    // 4. cogny reads its own wall and only that.
    let text = tool_content(&cogny_reqs[1], COGNY_CALL);
    assert!(
        text.contains(COGNY_SAID) && text.contains(&cogny_block[..12]),
        "the core's own block is a hit: {text}"
    );
    assert!(
        !text.contains("Ada Quill") && !text.contains(&talky_block[..12]),
        "a model read another model's wall (R-27-3): {text}"
    );

    // 5. Nothing lost.
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");
}
