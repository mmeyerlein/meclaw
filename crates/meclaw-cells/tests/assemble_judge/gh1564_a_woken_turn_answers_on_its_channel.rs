//! GH #1564 (review C1) -- a turn the clock woke answers on its own channel.
//!
//! A turn that waits for its route stamp (`route_wait_ms` above 0) and gets
//! none is fired by the strike of the one-shot `./assemble` orders at
//! `./clock`. The timer fires through its origin sink: a fresh trace with an
//! empty context. Without the turn's context kept on the order (`wake_ctx`)
//! and lifted back on the hive's edge `./clock -> ./assemble`, the woken turn
//! left on no channel, with no audience and no engine.
//!
//! This file boots the shipped member road of GH #1316 (screening door,
//! grown assistant, talky, channels) and cuts only talky's stamp edges into
//! its collector, so no stamp ever comes:
//!
//! ```text
//! member . --in_turn--> ./firewall --pass--> ./assistants --> scribe/talky
//!   -> session-keeper -> collector (waits for the stamp) --order--> clock
//!   (injected `trigger`) clock --strike--> assemble --curate--> curator ...
//!   brain (stub) ... collector --answer--> ./assistants --> ./channels
//! ```
//!
//! Measured at the receivers, never on a clock: the order at `clock`
//! (message log), the absence of a seam before the strike (quiescence), the
//! answer at `/delivered` and the `in_curate` seam into talky's curator,
//! compared key by key with the seam of the same turn on a run that does not
//! wait. The strike is a `trigger` op sent to the clock, never a real wait.
//!
//! Free of a real provider: every `llm` cell talks to a local stub. Guarded
//! like every template-reading test (GH #49).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

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

// harness_fold: #[path = "support/organism_assistant.rs"]
#[allow(unused_imports)]
use crate::organism_assistant; // harness_fold: mod organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = meclaw_testing::deadline::HANG;

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const REPLY: &str = "Shall I remind you of the walk?";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

/// The member under test sits one level below the root, so its own `.`
/// edges route like a member's do.
const M: &str = "/m";
const AUDIENCE: &str = r#"["member:owner","agent:scribe"]"#;
const NODE: &str = "telegram";
const CHANNEL: &str = "tg:42";
/// talky of the grown assistant, inside the member.
const TALKY: &str = "/m/assistants/scribe/talky";
/// The bound of the waiting run: far beyond any run, so only the injected
/// strike ends the wait.
const WAIT_MS: u64 = 600_000;
/// The turn keys `./assemble` keeps for the strike (`WAKE_CTX_KEYS` of its
/// script, the same list as on the edge `./clock -> ./assemble`). `speaker`
/// and `turn_round` are stamped keys (GH #972): they come back through the
/// clock's `contract.parks_context` (OR-54g-274), asserted on their own.
const WAKE_CTX_KEYS: [&str; 14] = [
    "channel",
    "channel_node",
    "audience_set",
    "engine",
    "live",
    "call_id",
    "counterpart",
    "counterpart_name",
    "chat_id",
    "assistant",
    "tools_allow",
    "tools_deny",
    "inner_source",
    "delegation_id",
];

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo(rel: &str) -> std::path::PathBuf {
    core_root().join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/firewall/config.json",
        "templates/firewall/screen/config.json",
        "templates/memory-hive/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/session-keeper/config.json",
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
            let target = core_root().join("templates").join(name);
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

/// Every `llm` cell of the tree at a local stub: the surface's brain at the
/// scripted one, all others at the background one.
fn point_llms_at_stubs(main: &std::path::Path, brain_rel: &str, brain: &str, background: &str) {
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
        let (url, model) = if rel == brain_rel {
            (brain, BRAIN_MODEL)
        } else {
            (background, BACKGROUND_MODEL)
        };
        cfg["params"]["base_url"] = json!(url);
        cfg["params"]["model"] = json!(model);
        cfg["params"]["api_key"] = json!("sk-test");
        write_json(&f, &cfg);
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x1564_0000 + n));
            }
            if s.get("cron").is_some() {
                s["cron"] = json!(NEVER_CRON);
            }
        }
        write_json(&f, &cfg);
    }
}

/// The run's own environment file: every `${VAR}` bound to a dummy, the
/// memory's endpoints to the background stub.
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
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{background}/v1/embeddings"),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The member's own edges of this road, verbatim off `templates/member`: the
/// door into the firewall, every exit of the firewall but the file road,
/// the answer's way into the channels, and the generations' memory edges.
fn member_road_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (
                e["from"].as_str().unwrap_or(""),
                e["to"].as_str().unwrap_or(""),
            );
            let cond = e["condition"].as_str().unwrap_or("");
            (f == "." && t == "./firewall" && cond.contains("'in_turn'"))
                || (f == "./firewall" && (t == "./assistants" || t == "."))
                || (f == "./assistants" && t == "./channels" && cond.contains("'answer'"))
                || (f == "./assistants" && t == "./memory-hive")
                || (f == "./memory-hive" && t == "./assistants")
        })
        .collect()
}

fn build_tree(root: &std::path::Path, brain: &str, background: &str) {
    let main = root.join("main");
    let m = main.join("m");
    copy_resolved(&repo("templates/firewall"), &m.join("firewall"), 0);
    copy_resolved(
        &repo("templates/assistant"),
        &m.join("assistants/scribe"),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &m.join("memory-hive"), 0);
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
    write_json(
        &m.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    for s in ["talky", "talky-chat"] {
        meclaw_testing::quiet_keeper_night(
            &m.join(format!("assistants/scribe/{s}/session-keeper")),
        );
    }
    // The channels: one witness per node, the answer routed by the node it
    // names -- the shape every channel of a member has (member README).
    write_json(
        &m.join("channels/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "/delivered",
             "condition": "has(hop.route) && hop.route == 'answer'"}
        ]}}}),
    );
    let mut edges = member_road_edges();
    assert!(
        edges.len() >= 4,
        "the member's road edges were found: {edges:?}"
    );
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    edges.push(json!({"from": "./memory-hive", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    write_json(
        &m.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./m", "to": "/park", "condition": "has(hop.route)", "default": true}
        ]}}}),
    );
    quiet_timers(&main);
    point_llms_at_stubs(&main, "m/assistants/scribe/talky/brain", brain, background);
    write_env(root, &main, background);
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
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
    let (del_tx, del_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/delivered"), move || {
        CaptureCell::new(del_tx.clone())
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
        .expect("the member road must boot");
    (h, del_rx, park_rx)
}

/// The collector of talky waits `wait_ms` for the stamp (0: not at all), runs
/// no speculation, and gets no stamp: talky's edges that carry `in_route`
/// into `./collector` are cut, on both runs alike.
fn no_stamp_comes(root: &std::path::Path, wait_ms: u64) {
    let talky = root.join("main/m/assistants/scribe/talky");
    override_params_on_disk(
        &talky.join("collector/assemble"),
        &json!({"route_wait_ms": wait_ms, "spec_ahead": "0"}),
    );
    let path = talky.join("config.json");
    let mut cfg = read_json(&path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .expect("talky has edges");
    let kept: Vec<Value> = edges
        .iter()
        .filter(|e| !(e["to"] == "./collector" && e.to_string().contains("'in_route'")))
        .cloned()
        .collect();
    assert_eq!(
        edges.len() - kept.len(),
        2,
        "talky's two stamp edges into its collector were cut"
    );
    cfg["params"]["graph"]["edges"] = Value::Array(kept);
    write_json(&path, &cfg);
}

/// Every delivery into exactly `to`, oldest first: (headers, inline body).
async fn deliveries(h: &ColonyHandle, to: &str) -> Vec<(Value, Value)> {
    h.quiesce().await;
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        to_path_prefix: Some(to.to_string()),
        ..Default::default()
    };
    let mut rows: Vec<(Value, Value)> = h
        .read_log(filter)
        .await
        .into_iter()
        .filter(|row| row.to_path == to)
        .map(|row| {
            let head = meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or(Value::Null);
            let body = row
                .body_payload
                .and_then(|b| meclaw_core::serde_json::from_str(&b).ok())
                .unwrap_or(Value::Null);
            (head, body)
        })
        .collect();
    rows.reverse();
    rows
}

/// The `in_curate` seams into talky's curator, oldest first.
async fn seams(h: &ColonyHandle) -> Vec<Value> {
    deliveries(h, &format!("{TALKY}/curator"))
        .await
        .into_iter()
        .map(|(head, _)| head)
        .filter(|head| head["hop"]["route"] == "in_curate")
        .collect()
}

/// A turn at the member's door, with the budget the ingress hands out there.
fn turn() -> Message {
    let hop = json!({"route": "in_turn", "turn_id": "p-1564"});
    // `speaker`/`turn_round` as the channel's wiring stamps them (GH #972).
    let ctx = json!({"assistant": "scribe", "audience_set": AUDIENCE, "channel": CHANNEL,
                     "channel_node": NODE, "speaker": "member:owner",
                     "turn_round": AUDIENCE});
    MessageBuilder::new(Path::new(M))
        .hop(as_map(&hop))
        .context(as_map(&ctx))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "Time for the afternoon check-in."}
        ]})))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

struct Run {
    /// The answer at `/delivered`, if one arrived.
    delivered: Option<Message>,
    /// Headers of the curate seam that carried the turn to the curator.
    seam: Option<Value>,
    /// The clock order's body (waiting run only).
    order: Option<Value>,
    /// Seams the curator got before the strike (waiting run only).
    seams_before_strike: usize,
    dead: Vec<String>,
}

async fn one_turn(wait_ms: u64) -> Run {
    let brain = MockOpenAI::start(vec![canned_chat_completion(REPLY, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build_tree(td.path(), &brain.base_url, &background.base_url);
    no_stamp_comes(td.path(), wait_ms);
    let (h, mut delivered_rx, _park) = boot(&td).await;
    h.send(turn()).await;
    let mut order = None;
    let mut seams_before_strike = 0;
    if wait_ms > 0 {
        let clock = format!("{TALKY}/collector/clock");
        order = deliveries(&h, &clock)
            .await
            .into_iter()
            .map(|(_, body)| body)
            .find(|b| b["op"] == "add" && b["schedule_name"] == "route_wait");
        seams_before_strike = seams(&h).await.len();
        if let Some(id) = order.as_ref().and_then(|o| o["schedule_id"].as_str()) {
            // The strike, injected: the timer fires the stored order now
            // (docs/cell-types.md `timer`, op `trigger`). Sent as from
            // `./assemble`, the one cell the collector hive lets reach it.
            h.send_from(
                Path::new(&format!("{TALKY}/collector/assemble")),
                MessageBuilder::new(Path::new(&clock))
                    .body(Body::Inline(json!({"messages": [], "op": "trigger",
                                              "schedule_id": id,
                                              "schedule_name": "route_wait"})))
                    .ttl(MESSAGE_DEFAULT_TTL)
                    .build(),
            )
            .await;
        }
    }
    let delivered = tokio::time::timeout(DEADLINE, delivered_rx.recv())
        .await
        .ok()
        .flatten();
    let seam = seams(&h).await.into_iter().next_back();
    let dead = h
        .drain_dead_letters()
        .await
        .iter()
        .map(|d| format!("{d:?}"))
        .collect();
    h.shutdown().await;
    Run {
        delivered,
        seam,
        order,
        seams_before_strike,
        dead,
    }
}

fn ctx_of(m: &Message) -> Value {
    Value::Object(m.headers.context.clone())
}

/// A context value as its readers read it: a missing key and '' alike.
fn read(ctx: &Value, k: &str) -> Value {
    match ctx.get(k) {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(s)) if s.is_empty() => Value::Null,
        Some(v) => v.clone(),
    }
}

/// The lock of C1: the woken turn reaches the channel it came on, and the
/// seam the strike sends to the curator carries the turn's context exactly as
/// the seam of a turn that never waited.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1564_a_woken_turn_answers_on_its_channel() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let plain = one_turn(0).await;
    let plain_seam = plain
        .seam
        .as_ref()
        .unwrap_or_else(|| panic!("the plain turn reached the curator; dead: {:?}", plain.dead));
    assert!(
        plain.delivered.is_some(),
        "the plain turn was answered; dead: {:?}",
        plain.dead
    );

    let woken = one_turn(WAIT_MS).await;
    let order = woken.order.as_ref().unwrap_or_else(|| {
        panic!(
            "the waiting turn ordered its strike; dead: {:?}",
            woken.dead
        )
    });
    assert_eq!(order["catch_up"], true, "{order}");
    assert_eq!(
        woken.seams_before_strike, 0,
        "the turn waited for its stamp until the strike"
    );
    let seam = woken.seam.as_ref().unwrap_or_else(|| {
        panic!(
            "the strike sent the turn to the curator; dead: {:?}",
            woken.dead
        )
    });
    let answer = woken
        .delivered
        .as_ref()
        .unwrap_or_else(|| panic!("the woken turn reached a channel; dead: {:?}", woken.dead));
    assert_eq!(ctx_of(answer)["channel"], CHANNEL, "the answer's channel");
    assert_eq!(ctx_of(answer)["channel_node"], NODE, "the answer's node");

    let (pc, wc) = (&plain_seam["context"], &seam["context"]);
    for k in ["channel", "channel_node", "engine", "audience_set"] {
        assert_eq!(read(wc, k), read(pc, k), "the woken seam's `{k}`: {seam}");
    }
    assert_eq!(read(wc, "channel"), json!(CHANNEL), "{seam}");
    for k in WAKE_CTX_KEYS {
        if !read(pc, k).is_null() {
            assert_eq!(read(wc, k), read(pc, k), "the woken seam's `{k}`: {seam}");
        }
    }
    // OR-54g-274: the stamped keys come back through the clock's
    // `contract.parks_context` -- a woken turn keeps who said it and its round.
    for k in ["speaker", "turn_round"] {
        assert!(
            !read(pc, k).is_null(),
            "the plain seam carries `{k}`: {plain_seam}"
        );
        assert_eq!(read(wc, k), read(pc, k), "the woken seam's `{k}`: {seam}");
    }
    // No key of the plain seam is lost on the woken one, save the two return
    // keys of the recall leg (`recall_caller`, and `audience_now`, which the
    // member's recall edge derives afresh from the restored `audience_set`):
    // they ride on the memory leg's own chain, not on the turn's.
    let lost: Vec<&str> = pc
        .as_object()
        .map(|m| {
            m.keys()
                .map(String::as_str)
                .filter(|k| !read(pc, k).is_null() && read(wc, k).is_null())
                .filter(|k| !["audience_now", "recall_caller"].contains(k))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        lost.is_empty(),
        "plain-seam keys lost on the woken seam: {lost:?}\n{seam}"
    );
}
