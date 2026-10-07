//! GH #941 -- a sidecar section addresses the episode of its own turn.
//!
//! A recognizer elsewhere in the colony reads a section of a model's sidecar
//! block and marks the episode of the turn it came from through the memory
//! hive's `in_affect` lane (GH #936), addressed by `session_id` + `turn_id`.
//! The episode's turn id is `<session>#<tag>-<index>`, minted by the curator
//! (GH #932); a section carried only the round's uuid (`context.turn_id`), so a
//! consumer could not address the episode without building the id itself --
//! out of the tag rule and a per-round index it has no way to know.
//!
//! The fix stamps the id where it is known: the curator's intake, which mints
//! the index with the wall row, hands the incoming turn's episode id on with
//! the model call, and the brain edge of every composite promotes it to
//! `context.episode_turn_id`, which rides the call's answer -- and every
//! section split out of it -- to whoever reads it.
//!
//! Two layers, cheapest first:
//!
//! 1. **The rule** (no colony): the intake mints the id with exactly the
//!    function the writer uses on `turn_write`, and every composite that runs
//!    a curator promotes it on its brain edge.
//! 2. **The colony** (the member road of `gh916`: a generation of the shipped
//!    assistant, the shipped memory hive, a probe app that logs every section it
//!    hears; stub models): a section of a plain chat turn and one of a voice
//!    turn that consulted its core first (a second model call, so the id comes
//!    off the wall row the first call wrote) each carry the id; a consumer that
//!    sends `in_affect` with nothing but what the section carries gets an
//!    `affect_ack` without an error code, and the mark stands on exactly the
//!    person's episode of that turn.
//!
//! Free of a real provider by construction: every `llm` cell of the tree talks
//! to a local stub, the memory hive's embedder too.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry one of the templates or the fixture is skipped, never judged.

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
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, emit_all, override_params_on_disk, shipped_script};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[path = "support/organism_assistant.rs"]
mod organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

const AUDIENCE: &str = r#"["member:owner","agent:sam"]"#;
const GENERATION: &str = "sam";
const BRAINS: [&str; 3] = ["talky", "talky-chat", "cogny"];
const SECTION: &str = "probe";
const PROBE_APP: &str = "probe-app";

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
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
        "templates/curator/intake/config.json",
        "templates/curator/writer/config.json",
        "templates/dispatcher/config.json",
        "templates/session-keeper/config.json",
        "templates/tools/config.json",
        "templates/builder/recipes/config.json",
        "examples/organism/grow-assistant.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
        && fixture("gh916_probe_app/config.json").is_file()
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

// ═══════════════════════════════════════════════════════════ 1. the rule

/// The code of `def <name>(` in a shipped script, its docstring left out --
/// two cells may explain the same rule in their own words, never compute it
/// differently.
fn code_of(script: &str, name: &str) -> String {
    let head = format!("\ndef {name}(");
    let at = script
        .find(&head)
        .unwrap_or_else(|| panic!("no def {name}"));
    let mut lines = script[at + 1..].lines();
    let sig = lines.next().unwrap_or_default().to_string();
    let body: Vec<&str> = lines
        .take_while(|l| l.is_empty() || l.starts_with(' '))
        .collect();
    let mut code = vec![sig];
    let mut in_doc = false;
    for l in body {
        let t = l.trim();
        if t.starts_with("\"\"\"") {
            // A one-line docstring opens and closes on the same line.
            in_doc = !(t.len() > 3 && t.ends_with("\"\"\"")) && !in_doc;
            continue;
        }
        if in_doc {
            if t.ends_with("\"\"\"") {
                in_doc = false;
            }
            continue;
        }
        if !t.is_empty() && !t.starts_with('#') {
            code.push(l.to_string());
        }
    }
    code.join("\n")
}

#[test]
fn the_intake_mints_the_id_exactly_as_the_writer_does() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let intake = shipped_script(&repo("templates/curator/intake/config.json").to_string_lossy());
    let writer = shipped_script(&repo("templates/curator/writer/config.json").to_string_lossy());
    assert!(
        intake.contains("\ndef episode_turn_id("),
        "the curator's intake mints no episode turn id for the model call"
    );
    assert_eq!(
        code_of(&intake, "episode_turn_id"),
        code_of(&writer, "episode_turn_id"),
        "the id a section carries must be the id `turn_write` gives the episode"
    );
}

/// The `./curator -> ./brain` edge of a composite.
fn brain_edge(composite: &str) -> Value {
    let cfg = read_json(&repo(&format!("templates/{composite}/config.json")));
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|e| e["from"] == "./curator" && e["to"] == "./brain")
        .unwrap_or_else(|| panic!("{composite}: no brain edge out of the curator"))
        .clone()
}

#[test]
fn the_composite_that_writes_episodes_promotes_the_id_and_cogny_passes_it_through() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let edge = brain_edge("talky");
    assert_eq!(
        edge["modifier"]["set_context"]["episode_turn_id"],
        json!("has(hop.episode_turn_id) ? hop.episode_turn_id : ''"),
        "talky: the brain edge promotes the incoming turn's episode id, and clears a \
         stale one when the call has none: {edge}"
    );
    // Only the composite whose curator writes the person's episodes stamps
    // the id. cogny's writer is off (`turn_write: "0"`), so the index its own
    // intake mints names talky's k-th episode of the round, not the consulting
    // turn (measured: `#<tag>-0` where talky's turn was `#<tag>-2`) -- and the
    // edge would overwrite the right value talky hands over with the consult
    // (`episode_turn_id` is SHARED). cogny passes it through.
    let edge = brain_edge("cogny");
    assert!(
        edge["modifier"]["set_context"]
            .get("episode_turn_id")
            .is_none(),
        "cogny writes no episodes (`turn_write: \"0\"`): its brain edge leaves the id the \
         consulting turn handed over alone: {edge}"
    );
}

// ═══════════════════════════════════════════════ 2. the colony (stub models)

/// What `install_app` draws for the probe app at a member, member-relative.
fn install_edges() -> Vec<Value> {
    let tpl = read_json(&fixture("gh916_probe_app/template.json"));
    let out = emit_all(
        &shipped_script(&repo("templates/builder/recipes/config.json").to_string_lossy()),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/os/orgs/acme/members/alex",
                                                    "app": PROBE_APP,
                                                    "template": format!("{PROBE_APP}@1.0.0"),
                                                    "screen": "display",
                                                    "generation": GENERATION,
                                                    // GH #949: the member's person, whose
                                                    // round (`AUDIENCE`) the app's edges
                                                    // stamp; `install_app` asks for it.
                                                    "ctx": {"member_person": "owner"},
                                                    "declaration": tpl["app"].clone()}})
                                  .to_string()}],
        }),
    );
    let first = out.first().expect("the renderer emits");
    assert!(
        first["header"]["error_code"].is_null(),
        "install_app refused the declaration: {first}"
    );
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no add_edges in {first}"))
        .clone()
}

/// The container as `examples/organism` grows it, its generation renamed.
fn container_edges() -> Vec<Value> {
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
    let raw = meclaw_core::serde_json::to_string(&grown["diff"]["add_edges"]).expect("serialise");
    let renamed = raw
        .replace("./scribe", &format!("./{GENERATION}"))
        .replace("'scribe'", &format!("'{GENERATION}'"))
        .replace("/assistants/scribe/", &format!("/assistants/{GENERATION}/"));
    meclaw_core::serde_json::from_str(&renamed).expect("reparse")
}

/// The shipped template, copied the way instantiation lays it out.
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

struct Stubs {
    talky: String,
    talky_chat: String,
    cogny: String,
    background: String,
}

/// Every `llm` cell of the tree at a local stub: each brain at its own, all
/// others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, stubs: &Stubs) {
    let mut files = Vec::new();
    configs_under(main, &mut files);
    let brains: BTreeMap<String, &String> = [
        ("talky", &stubs.talky),
        ("talky-chat", &stubs.talky_chat),
        ("cogny", &stubs.cogny),
    ]
    .into_iter()
    .map(|(b, url)| (format!("assistants/{GENERATION}/{b}/brain"), url))
    .collect();
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
        let (url, model) = match brains.get(&rel) {
            Some(url) => (*url, BRAIN_MODEL),
            None => (&stubs.background, BACKGROUND_MODEL),
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
        pointed.push(rel);
    }
    for b in BRAINS {
        let brain = format!("assistants/{GENERATION}/{b}/brain");
        assert!(
            pointed.contains(&brain),
            "{brain} is not an llm cell of the tree"
        );
    }
}

/// Every timer of the tree, out of the run's way.
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0941_0000 + n));
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

/// The member's own edges this road needs, verbatim off `templates/member`:
/// everything between its generations and its memory, and the one edge that
/// hands a section to its apps.
fn member_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (e["from"].as_str(), e["to"].as_str());
            let sidecar = e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'sidecar'"));
            matches!(
                (f, t),
                (Some("./assistants"), Some("./memory-hive"))
                    | (Some("./memory-hive"), Some("./assistants"))
            ) || (f == Some("./assistants") && t == Some("./apps") && sidecar)
        })
        .collect()
}

/// The lanes a template declares at its own path.
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

/// The member stand-in of `gh916` with the probe app alone; what leaves the
/// memory hive (the `affect_ack` among it) ends at `/park`.
fn build_member(td: &tempfile::TempDir, stubs: &Stubs) -> std::path::PathBuf {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join(format!("assistants/{GENERATION}")),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": container_edges()}}}),
    );
    for s in ["talky", "talky-chat"] {
        override_params_on_disk(
            &main.join(format!("assistants/{GENERATION}/{s}/collector/assemble")),
            &json!({"brief_slots": []}),
        );
    }
    write_json(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    let log = root.join("probe-app.jsonl");
    copy_resolved(
        &fixture("gh916_probe_app"),
        &main.join(format!("apps/{PROBE_APP}")),
        0,
    );
    override_params_on_disk(
        &main.join(format!("apps/{PROBE_APP}/sink")),
        &json!({"log_path": log.to_string_lossy()}),
    );
    let mut edges = member_edges();
    edges.extend(install_edges());
    edges.push(json!({"from": "./assistants", "to": "/sink",
                      "condition": "has(hop.route) && hop.route == 'answer'"}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    edges.push(json!({"from": "./apps", "to": "/park",
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
    point_llms_at_stubs(&main, stubs);
    write_env(root, &main, stubs);
    log
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
            ("bash".to_string(), Arc::new(BashCellFactory)),
            ("edit".to_string(), Arc::new(EditCellFactory)),
            ("file".to_string(), Arc::new(FileCellFactory)),
            ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(4096);
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
        .expect("the generation, the member road, the memory hive and the app must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

fn person(surface: &str, text: &str) -> Message {
    person_in(surface, text, &format!("T-941-{surface}"))
}

fn person_in(surface: &str, text: &str, turn: &str) -> Message {
    let mut ctx = json!({"assistant": GENERATION, "channel": format!("{surface}:941"),
                         "audience_set": AUDIENCE, "turn_id": turn});
    if surface == "talky-chat" {
        ctx["channel_node"] = json!("chat");
    }
    MessageBuilder::new(Path::new("/assistants"))
        .hop(as_map(&json!({"route": "in_turn"})))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(400)
        .build()
}

fn with_block(text: &str, brain: &str) -> String {
    let block = json!({
        SECTION: {"entries": [{"brain": brain, "note": "noted for the app"}]},
        "memory": {"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}
    });
    format!("{text}\n\n```sidecar\n{block}\n```")
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn said(m: &Message) -> String {
    body_of(m)["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn answer_saying(ports: &mut Ports, needle: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.sink.recv()).await {
            Ok(Some(m)) if said(&m).contains(needle) => return,
            Ok(Some(_)) => {}
            _ => panic!("no answer saying {needle:?} left the generation within {DEADLINE:?}"),
        }
    }
}

fn log_lines(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// The section `brain`'s model wrote, as the app heard it.
async fn section_of(log: &std::path::Path, brain: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    let sender = format!("/assistants/{GENERATION}/{brain}/");
    loop {
        if let Some(line) = log_lines(log).into_iter().find(|l| {
            l["route"] == "sidecar"
                && l["hop"]["section"] == json!(SECTION)
                && l["reply_to"]
                    .as_str()
                    .is_some_and(|r| r.starts_with(&sender))
        }) {
            return line;
        }
        assert!(
            Instant::now() < deadline,
            "{brain}: no section reached the app within {DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
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

async fn until_rows(db: &std::path::Path, sql: &str, n: usize, what: &str) -> Vec<Vec<String>> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let got = rows(db, sql);
        if got.len() >= n {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: fewer than {n} row(s) of `{sql}` within {DEADLINE:?}: {got:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn ctx_str(line: &Value, key: &str) -> String {
    line["context"][key]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A consumer's `in_affect` built ONLY from what the section carries: its
/// session, the episode id and the round it was said in.
fn affect_from(line: &Value, tag: &str) -> Message {
    let round = match ctx_str(line, "audience_now") {
        r if !r.is_empty() => r,
        _ => ctx_str(line, "audience_set"),
    };
    MessageBuilder::new(Path::new("/memory-hive"))
        .hop(as_map(&json!({"route": "in_affect", "affect_tag": tag})))
        .context(as_map(&json!({"audience_now": round})))
        .body(Body::Inline(json!({"messages": [], "affect": {
            "session_id": ctx_str(line, "session_id"),
            "turn_id": ctx_str(line, "episode_turn_id"),
            "valence": -0.4, "arousal": 0.3, "confidence": 0.8, "source": "probe-941"}})))
        .ttl(400)
        .build()
}

async fn ack_tagged(ports: &mut Ports, tag: &str) -> Message {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ports.park.recv()).await {
            Ok(Some(m))
                if m.headers.hop.get("route") == Some(&json!("affect_ack"))
                    && m.headers.hop.get("affect_tag") == Some(&json!(tag)) =>
            {
                return m;
            }
            Ok(Some(_)) => {}
            _ => panic!("no affect_ack tagged {tag:?} left the memory within {DEADLINE:?}"),
        }
    }
}

const CHAT_ASKS: &str = "What is on my list for today?";
const CHAT_SAYS: &str = "Two things: the dentist and the letter.";
const TALKY_ASKS: &str = "Can you work out a plan for the weekend?";
/// An earlier voice turn of the same round, so the consulting turn's episode
/// is not the round's first: cogny's own count (it writes no episodes) would
/// then name a different one than talky's.
const WARMUP_ASKS: &str = "Good morning, are you there?";
const WARMUP_SAYS: &str = "Good morning, I am here.";
const INTERIM: &str = "Let me ask my core.";
const CONSULT_ID: &str = "call-c941";
const CONSULT_ARGS: &str = r#"{"question": "Plan a weekend for the person.", "context": "Goal: a quiet weekend. Facts: none yet. Constraints: none named. Form: a short plan. Length: keep the answer under 4000 characters when you can."}"#;
const COGNY_SAYS: &str = "Saturday a walk, Sunday a book.";
const TALKY_SAYS: &str = "A walk on Saturday and a book on Sunday.";

/// A section of a real turn names its episode; `in_affect` built from the
/// section alone marks exactly the person's episode of that turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_section_marks_the_persons_episode_of_its_own_turn() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let talky = MockOpenAI::start(vec![
        canned_chat_completion(WARMUP_SAYS, "stop"),
        canned_content_and_tool_calls(INTERIM, vec![(CONSULT_ID, "consult_cogny", CONSULT_ARGS)]),
        canned_chat_completion(&with_block(TALKY_SAYS, "talky"), "stop"),
    ])
    .await;
    let cogny = MockOpenAI::start(vec![canned_chat_completion(
        &with_block(COGNY_SAYS, "cogny"),
        "stop",
    )])
    .await;
    let chat = MockOpenAI::start(vec![canned_chat_completion(
        &with_block(CHAT_SAYS, "talky-chat"),
        "stop",
    )])
    .await;
    let background = MockOpenAI::start(
        (0..64)
            .map(|_| canned_chat_completion(BACKGROUND_REPLY, "stop"))
            .collect(),
    )
    .await;
    let stubs = Stubs {
        talky: talky.base_url.clone(),
        talky_chat: chat.base_url.clone(),
        cogny: cogny.base_url.clone(),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let log = build_member(&td, &stubs);
    let memory = td.path().join("main/memory-hive/store/cell.db");
    let (h, mut ports) = boot(&td).await;

    h.send(person("talky-chat", CHAT_ASKS)).await;
    answer_saying(&mut ports, CHAT_SAYS).await;
    h.send(person_in("talky", WARMUP_ASKS, "T-941-talky-warmup"))
        .await;
    answer_saying(&mut ports, WARMUP_SAYS).await;
    h.send(person("talky", TALKY_ASKS)).await;
    answer_saying(&mut ports, TALKY_SAYS).await;

    // (brain, the person's words of that turn): the chat turn is one model
    // call; the voice turn's section comes off its SECOND call, after the
    // consult -- the id then comes off the wall row the first call wrote.
    let mut consulting = String::new();
    for (n, (brain, asked)) in [("talky-chat", CHAT_ASKS), ("talky", TALKY_ASKS)]
        .into_iter()
        .enumerate()
    {
        let line = section_of(&log, brain).await;
        let session = ctx_str(&line, "session_id");
        let episode = ctx_str(&line, "episode_turn_id");
        assert!(
            !session.is_empty() && episode.starts_with(&format!("{session}#")),
            "{brain}: the section names the episode of its turn as `<session>#<tag>-<index>` \
             in `context.episode_turn_id`: {line}"
        );
        // The episode the id names is in the memory before anyone marks it.
        until_rows(
            &memory,
            &format!("SELECT id FROM episodes WHERE turn_id = '{episode}'"),
            1,
            &format!("{brain}: the episode of the section's turn"),
        )
        .await;

        let tag = format!("t-{n}");
        h.send(affect_from(&line, &tag)).await;
        let ack = ack_tagged(&mut ports, &tag).await;
        assert_eq!(
            ack.headers.hop.get("error_code"),
            Some(&json!("")),
            "{brain}: the mark addressed by the section alone is written: {:?}",
            ack.headers.hop
        );
        let marked = until_rows(
            &memory,
            &format!(
                "SELECT turn_id, sender, content FROM episodes \
                 WHERE affect IS NOT NULL AND session_id = '{session}'"
            ),
            1,
            &format!("{brain}: the marked episode"),
        )
        .await;
        assert_eq!(
            marked.len(),
            1,
            "{brain}: exactly one episode of the session is marked: {marked:?}"
        );
        assert_eq!(
            marked[0][0], episode,
            "{brain}: the episode the section named"
        );
        assert_ne!(
            marked[0][1], "assistant",
            "{brain}: never the agent's answer"
        );
        assert!(
            marked[0][2].contains(asked),
            "{brain}: the person's words of THIS turn: {marked:?}"
        );
        consulting = episode;
    }

    // The core's section of the consult names the consulting turn's episode
    // (the value talky handed over), not one cogny counted itself.
    let line = section_of(&log, "cogny").await;
    assert_eq!(
        ctx_str(&line, "episode_turn_id"),
        consulting,
        "cogny: the section of the consult names the person's episode of the turn that \
         consulted: {line}"
    );
    h.shutdown().await;
}
