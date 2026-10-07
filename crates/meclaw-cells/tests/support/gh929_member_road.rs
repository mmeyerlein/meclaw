//! The member road of GH #929's chain-length locks: a person's turn at a
//! member's generation, booted from the SHIPPED templates, every `llm` cell on
//! a local stub. Taken over from `gh919_a_turn_with_an_audience_is_answered.rs`
//! (which took it from `gh895_*`): `./assistants` is the container, grown with
//! the generation `scribe` the way `examples/organism/grow-assistant.json`
//! grows it; `./memory-hive` is the person's memory; the member's own edges
//! between the two; everything else that leaves either is drained.
//!
//! What it adds is the measurement: a segment is read off the test colony's
//! own `message_log` along the parent chain. A cell's emission carries the ttl
//! of the input it was handling and names that input as its parent, and a hive
//! transit names the message it forwards, so walking `parent_message_id` back
//! from a delivery walks exactly the chain that carried its budget -- no
//! interleaved traffic of another trace can lower a floor.
//!
//! The including test file declares `#[path = "mock_openai.rs"] mod
//! mock_openai;` at its root.
#![allow(dead_code)]

use crate::mock_openai::{MockOpenAI, canned_chat_completion};
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
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[path = "organism_assistant.rs"]
pub mod organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator.
pub const DEADLINE: Duration = Duration::from_secs(30);

pub const BRAIN_MODEL: &str = "brain-stub";
pub const BACKGROUND_MODEL: &str = "background-stub";
pub const BACKGROUND_REPLY: &str = "A short account of what was said.";
pub const REPLY: &str = "Noted: the probe.";

/// The round of the turn, in the affinity vocabulary.
pub const AUDIENCE: &str = r#"["member:owner","agent:scribe"]"#;

pub const SURFACES: [&str; 2] = ["talky", "talky-chat"];

/// The routing decisions a person's turn costs ABOVE the container this road
/// injects at: the member's own entry and its screening door (`. ->
/// ./firewall`, `./firewall -> ./assistants`).
pub const ABOVE_THE_CONTAINER: u32 = 2;

/// The generation under the container.
pub const GENERATION: &str = "/assistants/scribe";

// ─────────────────────────────────────────────────────────────── the tree

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

pub fn shipped() -> bool {
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

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

pub fn write_json(p: &std::path::Path, v: &Value) {
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

/// Base URLs of the local stubs: `scripted` answers the llm cells named in
/// `Road::scripted`, `background` every other.
pub struct Stubs {
    pub scripted: HashMap<String, String>,
    pub background: String,
}

/// Point EVERY `llm` cell of the tree at a local stub: a cell named in
/// `stubs.scripted` (path relative to `main/`) at its own, all others at the
/// background one.
pub fn point_llms_at_stubs(main: &std::path::Path, stubs: &Stubs) -> Vec<String> {
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
        let (url, model) = match stubs.scripted.get(&rel) {
            Some(url) => (url, BRAIN_MODEL),
            None => (&stubs.background, BACKGROUND_MODEL),
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0929_0000 + n));
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
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{background}/v1/embeddings"),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The lanes a template declares at its own path (an `at` lane docks
/// elsewhere, ADR-0020).
pub fn rim_emits(template: &str) -> Vec<String> {
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
/// off `templates/member`.
pub fn member_memory_edges() -> Vec<Value> {
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

/// The member stand-in (see the file header). Returns the `llm` cells pointed.
pub fn build_member(td: &tempfile::TempDir, stubs: &Stubs) -> Vec<String> {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
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
    let pointed = point_llms_at_stubs(&main, stubs);
    write_env(root, &main, &stubs.background);
    pointed
}

pub fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        // The generation's own tools hive (`templates/tools`).
        ("bash".to_string(), Arc::new(BashCellFactory)),
        ("edit".to_string(), Arc::new(EditCellFactory)),
        ("file".to_string(), Arc::new(FileCellFactory)),
        ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
        ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
    ]
}

pub struct Ports {
    pub sink: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    pub park: mpsc::Receiver<Message>,
}

pub async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
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

/// A person's words at the container's door, on the given surface's channel,
/// with the budget the ingress leaves after the member's own two routing
/// decisions and `deeper` more.
pub fn person(surface: &str, text: &str, audience: bool, deeper: u32) -> Message {
    let mut ctx = json!({"assistant": "scribe", "channel": "talky:929"});
    if surface == "talky-chat" {
        ctx["channel_node"] = json!("chat");
    }
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

/// One delivery the colony logged.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub id: String,
    pub parent: Option<String>,
    pub from: String,
    pub to: String,
    pub route: String,
    pub ttl: i64,
}

impl Delivery {
    pub fn say(&self) -> String {
        format!(
            "ttl={:3} {} -> {} [{}]",
            self.ttl, self.from, self.to, self.route
        )
    }
}

/// Every delivery the colony logged, in order.
pub fn deliveries(root: &std::path::Path) -> Vec<Delivery> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT id, parent_message_id, from_path, to_path, headers, ttl FROM message_log \
             ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(id, parent, from, to, headers, ttl)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        let route = h["hop"]["route"].as_str().unwrap_or_default().to_string();
        Delivery {
            id,
            parent,
            from,
            to,
            route,
            ttl,
        }
    })
    .collect()
}

/// `(sender, target, error_code)` of every dead letter of the run.
pub fn dead_letters(root: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT sender_path, original_target, error_code FROM dead_letters ORDER BY id")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// The budget one segment spent: `start` is the ttl of the delivery right
/// behind the opening seam, `end` the ttl of the last delivery before the
/// closing seam on the same parent chain, `used = start - end`.
#[derive(Clone, Debug)]
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub hops: usize,
}

impl Segment {
    pub fn used(&self) -> i64 {
        self.start - self.end
    }

    /// The line every case prints (the evidence for a ruling row).
    pub fn say(&self, name: &str) -> String {
        format!(
            "gh929 {name}: start={} end={} used={}",
            self.start,
            self.end,
            self.used()
        )
    }
}

/// The segment that ends at `closing` (the delivery through the closing seam)
/// and opens at the nearest ancestor for which `opening` holds. `None` when the
/// parent chain from `closing` does not reach such an ancestor.
pub fn segment_before(
    log: &[Delivery],
    closing: &Delivery,
    opening: impl Fn(&Delivery) -> bool,
) -> Option<Segment> {
    let by_id: HashMap<&str, &Delivery> = log.iter().map(|d| (d.id.as_str(), d)).collect();
    let last = closing
        .parent
        .as_deref()
        .and_then(|p| by_id.get(p))
        .copied()?;
    let mut at = last;
    let mut hops = 1;
    loop {
        if opening(at) {
            return Some(Segment {
                start: at.ttl,
                end: last.ttl,
                hops,
            });
        }
        at = at.parent.as_deref().and_then(|p| by_id.get(p)).copied()?;
        hops += 1;
        if hops > 4 * MESSAGE_DEFAULT_TTL as usize {
            return None;
        }
    }
}

/// The parent chain of `d`, newest first -- for a failure message.
pub fn chain_of(log: &[Delivery], d: &Delivery, max: usize) -> Vec<String> {
    let by_id: HashMap<&str, &Delivery> = log.iter().map(|d| (d.id.as_str(), d)).collect();
    let mut out = vec![d.say()];
    let mut at = d;
    while let Some(p) = at.parent.as_deref().and_then(|p| by_id.get(p)).copied() {
        out.push(p.say());
        at = p;
        if out.len() >= max {
            break;
        }
    }
    out
}

/// What a road needs besides the tree: which `llm` cells follow a script (path
/// under `main/`, e.g. `assistants/scribe/talky/brain`) and what the
/// background stub answers.
pub struct Road {
    pub scripted: Vec<(String, Vec<MockResponse>)>,
    pub turn: Message,
    /// How many answers leave the container before the run is read.
    pub answers: usize,
}

/// What a road run leaves to read.
pub struct Run {
    pub log: Vec<Delivery>,
    pub dead: Vec<(String, String, String)>,
    pub answers: Vec<Message>,
    /// Requests each scripted stub saw, in `Road::scripted` order.
    pub requests: Vec<usize>,
}

impl Run {
    pub fn expired(&self) -> Vec<(String, String, String)> {
        self.dead
            .iter()
            .filter(|(_, _, c)| c == "ttl_expired")
            .cloned()
            .collect()
    }

    /// The `n`-th delivery (0-based) for which `pred` holds.
    pub fn nth(&self, n: usize, pred: impl Fn(&Delivery) -> bool) -> Option<&Delivery> {
        self.log.iter().filter(|d| pred(d)).nth(n)
    }

    pub fn trace(&self, tag: &str) {
        if std::env::var("GH929_TRACE").is_ok() {
            for (i, d) in self.log.iter().enumerate() {
                eprintln!("gh929 trace {tag} {i:3} {}", d.say());
            }
        }
    }
}

/// Boot the member road, send the turn, wait for the answers, read the log.
pub async fn run(road: Road) -> Run {
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let mut scripted_stubs = Vec::new();
    let mut scripted = HashMap::new();
    for (cell, responses) in road.scripted {
        let stub = MockOpenAI::start(responses).await;
        scripted.insert(cell, stub.base_url.clone());
        scripted_stubs.push(stub);
    }
    let stubs = Stubs {
        scripted,
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = build_member(&td, &stubs);
    for cell in stubs.scripted.keys() {
        assert!(
            pointed.contains(cell),
            "{cell} is not an llm cell of the road: {pointed:?}"
        );
    }
    let (h, mut ports) = boot(&td).await;
    h.send(road.turn).await;
    let mut answers = Vec::new();
    while answers.len() < road.answers {
        match tokio::time::timeout(DEADLINE, ports.sink.recv()).await {
            Ok(Some(m)) => answers.push(m),
            _ => break,
        }
    }
    let mut requests = Vec::new();
    for stub in &scripted_stubs {
        requests.push(stub.recorded_requests().await.len());
    }
    h.shutdown().await;
    Run {
        log: deliveries(td.path()),
        dead: dead_letters(td.path()),
        answers,
        requests,
    }
}

/// The one-response brain of a turn that calls no tool.
pub fn plain_brain() -> Vec<MockResponse> {
    vec![canned_chat_completion(REPLY, "stop")]
}
