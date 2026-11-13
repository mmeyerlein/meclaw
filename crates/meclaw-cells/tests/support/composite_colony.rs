//! A shipped role composite as a colony of its own, with a stub provider behind
//! every `llm` cell -- the colony start the GH #892 colony case was written
//! with, lifted here unchanged so that a second test boots the same way
//! instead of carrying a copy (GH #1468).
//!
//! `build` lays the template out the way instantiation does (`ref` resolved,
//! `override_params` applied), drains every rim lane (`answer` to `/sink`,
//! everything else to `/park`), quiets the timers and points the cell named
//! `brain` at the scripted stub and every other `llm` cell at the background
//! one. `boot` starts it. What a test reads afterwards: the requests the stub
//! recorded, the answers at the sink, the rows of a cell's own `cell.db` and
//! the colony's dead letters. Nothing here reaches a paid endpoint.
#![allow(dead_code)]

use crate::curator_hive;
use crate::mock_openai::{MockOpenAI, OpenAiRequestSnapshot};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::DeadLetter;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, deadline, override_params_on_disk, wait};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

pub const BRAIN_MODEL: &str = "brain-stub";
pub const BACKGROUND_MODEL: &str = "background-stub";

pub fn repo(rel: &str) -> std::path::PathBuf {
    curator_hive::repo(rel)
}

pub fn shipped() -> bool {
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

pub fn read_json(p: &std::path::Path) -> Value {
    curator_hive::read_json(p)
}

pub fn write_json(p: &std::path::Path, v: &Value) {
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
/// is how the talky's and the cogny's curators arrive with the knobs their
/// composite sets on them.
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

pub fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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
pub fn point_llms_at_stubs(
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
pub fn quiet_timers(main: &std::path::Path) {
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
pub fn write_env(root: &std::path::Path, main: &std::path::Path, background: &str) {
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
pub fn rim_emits(template: &str) -> Vec<String> {
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
pub fn drains(from: &str, lanes: &[String]) -> Vec<Value> {
    lanes
        .iter()
        .map(|lane| {
            let to = if lane == "answer" { "/sink" } else { "/park" };
            json!({"from": from, "to": to,
                   "condition": format!("has(hop.route) && hop.route == '{lane}'")})
        })
        .collect()
}

pub fn build(td: &tempfile::TempDir, composite: &str, stub: &str, background: &str, extra: Value) {
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
pub fn ask_the_menu_at_boot(td: &tempfile::TempDir, composite: &str) {
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

pub struct Ports {
    pub sink: mpsc::Receiver<Message>,
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

pub fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("an object")
}

pub fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

pub fn texts_of(m: &Message) -> Vec<String> {
    body_of(m)["messages"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .filter_map(|t| t["text"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `(error_code, sender, resolved_target, detail)` of every dead letter the
/// colony recorded since the last take, oldest first -- the whole queue in one
/// take through the colony (`ColonyHandle::drain_dead_letters` fences the
/// writer first, TV-A3; no page of it read as the whole, 809), never read from
/// its database file. The take empties the queue: a second call names only the
/// letters that came after the first.
pub async fn dead_letters(h: &ColonyHandle) -> Vec<(String, String, String, String)> {
    deadline::hang(
        "the colony hands over its dead letters",
        h.drain_dead_letters(),
    )
    .await
    .into_iter()
    .map(|d: DeadLetter| {
        (
            d.reason.as_code().to_string(),
            d.sender_path.as_str().to_string(),
            d.resolved_target.as_str().to_string(),
            d.detail().unwrap_or_default().to_string(),
        )
    })
    .collect()
}

/// Rows of one query against a cell's own `cell.db`, read-only: "no such
/// table" and "no file yet" are the honest "nothing written yet".
pub fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
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

/// The next answer that left the composite; waits for it under the hang mark,
/// which only aborts a run that stands still and names the dead letters then.
pub async fn answer_or_explain(
    rx: &mut mpsc::Receiver<Message>,
    h: &ColonyHandle,
    what: &str,
) -> Message {
    wait::until_some_with(
        &format!("{what}: an answer leaves the composite"),
        || {
            let got = rx.try_recv().ok();
            async move { got }
        },
        || async move { format!("dead letters: {:#?}", dead_letters(h).await) },
    )
    .await
}

/// The provider's recorded requests once it was called `n` times -- the event
/// is the n-th request; the hang mark only aborts.
pub async fn provider_requests(
    mock: &MockOpenAI,
    h: &ColonyHandle,
    n: usize,
) -> Vec<OpenAiRequestSnapshot> {
    wait::until_some_with(
        &format!("the provider is called {n} time(s)"),
        || async move {
            let reqs = mock.recorded_requests().await;
            (reqs.len() >= n).then_some(reqs)
        },
        || async move {
            format!(
                "called {} time(s). Dead letters: {:#?}",
                mock.recorded_requests().await.len(),
                dead_letters(h).await
            )
        },
    )
    .await
}

/// Wait until `sql` against `db` returns a row whose first column contains
/// `needle` -- a store write behind an answer, or a rebuild behind a cold
/// cache. The row is the event; the hang mark only aborts.
pub async fn until_row(db: &std::path::Path, sql: &str, needle: &str, what: &str) {
    wait::until(what, || {
        let found = rows(db, sql)
            .iter()
            .any(|r| r.first().is_some_and(|v| v.contains(needle)));
        async move { found }
    })
    .await
}

pub fn messages(req: &OpenAiRequestSnapshot) -> Vec<Value> {
    req.messages().cloned().expect("wire messages")
}

/// D58 target (contract point 13, OR-d58-2.target.60 c): every model cell of
/// `composite` with `package_line` on reports its package -- its row and the
/// profile beside it -- as the registry's push after the birth makes it do
/// in a grown colony (this colony has no registry). Each cell gets the
/// registry's push through the composite's door (`in_model`, addressed by
/// `hop.subscriber`, the empty params slot) and answers on its `package`
/// edge; the run waits until the curator holds the row.
/// Without it the first call of a test is cut before any profile arrived
/// (the ask goes beside the call, never instead of it, OR-d58-2.target.15).
pub async fn report_packages_at_boot(td: &tempfile::TempDir, h: &ColonyHandle, composite: &str) {
    let dir = repo(&format!("templates/{composite}"));
    let mut cells: Vec<String> = std::fs::read_dir(&dir)
        .expect("the composite's template")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let cfg = e.path().join("config.json");
            cfg.is_file() && read_json(&cfg)["params"]["package_line"] == json!(true)
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    cells.sort();
    for cell in &cells {
        h.send(
            meclaw_core::MessageBuilder::new(Path::new(&format!("/{composite}")))
                .hop(map(json!({"route": "in_model",
                                "subscriber": format!("/{composite}/{cell}")})))
                .body(Body::Inline(json!({"system": {}, "params": {}})))
                .ttl(400)
                .build(),
        )
        .await;
    }
    let ledger = td
        .path()
        .join(format!("main/{composite}/curator/ledger/cell.db"));
    for cell in &cells {
        let key = format!("catalog:{cell}");
        let held = || {
            rows(&ledger, "SELECT key FROM state WHERE key LIKE 'catalog:%'")
                .iter()
                .any(|r| r.first() == Some(&key))
        };
        wait::until_with(
            &format!("the curator holds the package of {cell}"),
            || {
                let got = held();
                async move { got }
            },
            || format!("state keys: {:?}", rows(&ledger, "SELECT key FROM state")),
        )
        .await;
    }
}
