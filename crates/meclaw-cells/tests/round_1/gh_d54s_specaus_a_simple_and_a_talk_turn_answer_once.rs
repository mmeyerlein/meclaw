//! d54s specaus (GH #1584, R-54-9) -- with the speculation gone, a text turn
//! stamped `simple` and one stamped `talk` each still reach the brain and
//! answer exactly once.
//!
//! WHY this file exists: the strand takes the quick voice's speculation
//! (`./quick_spec`, its edges, the `spec_*` params of the lead) out of talky.
//! The owner's product runs the 0.72 form: `spec_ahead` "0", every stamp on
//! `path_routes` to the brain (`simple:brain`), the bridge on `*:quick`. The
//! contract of the strand (point 4): that form behaves as before. A config pin
//! cannot see that -- a set override the lead no longer knows (`spec_target`)
//! refuses the whole birth (GH #294; a ref marker in the template does not), and a dropped edge loses a turn only at
//! run time. So this file BIRTHS the shipped talky from its template (refs resolved
//! through the mutation door, the 0.72 lead values as the set's
//! `override_params`), points `./brain` at a local stub and every other `llm` cell at a
//! closed port, lets the route cell stamp its fallback class (the decider at
//! the closed port fails at once) under a collector that waits for the stamp,
//! and measures at the parent's sink: one answer, the brain's words, from one
//! brain call -- and no request at the quick voice's own stub.
//!
//! Free of a real provider and of the network by construction. Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, DeadLetter, MutationOutcome,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, deadline, wait};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// An endpoint nothing listens on: a call there fails at once and costs nothing.
const CLOSED_PORT: &str = "http://127.0.0.1:9/v1";
const STUB_MODEL: &str = "stub-model";
const CHANNEL: &str = "chat:d54s";
const AUDIENCE: &str = r#"["member:a","member:e"]"#;
/// The brain's words: what the person must hear.
const BRAIN_TEXT: &str = "The kettle is on the left shelf.";
/// The quick voice's words: the person must not hear them on this way.
const QUICK_TEXT: &str = "Quick voice here.";
/// The lead's way of the 0.72 form (soul2me set 0.72, `talky/lead/lead`):
/// every class to the brain, the bridge on the quick voice.
const PATH_ROUTES_072: &str = "talk:brain,simple:brain,complex:consult,research:research,*:brain";
const BRIDGE_072: &str = "*:quick";

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

/// A whole directory, byte for byte: the library a colony scans for its
/// templates.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0d54_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// `talky/brain` at the words stub, the quick voice and its bridge
/// (`talky/quick`, `talky/quick_bridge`) at a second stub that must stay
/// unasked, every other `llm` cell (the decider, the judges, the summarizer)
/// at a closed port: a call there fails at once and costs nothing.
fn point_llms(main: &std::path::Path, brain_url: &str, quick_url: &str) {
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
        let url = match rel.as_str() {
            "talky/brain" => brain_url,
            "talky/quick" | "talky/quick_bridge" => quick_url,
            _ => CLOSED_PORT,
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(STUB_MODEL);
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

/// The lanes the talky template declares at its own path.
fn rim_emits() -> Vec<String> {
    let cfg = read_json(&repo("templates/talky/config.json"));
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

/// The parent: the answer to the sink, the error drain to its own capture,
/// every other talky lane to the park.
fn parent_edges() -> Vec<Value> {
    let mut edges = vec![];
    for lane in rim_emits() {
        let to = match lane.as_str() {
            "answer" => "./sink",
            "error" => "./drain",
            _ => "./park",
        };
        edges.push(json!({"from": "./talky", "to": to,
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    edges
}

/// The colony before the birth: an empty root hive and the library in
/// `templates/`. Only the library copy is fitted for a run without a provider
/// -- every `llm` cell pointed at a stub or a closed port, every timer out of
/// the way, every `${VAR}` bound to a dummy; the composite and its overrides
/// come through the mutation door like a set's.
fn build(td: &tempfile::TempDir, brain_url: &str, quick_url: &str) {
    let root = td.path();
    let main = root.join("main");
    let library = root.join("templates");
    copy_tree(&repo("templates"), &library);
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    quiet_timers(&library);
    point_llms(&library, brain_url, quick_url);
    write_env(root, &library);
}

/// The birth of talky, as a set asks for it: the template by name, the lead's
/// way of the 0.72 form, the route's fallback class and the collector's wait
/// for the stamp as `override_params`, the parent's edges in the same diff.
fn birth(class: &str) -> Value {
    json!({
        "scope": "/",
        "ctx": {"model": STUB_MODEL},
        "diff": {
            "add_nodes": [{
                "name": "talky",
                "template": "talky",
                "override_params": {
                    "lead/lead": {"path_routes": PATH_ROUTES_072, "bridge_target": BRIDGE_072},
                    // The decider sits at the closed port, so the route cell
                    // stamps its fallback class at once; the collector waits
                    // for that stamp (the bound is a hang mark, never reached).
                    "route": {"fallback_class": class},
                    "collector/assemble": {"route_wait_ms": 600000}
                }
            }],
            "add_edges": parent_edges()
        }
    })
}

struct Ports {
    answer: mpsc::Receiver<Message>,
    drain: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    _park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir, class: &str) -> (ColonyHandle, Ports) {
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
    for path in ["/sink", "/drain", "/park"] {
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
        .expect("the empty root must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("the library scans");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: birth(class),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send the birth");
    let outcome = ack_rx.await.expect("the birth is answered");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the shipped talky is born from its template with the 0.72 lead: {outcome:?}"
    );
    let park = rx.pop().expect("park");
    let drain = rx.pop().expect("drain");
    let answer = rx.pop().expect("sink");
    (
        h,
        Ports {
            answer,
            drain,
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
            {"origin": "user", "type": "text", "text": "Where did I put the kettle?"}]})))
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

fn shipped() -> bool {
    [
        "talky",
        "collector",
        "curator",
        "dispatcher",
        "session-keeper",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => meclaw_core::serde_json::to_string(&v["messages"]).unwrap_or_default(),
        other => format!("{other:?}"),
    }
}

/// One birth, one turn stamped `class`: the brain is called once and the
/// person hears its words once.
async fn one_turn_answers_once_from_the_brain(class: &str) {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = MockOpenAI::start(vec![canned_chat_completion(BRAIN_TEXT, "stop")]).await;
    // The quick voice answers too, with words of its own: a turn sent there
    // would not fail over to the brain unseen (GH #1352's `-esc` copy), it
    // would count a request here.
    let quick = MockOpenAI::start(vec![canned_chat_completion(QUICK_TEXT, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &brain.base_url, &quick.base_url);
    let (h, mut ports) = boot(&td, class).await;

    h.send(talky_turn()).await;
    let answer = wait::until_some_with(
        &format!("talky: a {class} turn reaches the person"),
        || {
            let got = ports.answer.try_recv().ok();
            async move { got }
        },
        || {
            let drained = ports.drain.try_recv().ok().map(|m| text_of(&m));
            let hr = &h;
            async move {
                format!(
                    "drained errors: {drained:?}; dead letters so far: {:#?}",
                    dead_letters(hr).await
                )
            }
        },
    )
    .await;
    h.quiesce().await;
    let mut more = 0;
    while ports.answer.try_recv().is_ok() {
        more += 1;
    }
    let brain_reqs = brain.recorded_requests().await;
    let quick_reqs = quick.recorded_requests().await;
    h.shutdown().await;

    let said = text_of(&answer);
    assert!(
        said.contains(BRAIN_TEXT),
        "a {class} turn: the brain's words reach the person: {said}"
    );
    assert_eq!(
        more, 0,
        "a {class} turn: exactly one answer reaches the person"
    );
    assert_eq!(
        brain_reqs.len(),
        1,
        "a {class} turn: the brain is called exactly once"
    );
    assert_eq!(
        quick_reqs.len(),
        0,
        "a {class} turn: the 0.72 way sends it to the brain, never to the quick voice"
    );
    let window = meclaw_core::serde_json::to_string(brain_reqs[0].messages().expect("wire"))
        .unwrap_or_default();
    assert!(
        window.contains("the kettle"),
        "a {class} turn: the brain sees the person's turn: {window}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_simple_turn_answers_once_from_the_brain() {
    one_turn_answers_once_from_the_brain("simple").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_talk_turn_answers_once_from_the_brain() {
    one_turn_answers_once_from_the_brain("talk").await;
}
