//! GH #892, the colony case -- the window of a shipped composite follows its
//! role, and the model trims it through the sidecar of its own answer.
//!
//! Measured AT THE RECEIVER: the request bodies a stub provider recorded, the
//! rows of the curator's own ledger, the answers that left the composite and
//! the colony's dead letters. The SHIPPED `talky` and `cogny` are booted with
//! a stub behind every `llm` cell -- nothing here reaches a paid endpoint.
//!
//! talky (role `talky`, the brain stamping a two-second cache):
//!
//! 1. every foreign block the provider reads begins with its short id, the
//!    id of the wall block that holds it;
//! 2. while the cache is warm, the second request begins with the whole of
//!    the first -- short ids do not move the prefix;
//! 3. an answer whose sidecar releases the first turn's id changes nothing
//!    until the cache has gone cold; after the rebuild the request carries the
//!    one-line form in the block's place, and the ledger still holds the block
//!    byte for byte;
//! 4. the person never sees the block, and nothing dead-letters;
//! 5. the menu asked at boot reaches the curator, and its `window` section
//!    stands in the brain's system part (OR-KY-G1).
//!
//! cogny (role `consult`, the splitter it grew for the curator): an answer
//! with a block leaves as an advice without it, the block's `memory` section
//! leaves nothing but a `topic` mark in the core's ledger, and nothing
//! dead-letters.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "support/curator_hive.rs"]
mod curator_hive;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long a run is left alone after its last expected arrival before
/// "nothing else" is read.
const SETTLE: Duration = Duration::from_secs(2);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";

const CHANNEL: &str = "chat:892";
const AUDIENCE: &str = r#"["member:owner","agent:voice"]"#;
const FIRST: &str = "please keep this long list of names in mind for a moment";
const SECOND: &str = "thanks, that is all about the list";
const THIRD: &str = "what else is new?";

const CONSULT_ID: &str = "k-892";
const CONSULT_SESSION: &str = "s-892";
const QUESTION: &str = r#"{"question":"which way does the river run?"}"#;
const ADVICE: &str = "It runs north, past the old mill.";

fn repo(rel: &str) -> std::path::PathBuf {
    curator_hive::repo(rel)
}

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

fn read_json(p: &std::path::Path) -> Value {
    curator_hive::read_json(p)
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The shipped template, laid out the way instantiation lays it out: a `ref`
/// directory is replaced by the referenced template's tree, and the marker's
/// `override_params` land on the cells they name (GH #140, GH #277) -- which
/// is how the talky's curator arrives with `role talky` and the cogny's with
/// `role consult`.
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
                .expect("a ref names a template");
            let name = reference.split('@').next().unwrap_or_default();
            copy_resolved(&repo("templates").join(name), dst, depth + 1);
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

/// Every `llm` cell of the tree at a local stub: `brain` at the scripted one
/// (with `extra` params), all others at the background one.
fn point_llms_at_stubs(
    main: &std::path::Path,
    brain: &str,
    stub: &str,
    background: &str,
    extra: &Value,
) {
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
        let (url, model) = if rel == brain {
            (stub, BRAIN_MODEL)
        } else {
            (background, BACKGROUND_MODEL)
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        if rel == brain {
            for (k, v) in extra.as_object().cloned().unwrap_or_default() {
                cfg["params"][k] = v;
            }
        }
        write_json(&f, &cfg);
    }
}

/// Every timer out of the run's way: a cron to a date no run reaches, a
/// `${uuid7:*}` schedule id to a fixed one. The curator's clock carries no
/// schedule and keeps striking the one-shots it is ordered.
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0892_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// Every `${VAR}` the tree references bound to a dummy; the endpoints to the
/// background stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, background: &str) {
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
    vars.insert("MEMORY_LLM_BASE_URL".into(), background.to_string());
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (a lane with `at` docks
/// below the rim and is not the rim's).
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

/// One drain per lane: `answer` to the sink, everything else to the park.
fn drains(from: &str, lanes: &[String]) -> Vec<Value> {
    lanes
        .iter()
        .map(|lane| {
            let to = if lane == "answer" { "/sink" } else { "/park" };
            json!({"from": from, "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

fn build(td: &tempfile::TempDir, composite: &str, stub: &str, background: &str, extra: Value) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo(&format!("templates/{composite}")),
        &main.join(composite),
        0,
    );
    let edges = drains(&format!("./{composite}"), &rim_emits(composite));
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    point_llms_at_stubs(
        &main,
        &format!("{composite}/brain"),
        stub,
        background,
        &extra,
    );
    write_env(root, &main, background);
}

/// The menu asked at boot, as a grown colony asks it: the boot is the first
/// mutation receipt (GH #553, ruling O-0904-2), and the rim hands it to the
/// composite's collector, which asks every answerer -- the curator among them
/// (OR-KY-G1). The parent's own `schemas` ask parks unanswered.
fn ask_the_menu_at_boot(td: &tempfile::TempDir, composite: &str) {
    let root = td.path();
    std::fs::write(
        root.join("colony.json"),
        r#"{"schema_version": 1, "mutation_receipts": {"to": "/"}}"#,
    )
    .expect("colony.json");
    let main = root.join("main/config.json");
    let mut cfg = read_json(&main);
    cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the root's edges")
        .push(json!({"from": ".", "to": format!("./{composite}"),
                     "condition": "has(hop.route) && hop.route == 'mutation_committed'"}));
    write_json(&main, &cfg);
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    park: mpsc::Receiver<Message>,
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
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
        .expect("the shipped composite and its curator must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn talky_turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn consult() -> Message {
    MessageBuilder::new(Path::new("/cogny"))
        .hop(map(json!({"route": "in_turn", "consult_id": CONSULT_ID,
                        "session_id": CONSULT_SESSION, "tool_name": "consult_cogny"})))
        .context(map(json!({"consult_id": CONSULT_ID,
                            "session_id": CONSULT_SESSION, "col_phase": ""})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": CONSULT_ID, "text": QUESTION}]})))
        .ttl(400)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn texts_of(m: &Message) -> Vec<String> {
    body_of(m)["messages"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .filter_map(|t| t["text"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `(error_code, sender, resolved_target, hop.route)` of every dead letter.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String, String)> {
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
        let route = m["headers"]["hop"]["route"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (code, sender, target, route)
    })
    .collect()
}

/// Rows of one query against a cell's own `cell.db`, read-only: "no such
/// table" and "no file yet" are the honest "nothing written yet".
fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .map(|it| it.filter_map(Result::ok).collect())
    .unwrap_or_default()
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
            "{what}: no answer left the composite within {DEADLINE:?}. Dead letters: {:#?}",
            dead_letters(root)
        ),
    }
}

async fn provider_requests(
    mock: &MockOpenAI,
    root: &std::path::Path,
    n: usize,
) -> Vec<OpenAiRequestSnapshot> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reqs = mock.recorded_requests().await;
        if reqs.len() >= n {
            return reqs;
        }
        if Instant::now() >= deadline {
            panic!(
                "the provider was called {} time(s), not {n}, within {DEADLINE:?}. Dead \
                 letters: {:#?}",
                reqs.len(),
                dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Wait until `sql` against `db` returns a row whose first column contains
/// `needle` -- a store write behind an answer, or a rebuild behind a cold
/// cache.
async fn until_row(db: &std::path::Path, sql: &str, needle: &str, what: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if rows(db, sql)
            .iter()
            .any(|r| r.first().is_some_and(|v| v.contains(needle)))
        {
            return;
        }
        assert!(Instant::now() < deadline, "{what} within {DEADLINE:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn messages(req: &OpenAiRequestSnapshot) -> Vec<Value> {
    req.messages().cloned().expect("wire messages")
}

/// talky: short ids on the wire, a warm prefix that holds, and a release
/// that becomes one line after the cache went cold -- never before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh892_the_window_follows_its_role_and_the_models_trim() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    // The run is seconds long; a day that turns during it would move the
    // talky's day tiers under the assertions. A wait for the clock, not for a
    // result.
    let left = curator_hive::secs_to_midnight();
    if left <= 60 {
        tokio::time::sleep(Duration::from_secs(left as u64 + 1)).await;
    }
    let first = curator_hive::user(FIRST);
    let first_id = curator_hive::short_id(&first);
    let release = format!(
        "Done, the list can go.\n\n```sidecar\n{{\"window\": {{\"release\": [\"#{first_id}\"]}}}}\n```"
    );
    let brain = MockOpenAI::start(vec![
        canned_chat_completion("Noted, I have the list.", "stop"),
        canned_chat_completion(&release, "stop"),
        canned_chat_completion("Nothing else is new.", "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    // The brain stamps a cache that goes cold two seconds after each call:
    // the curator's clock is ordered on that stamp (GH #890).
    build(
        &td,
        "talky",
        &brain.base_url,
        &background.base_url,
        json!({"cache_mode": "implicit", "cache_ttl_s": 2}),
    );
    ask_the_menu_at_boot(&td, "talky");
    let (h, mut ports) = boot(&td).await;
    let ledger = td.path().join("main/talky/curator/ledger/cell.db");
    // 5. The curator answered the menu: its `window` section stands in the
    //    sidecar contract the collector filed with the curator.
    until_row(
        &ledger,
        "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash \
         WHERE s.path = 'instructions.sidecar'",
        "[#0123456789ab]",
        "the curator's menu answer reached the sidecar contract",
    )
    .await;

    h.send(talky_turn(FIRST)).await;
    answer_or_explain(&mut ports.sink, td.path(), "turn 1").await;
    h.send(talky_turn(SECOND)).await;
    let second = answer_or_explain(&mut ports.sink, td.path(), "turn 2").await;
    let reqs = provider_requests(&brain, td.path(), 2).await;

    // 1. The first turn's words, behind the id of the wall block that holds them.
    let one = messages(&reqs[0]);
    let two = messages(&reqs[1]);
    // 5, at the receiver: the brain reads the `window` section and how to
    // write it in its system part (OR-KY-G1, the curator as menu answerer).
    let system: String = one
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(
        system.contains("[#0123456789ab]") && system.contains("\"window\""),
        "the brain's system part carries the curator's `window` section: {system}"
    );
    let shown = format!("[#{first_id}] {FIRST}");
    assert!(
        one.iter()
            .any(|m| m["role"] == "user" && m["content"] == json!(shown)),
        "the first request shows the turn behind its short id: {one:?}"
    );
    // 2. Warm: the second request begins with the whole of the first.
    assert_eq!(
        &two[..one.len()],
        one.as_slice(),
        "short ids do not move the prefix a provider's cache keys on"
    );
    // 4. The person hears the prose, never the block.
    let said = texts_of(&second);
    assert!(
        said.iter().any(|t| t == "Done, the list can go.")
            && !said
                .iter()
                .any(|t| t.contains("```") || t.contains("release")),
        "the answer leaves without its block: {said:?}"
    );
    // The release is a mark in the ledger at once ...
    until_row(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'release'",
        &first_id,
        "the window section reached the curator as a `release` mark",
    )
    .await;
    // ... and the window at the next rebuild: after the cache went cold.
    until_row(
        &ledger,
        "SELECT value FROM state WHERE key = 'window_plan'",
        &first_id,
        "a cold cache rebuilt the window with the release in its plan",
    )
    .await;
    h.send(talky_turn(THIRD)).await;
    answer_or_explain(&mut ports.sink, td.path(), "turn 3").await;
    let reqs = provider_requests(&brain, td.path(), 3).await;
    let three = messages(&reqs[2]);
    let line = format!("[#{first_id} released \u{2014} history_read(\"#{first_id}\")]");
    assert!(
        three
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == json!(line)),
        "after the rebuild the block is one line: {three:?}"
    );
    assert!(
        !three
            .iter()
            .any(|m| m["content"].as_str().is_some_and(|c| c.contains(FIRST))),
        "and its words are gone from the window: {three:?}"
    );
    // 3. The ledger holds the block byte for byte.
    let body = rows(
        &ledger,
        &format!("SELECT body FROM blocks WHERE hash LIKE '{first_id}%' AND kind = 'user'"),
    );
    assert_eq!(
        body,
        vec![vec![curator_hive::canonical(&first)]],
        "the released block stands in the ledger as it came"
    );
    tokio::time::sleep(SETTLE).await;
    let dead = dead_letters(td.path());
    while ports.park.try_recv().is_ok() {}
    h.shutdown().await;
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}

/// cogny: the splitter it grew cuts the block out of the advice, and the
/// block's sections stay in the core.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh892_cogny_cuts_its_block_and_keeps_its_sections() {
    if !shipped() {
        return;
    }
    let with_block = format!(
        "{ADVICE}\n\n```sidecar\n{{\"memory\": {{\"nothing_new\": false, \"facts\": [], \
         \"topic\": {{\"movement\": \"start\", \"name\": \"the river\"}}}}}}\n```"
    );
    let brain = MockOpenAI::start(vec![canned_chat_completion(&with_block, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion("A short account.", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(
        &td,
        "cogny",
        &brain.base_url,
        &background.base_url,
        json!({}),
    );
    let (h, mut ports) = boot(&td).await;
    let ledger = td.path().join("main/cogny/curator/ledger/cell.db");

    h.send(consult()).await;
    let advice = answer_or_explain(&mut ports.sink, td.path(), "cogny").await;
    let said = texts_of(&advice);
    assert!(
        said.iter().any(|t| t == ADVICE)
            && !said
                .iter()
                .any(|t| t.contains("```") || t.contains("topic")),
        "the advice leaves without its block: {:?}",
        body_of(&advice)
    );
    until_row(
        &ledger,
        "SELECT value FROM marks WHERE kind = 'topic'",
        r#"{"movement":"start","name":"the river"}"#,
        "the memory section left its topic mark in the core's ledger",
    )
    .await;
    tokio::time::sleep(SETTLE).await;
    let mut parked = Vec::new();
    while let Ok(m) = ports.park.try_recv() {
        parked.push(m.headers.hop.clone());
    }
    let dead = dead_letters(td.path());
    h.shutdown().await;
    assert!(
        !parked
            .iter()
            .any(|hop| hop.get("route").and_then(Value::as_str) == Some("sidecar")),
        "no section leaves the core: {parked:?}"
    );
    assert!(dead.is_empty(), "no dead letter: {dead:#?}");
}
