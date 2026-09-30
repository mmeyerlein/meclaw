//! GH #919 -- a person's turn that names its audience is answered within the
//! colony's own TTL budget.
//!
//! A turn at a member's generation with `context.audience_set` asks the memory
//! first (the ambient recall leg: collector -> curator policy/push -> the
//! generation's rim -> the member's recall door -> the memory hive -> the
//! bundle back into the collector) and only then curates the round (collector
//! -> curator intake/policy with their ledger round trips -> brain). Without an
//! audience the memory refuses the ask at its door and the leg is short.
//!
//! Measured on a colony of the shipped road at the default budget
//! (`MESSAGE_DEFAULT_TTL`): no loop, a straight road that is too long for one
//! budget. The turn WITH an audience crossed into the brain at ttl 1; two
//! routing decisions deeper it dead-lettered `ttl_expired` inside the curator
//! (policy -> ledger) and the brain was never asked. The same turn without an
//! audience kept 10. The numbers stand on the lock below and on this file's
//! failure messages.
//!
//! Every sibling lock of this road sends its turns with `ttl(400)`
//! (`gh895_*`), which is why none of them saw it: this file keeps the default.
//!
//! Free of a real provider by construction: every `llm` cell talks to one of
//! two local stubs. Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";
const REPLY: &str = "Noted: the probe.";

/// The round of the turn, in the affinity vocabulary.
const AUDIENCE: &str = r#"["member:owner","agent:scribe"]"#;

const SURFACES: [&str; 2] = ["talky", "talky-chat"];

/// The routing decisions a person's turn costs ABOVE the container this file
/// injects at: the member's own entry and its screening door (`. -> ./firewall`,
/// `./firewall -> ./assistants`). The ingress hands out the default budget at
/// the member, so the container sees two less.
const ABOVE_THE_CONTAINER: u32 = 2;

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/memory-hive/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/cogny/config.json",
        "templates/collector/config.json",
        "templates/curator/config.json",
        "templates/curator/push/config.json",
        "templates/dispatcher/config.json",
        "templates/session-keeper/config.json",
        "templates/tools/config.json",
        "examples/organism/grow-assistant.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
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

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

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

struct Stubs {
    brain: String,
    background: String,
}

/// Point EVERY `llm` cell of the tree at a local stub: the surface's brain at
/// the scripted one, all others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, brain: &str, stubs: &Stubs) -> Vec<String> {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let mut pointed = Vec::new();
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
            (&stubs.brain, BRAIN_MODEL)
        } else {
            (&stubs.background, BACKGROUND_MODEL)
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
        pointed.push(rel);
    }
    pointed.sort();
    pointed
}

/// Every timer of the tree, out of the run's way (the sweep of `gh889`).
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0919_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` bound to a dummy, the
/// memory's model and embedder endpoints to the background stub.
fn write_env(root: &std::path::Path, main: &std::path::Path, stubs: &Stubs) {
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
    vars.insert("MEMORY_LLM_BASE_URL".into(), stubs.background.clone());
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", stubs.background),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (an `at` lane docks
/// elsewhere, ADR-0020).
fn rim_emits(template: &str) -> Vec<String> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["contract"]["emits"]
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

/// The member's own edges between its generations and its memory, verbatim
/// off `templates/member`: the recall door and the bundle's way back, the
/// memory section, the episodes, the close pass, the memory tool and its menu.
fn member_memory_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (e["from"].as_str(), e["to"].as_str());
            matches!(
                (f, t),
                (Some("./assistants"), Some("./memory-hive"))
                    | (Some("./memory-hive"), Some("./assistants"))
            )
        })
        .collect()
}

/// The member stand-in: `./assistants` is the container, grown with the
/// generation `scribe` the way `examples/organism/grow-assistant.json` grows
/// it; `./memory-hive` is the person's memory; the member's own edges between
/// the two; everything else that leaves either is drained.
fn build_member(td: &tempfile::TempDir, surface: &str, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    let grown = read_json(&repo("examples/organism/grow-assistant.json"));
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    // A sweep closes every channel that has been silent at all.
    for s in SURFACES {
        override_params_on_disk(
            &main.join(format!("assistants/scribe/{s}/session-keeper/close")),
            &json!({"idle_ms": 1}),
        );
    }
    let mut edges = member_memory_edges();
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    for lane in rim_emits("memory-hive") {
        let taken = ["bundle", "tool_result", "tool_schemas", "reject"];
        if !taken.contains(&lane.as_str()) {
            edges.push(json!({"from": "./memory-hive", "to": "/park",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
    }
    edges.push(json!({"from": "./memory-hive", "to": "/park",
                      "condition": "has(hop.route) && hop.route == 'reject' && \
                                    (!has(hop.recall_caller) || hop.recall_caller == 'outside')"}));
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    quiet_timers(&main);
    let pointed = point_llms_at_stubs(&main, &format!("assistants/scribe/{surface}/brain"), stubs);
    write_env(root, &main, stubs);
    pointed
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    #[allow(dead_code)]
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
            // The generation's own tools hive (`templates/tools`): spawned,
            // never called -- the stub brains call no tool.
            ("bash".to_string(), Arc::new(BashCellFactory)),
            ("edit".to_string(), Arc::new(EditCellFactory)),
            ("file".to_string(), Arc::new(FileCellFactory)),
            ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
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
        .expect("the generation, the member road and the memory hive must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

// ─────────────────────────────────────────────────────────────── the turn

/// A person's words at the container's door, on the talky channel, with the
/// budget the ingress leaves after the member's own two routing decisions.
fn person(text: &str, audience: bool, deeper: u32) -> Message {
    let mut ctx = json!({"assistant": "scribe", "channel": "talky:919"});
    if audience {
        ctx["audience_set"] = json!(AUDIENCE);
    }
    MessageBuilder::new(Path::new("/assistants"))
        .hop(as_map(&json!({"route": "in_turn"})))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL - ABOVE_THE_CONTAINER - deeper)
        .build()
}

/// `(from, to, route, ttl)` of every delivery the colony logged, in order.
fn deliveries(root: &std::path::Path) -> Vec<(String, String, String, i64)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, headers, ttl FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, headers, ttl)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        let route = h["hop"]["route"].as_str().unwrap_or_default().to_string();
        (from, to, route, ttl)
    })
    .collect()
}

/// `(sender, target, error_code)` of every dead letter of the run.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT sender_path, original_target, error_code FROM dead_letters ORDER BY id")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// What a run measured: whether the answer left, what the brain was asked, the
/// budget at the seams of the round, and what died.
struct Run {
    answered: bool,
    brain_requests: usize,
    /// ttl on the last delivery into the talky's collector on `in_bundle`
    /// (the memory leg coming home); -1 when there was none.
    bundle_home: i64,
    /// ttl on the first delivery into the talky's curator on `in_curate`.
    curate_in: i64,
    /// the lowest ttl any delivery of the run carried.
    floor: i64,
    /// the lowest ttl inside the curator between the round's `in_curate` and
    /// the delivery into the brain: what the seam leaves for the curator's
    /// own ledger round trips.
    seam_floor: i64,
    /// deliveries between the turn and the first `in_curate` at the curator.
    hops_to_curate: usize,
    expired: Vec<(String, String, String)>,
}

impl Run {
    fn say(&self) -> String {
        format!(
            "answered={} brain_requests={} bundle_home_ttl={} curate_in_ttl={} floor_ttl={} \
             seam_floor_ttl={} hops_to_curate={} ttl_expired={:?}",
            self.answered,
            self.brain_requests,
            self.bundle_home,
            self.curate_in,
            self.floor,
            self.seam_floor,
            self.hops_to_curate,
            self.expired
        )
    }
}

async fn one_turn(audience: bool, deeper: u32) -> Run {
    let brain = MockOpenAI::start(vec![canned_chat_completion(REPLY, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        brain: brain.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_member(&td, "talky", &stubs);
    assert!(
        pointed.iter().any(|p| p == "assistants/scribe/talky/brain"),
        "the brain under test talks to the scripted stub; pointed: {pointed:?}"
    );
    let (h, mut ports) = boot(&td).await;
    h.send(person("Remember the probe for me.", audience, deeper))
        .await;
    let answered = tokio::time::timeout(DEADLINE, ports.sink.recv())
        .await
        .ok()
        .flatten()
        .is_some();
    let brain_requests = brain.recorded_requests().await.len();
    let log = deliveries(td.path());
    let dead = dead_letters(td.path());
    h.shutdown().await;

    let collector = "/assistants/scribe/talky/collector";
    let curator = "/assistants/scribe/talky/curator";
    let bundle_home = log
        .iter()
        .filter(|(_, to, route, _)| to == collector && route == "in_bundle")
        .map(|(_, _, _, ttl)| *ttl)
        .next_back()
        .unwrap_or(-1);
    let first_curate = log
        .iter()
        .position(|(_, to, route, _)| to == curator && route == "in_curate");
    let curate_in = first_curate.map(|i| log[i].3).unwrap_or(-1);
    let floor = log.iter().map(|(_, _, _, t)| *t).min().unwrap_or(-1);
    if std::env::var("GH919_TRACE").is_ok() {
        for (i, (from, to, route, ttl)) in log.iter().enumerate() {
            eprintln!("gh919 trace {i:3} ttl={ttl:3} {from} -> {to} [{route}]");
        }
    }
    let brain_at = "/assistants/scribe/talky/brain";
    let seam_floor = first_curate
        .map(|i| {
            let end = log[i..]
                .iter()
                .position(|(_, to, _, _)| to == brain_at)
                .map_or(log.len(), |j| i + j);
            log[i..end]
                .iter()
                .filter(|(_, to, _, _)| to.starts_with(curator))
                .map(|(_, _, _, t)| *t)
                .min()
                .unwrap_or(-1)
        })
        .unwrap_or(-1);
    Run {
        answered,
        seam_floor,
        brain_requests,
        bundle_home,
        curate_in,
        floor,
        hops_to_curate: first_curate.unwrap_or(log.len()),
        expired: dead
            .into_iter()
            .filter(|(_, _, code)| code == "ttl_expired")
            .collect(),
    }
}

/// The lock: with an audience, the memory leg runs AND the round still reaches
/// the brain and answers, and the curator gets the round on a budget of its
/// own. Measured before the repair (report F): the recall leg spends 26
/// routing decisions (collector 51 -> bundle home 25), the round reaches the
/// curator on `in_curate` with 20 left, the curator's intake/policy/handover
/// ledger round trips spend 19 of them, and the round crosses into the brain
/// at ttl 1 -- the only restore of the round sat on `curator -> brain`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_names_its_audience_is_answered_at_the_default_budget() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let run = one_turn(true, 0).await;
    eprintln!("gh919 with audience: {}", run.say());
    assert!(
        run.bundle_home >= 0,
        "the memory leg ran and came home into the collector: {}",
        run.say()
    );
    assert!(
        run.expired.is_empty() && run.brain_requests >= 1 && run.answered,
        "the brain was asked and the answer left the generation: {}",
        run.say()
    );
    assert!(
        run.seam_floor >= i64::from(MESSAGE_DEFAULT_TTL / 2),
        "the curator works the round on the budget the memory leg left over, not on \
         one of its own (the seam `collector -> curator` restores nothing): {}",
        run.say()
    );
}

/// The failure a lab colony saw, two routing decisions deeper than this
/// file's member stand-in (any level more between the ingress and the
/// generation): before the repair the round died `ttl_expired` in the
/// curator (policy -> ledger) and the brain was never asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_turn_two_levels_deeper_is_still_answered() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let run = one_turn(true, 2).await;
    eprintln!("gh919 with audience, two deeper: {}", run.say());
    assert!(
        run.expired.is_empty() && run.brain_requests >= 1 && run.answered,
        "the turn two levels deeper is answered: {}",
        run.say()
    );
}

/// The counter-probe: without an audience the memory refuses the ask at its
/// door, and the turn is answered -- before and after the repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_an_audience_is_answered_too() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let run = one_turn(false, 0).await;
    eprintln!("gh919 without audience: {}", run.say());
    assert!(
        run.expired.is_empty() && run.brain_requests >= 1 && run.answered,
        "the turn without an audience is answered: {}",
        run.say()
    );
}
