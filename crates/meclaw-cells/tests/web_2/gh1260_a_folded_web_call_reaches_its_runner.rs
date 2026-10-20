//! GH #1260 -- a call of the folded tool `web` reaches the cell that runs it.
//!
//! talky folds its menu (`fold_tools`): `web_search` and `web_fetch` stand as ONE
//! tool `web {op, args}`, while the tools hive of the parent keeps the two flat
//! names. The talky's dispatcher is the one place that unfolds a family call into
//! the member name. A lab run logged the model calling exactly what it was
//! offered -- `web {op: "search", args: {query}, id: "1"}` -- and the call
//! leaving the composite as tool `web`: the tools hive answered `unknown_tool`
//! ("no tool named web is wired into this hive") and the turn went without its
//! search. The stray `id` beside `op` and `args` was enough to keep the call
//! folded.
//!
//! This file boots the SHIPPED talky beside a tools hive built from the shipped
//! `templates/tools` with the two web cells the deployment wires (plus the
//! hive's own `schemas` and `unknown`), the parent edges drawn as the assistant
//! level draws them, and measures at the receiver -- the stub provider's second
//! request, which carries every result of the round under its call id:
//!
//! 1. `web {op: search, args, id}` reaches `web_search`, and its result (the
//!    stub search endpoint's hit) comes back.
//! 2. `web {op: fetch, args}` reaches `web_fetch`, and the page comes back.
//! 3. `web {op: lookup, args}` -- an op the family does not have -- comes back
//!    as the regular `unknown_tool` error, naming the ops of `web`.
//! 4. No dead letter in the run.
//! 5. Only the inner `args` reach `web_search`: the stray `id` stays behind.
//!
//! The round runs twice: through the brain, and through the quick voice a
//! `simple` turn takes (`./quick` -> `./splitter` -> `./dispatcher`, the way the
//! lab run's call came).
//!
//! Free of a real provider and of the network by construction: the brain talks
//! to a local stub, every other `llm` cell to a closed local port, and both web
//! cells to one local HTTP stub.
//!
//! Guarded like every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{LlmCellFactory, WebFetchCellFactory, WebSearchCellFactory};
use meclaw_colony::{CellFactory, CellFactoryRegistry, DeadLetter, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{MockResponse as HttpResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, deadline, override_params_on_disk, wait};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// An endpoint nothing listens on: a call there fails at once and costs nothing.
const CLOSED_PORT: &str = "http://127.0.0.1:9/v1";
const BRAIN_MODEL: &str = "brain-stub";
const CHANNEL: &str = "chat:1260";
const AUDIENCE: &str = r#"["member:a","member:e"]"#;

/// The calls of the one round, as the model writes them. The search carries
/// the stray key of the lab run.
const SEARCH_CALL: &str = "call-1260-search";
const SEARCH_ARGS: &str =
    r#"{"op":"search","args":{"query":"first train Berlin Leipzig"},"id":"1"}"#;
const FETCH_CALL: &str = "call-1260-fetch";
const LOOKUP_CALL: &str = "call-1260-lookup";
const LOOKUP_ARGS: &str = r#"{"op":"lookup","args":{}}"#;

/// What the stub endpoint answers, to the search and the fetch alike: a hit
/// whose words are this file's own, so a result that did not run is visible.
const HIT_SNIPPET: &str = "departs 05:12 from Berlin central";
/// The tools of the hive this file wires, as the deployment does.
const HIVE_CELLS: [&str; 4] = ["schemas", "unknown", "web_search", "web_fetch"];

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
/// `config.json` and the seeds beside them, a `cell.type: "ref"` directory
/// replaced by the referenced template with the marker's `override_params`
/// applied (GH #140, GH #277).
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

/// Every timer of the tree out of the run's way (a cron no run reaches, a fixed
/// schedule id for an instantiation-side `${uuid7:*}`).
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x1260_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// Every `llm` cell of the tree at a local endpoint: the talky's model cell
/// `voice` (`brain` or `quick`) at the stub, everything else at a closed port.
fn point_llms(main: &std::path::Path, brain_url: &str, voice: &str) {
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
        let url = if rel == format!("talky/{voice}") {
            brain_url.to_string()
        } else {
            CLOSED_PORT.to_string()
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(BRAIN_MODEL);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
    }
}

/// Every `${VAR}` the tree names bound to a dummy, so nothing reads the host's.
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

/// The shipped tools hive with the occupants of [`HIVE_CELLS`] only: its edges
/// that name another occupant are left out with it, every other edge stays as
/// shipped. Both web cells point at the local stub `web`.
fn tools_hive(main: &std::path::Path, web: &str) {
    let src = repo("templates/tools");
    let dst = main.join("tools");
    let mut hive = read_json(&src.join("config.json"));
    let keep = |end: &str| end == "." || HIVE_CELLS.iter().any(|c| end == format!("./{c}"));
    let edges: Vec<Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the tools hive draws edges")
        .iter()
        .filter(|e| keep(e["from"].as_str().unwrap_or("")) && keep(e["to"].as_str().unwrap_or("")))
        .cloned()
        .collect();
    hive["params"]["graph"]["edges"] = json!(edges);
    write_json(&dst.join("config.json"), &hive);
    for cell in HIVE_CELLS {
        copy_resolved(&src.join(cell), &dst.join(cell), 0);
    }
    override_params_on_disk(
        &dst.join("web_search"),
        &json!({"endpoint": format!("{web}/search")}),
    );
    override_params_on_disk(
        &dst.join("web_fetch"),
        &json!({"allow_private_networks": true}),
    );
}

/// The lanes the talky template declares at its own path.
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

/// The parent: talky and tools wired as the assistant level wires them
/// (`templates/assistant/config.json`, the `./talky` <-> `./tools` edges), the
/// answer to the sink, every other talky lane to the park.
fn parent_edges() -> Vec<Value> {
    let mut edges = vec![
        json!({"from": ".", "to": "./talky",
               "condition": "has(hop.route) && hop.route == 'mutation_committed'"}),
        json!({"from": "./talky", "to": "./tools", "default": true,
               "condition": "has(hop.route) && hop.route == 'tool'",
               "modifier": {"set_hop": {"route": "'tool_call'"},
                            "set_context": {"tool_caller": "'talky'",
                                            "called_tool": "has(hop.tool_name) ? hop.tool_name : ''"}}}),
        json!({"from": "./talky", "to": "./tools",
               "condition": "has(hop.route) && hop.route == 'schemas'",
               "modifier": {"set_hop": {"route": "'in_schemas'"},
                            "set_context": {"tool_caller": "'talky'"}}}),
        json!({"from": "./tools", "to": "./talky",
               "condition": "has(hop.route) && hop.route == 'tool_result'",
               "modifier": {"set_hop": {"route": "'in_tool'"}}}),
        json!({"from": "./tools", "to": "./talky",
               "condition": "has(hop.route) && hop.route == 'tool_schemas'",
               "modifier": {"set_hop": {"route": "'in_menu'"},
                            "set_context": {"tool_answerer": "'tools'"}}}),
    ];
    for lane in rim_emits("talky") {
        let to = match lane.as_str() {
            "answer" => "/sink",
            "tool" | "schemas" => continue,
            _ => "/park",
        };
        edges.push(json!({"from": "./talky", "to": to,
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    edges
}

/// The way a turn takes: the brain (`talk`, the shipped fallback) or the quick
/// voice of a `simple` turn.
#[derive(Clone, Copy, PartialEq)]
enum Way {
    Brain,
    Quick,
}

fn build(td: &tempfile::TempDir, brain_url: &str, web: &str, way: Way) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(&repo("templates/talky"), &main.join("talky"), 0);
    tools_hive(&main, web);
    // GH #553: the menu is asked for on the mutation receipt, and the boot is
    // the first receipt (ruling O-0904-2).
    std::fs::write(
        root.join("colony.json"),
        r#"{"schema_version": 1, "mutation_receipts": {"to": "/"}}"#,
    )
    .expect("colony.json");
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": parent_edges()}}}),
    );
    quiet_timers(&main);
    let voice = match way {
        Way::Brain => "brain",
        Way::Quick => "quick",
    };
    point_llms(&main, brain_url, voice);
    if way == Way::Quick {
        // The decider sits at the closed port, so the route cell stamps its
        // fallback class at once -- `simple` here, the class the curator sends
        // to `./quick`. The collector waits for that stamp (a wait of 0 parks
        // it unread); the bound is a hang mark, never reached.
        override_params_on_disk(
            &main.join("talky/route"),
            &json!({"fallback_class": "simple"}),
        );
        override_params_on_disk(
            &main.join("talky/collector/assemble"),
            &json!({"route_wait_ms": 600000}),
        );
    }
    write_env(root, &main);
}

struct Ports {
    answer: mpsc::Receiver<Message>,
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
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
            ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut rx = Vec::new();
    for path in ["/sink", "/park"] {
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
        .expect("the shipped talky beside the tools hive's web cells must boot");
    let park = rx.pop().expect("park");
    let answer = rx.pop().expect("sink");
    (
        h,
        Ports {
            answer,
            _park: park,
        },
    )
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

fn talky_turn() -> Message {
    MessageBuilder::new(Path::new("/talky"))
        .hop(map(json!({"route": "in_turn"})))
        .context(map(json!({"channel": CHANNEL, "audience_set": AUDIENCE})))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text",
             "text": "When does the first train leave Berlin for Leipzig on Sunday?"}]})))
        .ttl(400)
        .build()
}

/// Every dead letter the colony recorded, taken through the colony.
async fn dead_letters(h: &ColonyHandle) -> Vec<(String, String, String)> {
    deadline::hang(
        "the colony hands over its dead letters",
        h.drain_dead_letters(),
    )
    .await
    .into_iter()
    .map(|d: DeadLetter| {
        (
            d.reason.as_code().to_string(),
            d.resolved_target.as_str().to_string(),
            d.detail().unwrap_or_default().to_string(),
        )
    })
    .collect()
}

/// The tool names the talky curator's ledger holds as the collector's menu.
fn ledger_tools(root: &std::path::Path) -> Vec<String> {
    let p = root.join("main/talky/curator/ledger/cell.db");
    // Read-only and never created here: a poll that created the file before the
    // ledger woke would turn its fresh birth into a resumed, empty one.
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

/// Waits until the folded family `web` stands in the talky's menu: the tools
/// hive answered the boot receipt's menu question. The hang mark is the abort,
/// never a verdict.
async fn await_menu(h: &ColonyHandle, root: &std::path::Path) {
    wait::until_some_with(
        "talky: the tools hive's web cells stand in the menu as the family `web`",
        || {
            let ready = ledger_tools(root).iter().any(|n| n == "web");
            async move { ready.then_some(()) }
        },
        || async move {
            format!(
                "the menu holds {:?}. Dead letters: {:#?}",
                ledger_tools(root),
                dead_letters(h).await
            )
        },
    )
    .await
}

async fn answer_or_explain(rx: &mut mpsc::Receiver<Message>, h: &ColonyHandle) -> Message {
    wait::until_some_with(
        "talky: an answer after the web round",
        || {
            let got = rx.try_recv().ok();
            async move { got }
        },
        || async move { format!("dead letters so far: {:#?}", dead_letters(h).await) },
    )
    .await
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

fn shipped() -> bool {
    [
        "talky",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
        "tools",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

/// The arguments of every call that reached `/tools/web_search`, read through
/// the colony's message log.
async fn web_search_args(h: &ColonyHandle) -> Vec<Value> {
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        to_path_prefix: Some("/tools/web_search".to_string()),
        ..Default::default()
    };
    h.read_log(filter)
        .await
        .into_iter()
        .filter_map(|row| {
            let body: Value =
                meclaw_core::serde_json::from_str(row.body_payload.as_deref()?).ok()?;
            let text = body["messages"][0]["text"].as_str()?.to_string();
            meclaw_core::serde_json::from_str(&text).ok()
        })
        .collect()
}

/// One round of three folded `web` calls on the shipped talky: search (with the
/// lab run's stray key) and fetch reach their cells in the tools hive and their
/// results come back under their call ids; an op the family lacks comes back as
/// the regular error naming the ops of `web`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folded_web_call_reaches_its_runner_and_its_result_comes_back() {
    one_round_of_folded_calls(Way::Brain).await;
}

/// GH #1260: the lab run's call came through the quick voice. The
/// same round on the way a `simple` turn takes: `./quick` is the only model cell
/// at the stub, so both requests are the quick voice's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folded_web_call_of_the_quick_voice_reaches_its_runner() {
    one_round_of_folded_calls(Way::Quick).await;
}

async fn one_round_of_folded_calls(way: Way) {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let hit = json!({"results": [{"title": "Timetable", "url": "https://example.com/t",
                                  "snippet": HIT_SNIPPET}]});
    let (addr, _web) = start_mock_server(HttpResponse::ok_json(hit.to_string().as_bytes())).await;
    let web = format!("http://{addr}");
    let fetch_args =
        json!({"op": "fetch", "args": {"url": format!("{web}/timetable")}}).to_string();
    let brain = MockOpenAI::start(vec![
        canned_tool_calls(vec![
            (SEARCH_CALL, "web", SEARCH_ARGS),
            (FETCH_CALL, "web", fetch_args.as_str()),
            (LOOKUP_CALL, "web", LOOKUP_ARGS),
        ]),
        canned_chat_completion("The first train leaves at 05:12.", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &brain.base_url, &web, way);
    let (h, mut ports) = boot(&td).await;
    await_menu(&h, td.path()).await;

    h.send(talky_turn()).await;
    answer_or_explain(&mut ports.answer, &h).await;
    h.quiesce().await;
    let dead = dead_letters(&h).await;
    let searched = web_search_args(&h).await;
    let reqs = brain.recorded_requests().await;
    h.shutdown().await;

    assert_eq!(reqs.len(), 2, "one tool round is two provider calls");

    // 1. The search reached `web_search` and its hit came back.
    let search = tool_content(&reqs[1], SEARCH_CALL);
    assert!(
        search.contains(HIT_SNIPPET) && !search.contains("unknown_tool"),
        "`web {{op: search}}` reached web_search and its result came back: {search}"
    );

    // 2. The fetch reached `web_fetch` and the page came back.
    let fetch = tool_content(&reqs[1], FETCH_CALL);
    assert!(
        fetch.contains(HIT_SNIPPET) && !fetch.contains("unknown_tool"),
        "`web {{op: fetch}}` reached web_fetch and the page came back: {fetch}"
    );

    // 3. An op the family lacks: the regular error, naming the ops.
    let lookup = tool_content(&reqs[1], LOOKUP_CALL);
    assert!(
        lookup.contains("`web` has no op `lookup`; its ops: fetch, search."),
        "an unknown op is answered with the ops of its family: {lookup}"
    );

    // 4. Nothing lost.
    assert!(dead.is_empty(), "dead letters in the run: {dead:#?}");

    // 5. The stray key stayed behind: `web_search` got the family's `args`, whole
    // and alone.
    assert_eq!(
        searched,
        vec![json!({"query": "first train Berlin Leipzig"})],
        "only the inner args reach web_search"
    );
}
