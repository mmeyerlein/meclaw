//! GH #1316 -- an inner turn crosses the whole member and its answer reaches
//! the channel it names.
//!
//! GH #1305 gave an inner turn (`hop.kind == "inner"`, `hop.inner_source`,
//! `hop.deliver_channel_node`) a door of its own at talky: channel
//! `inner:<source>`, answer addressed to `context.channel_node`. Every lock of
//! #1305 handed the turn to talky directly or decided talky's edges alone.
//! Nobody sent one through the member, and the member's screening door
//! (`./firewall`, the shipped `screen` with its store round trips) rebuilds
//! the header of every exit from a fixed key set: the three keys were gone one
//! hop past the member's door, talky stamped the turn as a person's on channel
//! `default`, and the answer went to no channel (measured in the orga lab,
//! trace of `test_cap_and_gap_hold`).
//!
//! This file boots that road, every hop the shipped one:
//!
//! ```text
//! member . --in_turn--> ./firewall (screen + rules store, shipped seed)
//!   --pass -> in_turn--> ./assistants (container, grown `scribe`)
//!   --> scribe/talky (inner door) -> session-keeper ... brain (stub) ... collector
//!   --answer--> scribe --> ./assistants --answer, channel_node--> ./channels
//! ```
//!
//! Measured at the receivers, never on a clock: the delivery into talky's
//! session keeper (message log) carries `kind inner` and channel
//! `inner:<source>`, and the answer arrives at the channel named by
//! `deliver_channel_node`. The counter-probe sends a person's turn the same
//! way: its hop at talky carries none of the three keys, and its answer
//! reaches its own channel.
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
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// harness_fold: #[path = "support/organism_assistant.rs"]
#[allow(unused_imports)]
use crate::organism_assistant; // harness_fold: mod organism_assistant;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const BRAIN_MODEL: &str = "brain-stub";
const BACKGROUND_MODEL: &str = "background-stub";
const REPLY: &str = "Shall I remind you of the walk?";
const BACKGROUND_REPLY: &str = "A short account of what was said.";

/// The member under test sits one level below the root, so its own `.`
/// edges route like a member's do.
const M: &str = "/m";
const AUDIENCE: &str = r#"["member:owner","agent:scribe"]"#;
/// The channel node the inner turn names for its answer.
const NODE: &str = "telegram";
const SOURCE: &str = "orga";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
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
                    json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x1316_0000 + n));
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

/// A turn at the member's door, with the budget the ingress hands out there.
fn turn(hop: Value, ctx: Value, text: &str) -> Message {
    MessageBuilder::new(Path::new(M))
        .hop(as_map(&hop))
        .context(as_map(&ctx))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// Headers of the first delivery into `to`, read through the colony (the
/// log queries wait for the writer, TV-A3) after the colony settled.
async fn first_delivery(h: &ColonyHandle, to: &str) -> Option<Value> {
    h.quiesce().await;
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        to_path_prefix: Some(to.to_string()),
        ..Default::default()
    };
    h.read_log(filter)
        .await
        .into_iter()
        .rfind(|row| row.to_path == to)
        .map(|row| meclaw_core::serde_json::from_str(&row.headers_json).unwrap_or(Value::Null))
}

/// Body of the last delivery into `to` (inline bodies only), read like
/// [`first_delivery`].
async fn body_of_delivery(h: &ColonyHandle, to: &str) -> Option<Value> {
    h.quiesce().await;
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        to_path_prefix: Some(to.to_string()),
        ..Default::default()
    };
    h.read_log(filter)
        .await
        .into_iter()
        .rfind(|row| row.to_path == to)
        .and_then(|row| row.body_payload)
        .map(|b| meclaw_core::serde_json::from_str(&b).unwrap_or(Value::Null))
}

struct Run {
    /// The answer at `/delivered`, if one arrived.
    delivered: Option<Message>,
    /// Headers of the first delivery into talky's session keeper.
    at_talky: Option<Value>,
    dead: Vec<String>,
}

async fn one_turn(hop: Value, ctx: Value) -> Run {
    let brain = MockOpenAI::start(vec![canned_chat_completion(REPLY, "stop")]).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build_tree(td.path(), &brain.base_url, &background.base_url);
    let (h, mut delivered_rx, _park) = boot(&td).await;
    h.send(turn(hop, ctx, "Time for the afternoon check-in."))
        .await;
    let delivered = tokio::time::timeout(DEADLINE, delivered_rx.recv())
        .await
        .ok()
        .flatten();
    let keeper = format!("{M}/assistants/scribe/talky/session-keeper");
    let at_talky = first_delivery(&h, &keeper).await;
    let dead = h
        .drain_dead_letters()
        .await
        .iter()
        .map(|d| format!("{d:?}"))
        .collect();
    h.shutdown().await;
    Run {
        delivered,
        at_talky,
        dead,
    }
}

fn ctx_of(m: &Message) -> Value {
    Value::Object(m.headers.context.clone())
}

/// The lock of #1316: the inner turn reaches talky as an inner turn on its
/// own channel, and its answer reaches the node it named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1316_an_inner_turn_reaches_talky_and_its_channel() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let run = one_turn(
        json!({"route": "in_turn", "kind": "inner", "inner_source": SOURCE,
               "deliver_channel_node": NODE, "turn_id": "orga-1316"}),
        json!({"assistant": "scribe", "audience_set": AUDIENCE}),
    )
    .await;
    let at = run
        .at_talky
        .as_ref()
        .unwrap_or_else(|| panic!("the turn reached talky's keeper; dead: {:?}", run.dead));
    assert_eq!(at["hop"]["kind"], "inner", "talky sees an inner turn: {at}");
    assert_eq!(at["hop"]["inner_source"], SOURCE, "{at}");
    assert_eq!(
        at["context"]["channel"],
        format!("inner:{SOURCE}"),
        "the inner door put the turn on its own channel: {at}"
    );
    assert_eq!(at["context"]["channel_node"], NODE, "{at}");
    let answer = run
        .delivered
        .as_ref()
        .unwrap_or_else(|| panic!("the answer reached a channel; dead: {:?}", run.dead));
    assert_eq!(
        ctx_of(answer)["channel_node"],
        NODE,
        "the answer is addressed to the node the inner turn named"
    );
}

/// The counter-probe: a person's turn takes the same road unchanged -- none
/// of the inner keys at talky, its own channel, its answer at its node.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1316_a_persons_turn_crosses_as_before() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let run = one_turn(
        json!({"route": "in_turn", "turn_id": "p-1316"}),
        json!({"assistant": "scribe", "audience_set": AUDIENCE, "channel": "tg:42",
               "channel_node": NODE}),
    )
    .await;
    let at = run
        .at_talky
        .as_ref()
        .unwrap_or_else(|| panic!("the turn reached talky's keeper; dead: {:?}", run.dead));
    for k in ["kind", "inner_source", "deliver_channel_node"] {
        assert!(
            at["hop"].get(k).is_none(),
            "a person's hop carries no `{k}`: {at}"
        );
        assert!(
            at["context"].get(k).is_none() && at["context"].get(format!("fw_{k}")).is_none(),
            "a person's context carries no `{k}`: {at}"
        );
    }
    assert_eq!(at["context"]["channel"], "tg:42", "{at}");
    let answer = run
        .delivered
        .as_ref()
        .unwrap_or_else(|| panic!("the answer reached a channel; dead: {:?}", run.dead));
    assert_eq!(ctx_of(answer)["channel"], "tg:42");
    assert_eq!(ctx_of(answer)["channel_node"], NODE);
}

// ─────────────────────────────────────────────────── the two scripts that rebuild the header

const SCREEN: &str = "templates/firewall/screen/config.json";
const WARDEN: &str = "templates/firewall/warden/config.json";

fn run_script(cell: &str, mut doc: Value, params: Value) -> Vec<Value> {
    doc["params"] = params;
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&repo(cell).to_string_lossy()),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{cell} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice::<Value>(&out.stdout).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

/// The screen's last phase (the rate count came back under the store's own
/// hop): the pass verdict carries the inner keys out of the context, and a
/// person's verdict carries exactly the key set it always carried.
#[test]
fn gh1316_the_screen_hands_the_inner_keys_on_and_a_person_none() {
    if !shipped() {
        return;
    }
    let body = json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]});
    let rate = |extra: Value| {
        let mut ctx = json!({"fw_phase": "rate", "channel": "default", "turn_id": "t1",
                             "fw_body": body.to_string(), "fw_now": "2026-10-25T08:00:00.000000Z",
                             "fw_hold": ""});
        for (k, v) in as_map(&extra) {
            ctx[k] = v;
        }
        json!({"header": {"context": ctx, "hop": {"operation": "select", "rows_affected": 0}},
               "messages": [{"origin": "tool", "type": "tool_result", "id": "fw-rate",
                             "text": "[]"}]})
    };
    let pass = |out: &[Value]| -> Value {
        out.iter()
            .find(|m| m["header"]["route"] == "pass")
            .unwrap_or_else(|| panic!("a pass verdict: {out:?}"))["header"]
            .clone()
    };
    let inner = run_script(
        SCREEN,
        rate(json!({"fw_kind": "inner", "fw_inner_source": SOURCE,
                    "fw_deliver_channel_node": NODE})),
        json!({}),
    );
    let h = pass(&inner);
    assert_eq!(h["kind"], "inner", "{h}");
    assert_eq!(h["inner_source"], SOURCE, "{h}");
    assert_eq!(h["deliver_channel_node"], NODE, "{h}");
    let person = run_script(SCREEN, rate(json!({})), json!({}));
    let keys: Vec<String> = as_map(&pass(&person)).keys().cloned().collect();
    let mut want: Vec<String> = [
        "route",
        "reject_reason",
        "rule_id",
        "channel",
        "user_id",
        "turn_id",
        "hold_id",
        "store_error",
        "store_operation",
        "reject_detail",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.sort();
    let mut keys = keys;
    keys.sort();
    assert_eq!(keys, want, "a person's verdict keeps its key set");
}

/// A held inner turn released later: the warden hands the keys back on the
/// hop under their own names, out of the context the screen parked it with.
#[test]
fn gh1316_a_released_inner_turn_keeps_its_keys() {
    if !shipped() {
        return;
    }
    let parked = json!({"turn_id": "i1", "channel": "default", "audience_set": AUDIENCE,
                        "fw_kind": "inner", "fw_inner_source": SOURCE,
                        "fw_deliver_channel_node": NODE});
    let meta = json!({"hold_id": "h1", "state": "released", "turn_id": "i1",
                      "rule_id": "r1", "reason": "held", "channel": "default",
                      "user_id": "", "expires_at": "", "context": parked});
    let doc = json!({"header": {"context": {
                        "wd_phase": "decide", "store_origin": "fwwarden",
                        "wd_meta": meta.to_string(),
                        "wd_body": json!({"messages": [{"origin": "user", "type": "text",
                                                        "text": "hi"}]}).to_string(),
                        "wd_now": "2026-10-25T08:00:00.000000Z"},
                     "hop": {"operation": "update", "rows_affected": 1}},
                     "messages": []});
    let out = run_script(WARDEN, doc, json!({}));
    let h = &out
        .iter()
        .find(|m| m["header"]["route"] == "pass")
        .unwrap_or_else(|| panic!("a pass: {out:?}"))["header"];
    assert_eq!(h["kind"], "inner", "{h}");
    assert_eq!(h["inner_source"], SOURCE, "{h}");
    assert_eq!(h["deliver_channel_node"], NODE, "{h}");
    assert!(h.get("ctx_fw_kind").is_none(), "{h}");
    assert_eq!(
        h["ctx_audience_set"], AUDIENCE,
        "the rest of the carry is unchanged"
    );
}

// ─────────────────────────── GH #1320: what a conversation tells, out of the member

// WHY (GH #1320, measured in two labs one level up): on this same road
// - an inner turn the model answered with silence (`" "`, GH #1305) died in
//   the `llm` cell as `empty_answer` before the dispatcher could see it, so
//   no `inner_silent` ever left (orga lab, trace 01a175b1-…);
// - `opened` and `closed` of a talky surface were reported dead at the
//   assistant's hive (`hive_no_route`, target `…/assistants/<name>`);
// - the memory's answer to a reader INSIDE the member (`conversations`)
//   left over the member's rim as well and died there (`hive_no_route`,
//   sender `memory-hive/sessions`).
// Every hop below is the shipped one; only the knobs that ship off are on.

/// A reader inside the member (a role next to the assistants); its answer
/// is drawn by the product, here by one test edge to a witness.
const READER_INSIDE: &str = "counselor:review:1320";
/// A reader outside the member, asking at the member's own door.
const READER_OUTSIDE: &str = "app:outside:1320";

/// The member's edges of the conversation events and of the conversations
/// lane, verbatim off `templates/member`.
fn member_event_edges() -> Vec<Value> {
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
            (f == "./assistants" && t == "." && cond.contains("'opened'"))
                || cond.contains("'in_conversations'")
                || cond.contains("'conversations'")
        })
        .collect()
}

/// The road of #1316 with every conversation-event knob on, a keeper that
/// ends a conversation on the next sweep, the member's event edges and one
/// reader inside the member.
fn build_events_tree(root: &std::path::Path, brain: &str, background: &str) {
    build_tree(root, brain, background);
    let m = root.join("main/m");
    let talky = m.join("assistants/scribe/talky");
    for cell in ["dispatcher", "session-keeper/stamp", "curator/writer"] {
        override_params_on_disk(&talky.join(cell), &json!({"conversation_events": "1"}));
    }
    // A sweep ends every conversation that has been silent at all (the
    // precedent of gh895); the night stays closed (quiet_keeper_night).
    override_params_on_disk(&talky.join("session-keeper/close"), &json!({"idle_ms": 1}));
    let mut cfg = read_json(&m.join("config.json"));
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member's edges");
    let extra = member_event_edges();
    assert!(
        extra.len() >= 3,
        "the member's event edges were found: {extra:?}"
    );
    edges.extend(extra);
    // The witness of the inside reader sits outside the tree (it must exist
    // before the boot), drawn by an edge of THIS level like a role's would be.
    edges.push(json!({"from": "./memory-hive", "to": "/delivered",
                      "condition": "has(hop.route) && hop.route == 'conversations' && has(hop.reader) && hop.reader.startsWith('counselor:')"}));
    write_json(&m.join("config.json"), &cfg);
}

struct EventsRun {
    h: ColonyHandle,
    delivered: mpsc::Receiver<Message>,
    park: mpsc::Receiver<Message>,
    _brain: MockOpenAI,
    _background: MockOpenAI,
    _td: tempfile::TempDir,
}

async fn boot_events(brain_says: &str) -> EventsRun {
    boot_events_with(vec![canned_chat_completion(brain_says, "stop")]).await
}

/// [`boot_events`] with the brain's whole script, one answer per request.
async fn boot_events_with(brain_script: Vec<MockResponse>) -> EventsRun {
    let brain = MockOpenAI::start(brain_script).await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build_events_tree(td.path(), &brain.base_url, &background.base_url);
    let (h, delivered, park) = boot(&td).await;
    EventsRun {
        h,
        delivered,
        park,
        _brain: brain,
        _background: background,
        _td: td,
    }
}

fn hop_route(m: &Message) -> &str {
    m.headers
        .hop
        .get("route")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The first message on `rx` whose route is `route`; everything else seen
/// on the way is kept in `seen`. `None` at the hang marker.
async fn next_route(
    rx: &mut mpsc::Receiver<Message>,
    seen: &mut Vec<Message>,
    route: &str,
) -> Option<Message> {
    if let Some(i) = seen.iter().position(|m| hop_route(m) == route) {
        return Some(seen.remove(i));
    }
    let wait = async {
        while let Some(m) = rx.recv().await {
            if hop_route(&m) == route {
                return Some(m);
            }
            seen.push(m);
        }
        None
    };
    tokio::time::timeout(DEADLINE, wait).await.ok().flatten()
}

/// Dead letters of the run that carry one of `routes`.
async fn dead_on(h: &ColonyHandle, routes: &[&str]) -> Vec<String> {
    h.drain_dead_letters()
        .await
        .iter()
        .filter(|d| routes.contains(&hop_route(&d.message)))
        .map(|d| {
            format!(
                "{} from {} to {} ({:?})",
                hop_route(&d.message),
                d.sender_path.as_str(),
                d.original_target.as_str(),
                d.reason
            )
        })
        .collect()
}

/// K-13: an inner turn the model answers with silence is silence at the
/// dispatcher -- `inner_silent` leaves the member, nothing reaches a channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_an_inner_turn_answered_with_silence_leaves_inner_silent() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events(" ").await;
    run.h
        .send(turn(
            json!({"route": "in_turn", "kind": "inner", "inner_source": SOURCE,
                   "deliver_channel_node": NODE, "turn_id": "orga-1320"}),
            json!({"assistant": "scribe", "audience_set": AUDIENCE}),
            "The walk is due in ten minutes.",
        ))
        .await;
    let mut seen = Vec::new();
    let silent = next_route(&mut run.park, &mut seen, "inner_silent").await;
    let dead = dead_on(&run.h, &["inner_silent", "answer", "error"]).await;
    let silent = silent.unwrap_or_else(|| {
        panic!(
            "inner_silent left the member; parked: {:?}; dead: {dead:?}; errors: {:?}",
            seen.iter().map(hop_route).collect::<Vec<_>>(),
            seen.iter()
                .filter(|m| hop_route(m) == "error")
                .map(|m| (&m.headers.hop, &m.body))
                .collect::<Vec<_>>()
        )
    });
    assert_eq!(silent.headers.hop["inner_source"], SOURCE);
    assert_eq!(silent.headers.hop["turn_id"], "orga-1320");
    // The knob that let the brain stay silent came from talky's edge.
    let brain = format!("{M}/assistants/scribe/talky/brain");
    let at_brain = first_delivery(&run.h, &brain)
        .await
        .unwrap_or_else(|| panic!("the inner turn reached the brain"));
    assert_eq!(at_brain["hop"]["silence_ok"], "1", "{at_brain}");
    run.h.quiesce().await;
    assert!(
        run.delivered.try_recv().is_err(),
        "a silence sends nothing to a channel"
    );
    assert!(dead.is_empty(), "{dead:?}");
    run.h.shutdown().await;
}

/// K-13, the other half (#999 F2 stands): an empty answer to a person's
/// turn is still the brain's `empty_answer`, and no silence leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_an_empty_answer_to_a_person_stays_an_error() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events(" ").await;
    run.h
        .send(turn(
            json!({"route": "in_turn", "turn_id": "p-1320"}),
            json!({"assistant": "scribe", "audience_set": AUDIENCE, "channel": "tg:42",
                   "channel_node": NODE}),
            "How was the walk?",
        ))
        .await;
    run.h.quiesce().await;
    // Lock (K-13): a person's turn reaches the brain without the knob, so
    // its message is the one it was before GH #1320.
    let brain = format!("{M}/assistants/scribe/talky/brain");
    let at_brain = first_delivery(&run.h, &brain)
        .await
        .unwrap_or_else(|| panic!("the person's turn reached the brain"));
    assert!(at_brain["hop"].get("silence_ok").is_none(), "{at_brain}");
    let errors = format!("{M}/assistants/scribe/talky/errors");
    let at_errors = first_delivery(&run.h, &errors)
        .await
        .unwrap_or_else(|| panic!("the brain's empty answer went to talky's errors"));
    assert_eq!(at_errors["hop"]["finish_reason"], "error", "{at_errors}");
    assert_eq!(
        at_errors["hop"]["error_code"], "provider_error",
        "{at_errors}"
    );
    // The kind names the brain's empty answer, not any provider error
    // (re-check m3).
    let body = body_of_delivery(&run.h, &errors)
        .await
        .unwrap_or_else(|| panic!("the error carried a body"));
    assert_eq!(body["meta"]["error"]["kind"], "empty_answer", "{body}");
    let mut parked = Vec::new();
    while let Ok(m) = run.park.try_recv() {
        parked.push(m);
    }
    assert!(
        !parked.iter().any(|m| hop_route(m) == "inner_silent"),
        "a person's empty answer is no silence: {:?}",
        parked.iter().map(hop_route).collect::<Vec<_>>()
    );
    run.h.shutdown().await;
}

/// K-12b: `opened` and `closed` of a talky surface nested in an assistant in
/// the container of a member come out of the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_opened_and_closed_leave_the_member() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events(REPLY).await;
    run.h
        .send(turn(
            json!({"route": "in_turn", "turn_id": "p-1320"}),
            json!({"assistant": "scribe", "audience_set": AUDIENCE, "channel": "tg:42",
                   "channel_node": NODE}),
            "How was the walk?",
        ))
        .await;
    let mut seen = Vec::new();
    let opened = next_route(&mut run.park, &mut seen, "opened").await;
    assert!(
        tokio::time::timeout(DEADLINE, run.delivered.recv())
            .await
            .ok()
            .flatten()
            .is_some(),
        "the turn was answered"
    );
    run.h.quiesce().await;
    run.h
        .send(
            MessageBuilder::new(Path::new(&format!("{M}/assistants/scribe/talky")))
                .hop(as_map(&json!({"route": "in_sweep"})))
                .context(Map::new())
                .body(Body::Inline(json!({"messages": []})))
                .ttl(400)
                .build(),
        )
        .await;
    let closed = next_route(&mut run.park, &mut seen, "closed").await;
    let dead = dead_on(&run.h, &["opened", "closed"]).await;
    let opened = opened.unwrap_or_else(|| panic!("opened left the member; dead: {dead:?}"));
    assert_eq!(
        opened.headers.hop["channel"], "tg:42",
        "{:?}",
        opened.headers.hop
    );
    let closed = closed.unwrap_or_else(|| panic!("closed left the member; dead: {dead:?}"));
    assert_eq!(
        closed.headers.hop["channel"], "tg:42",
        "{:?}",
        closed.headers.hop
    );
    assert!(dead.is_empty(), "{dead:?}");
    run.h.shutdown().await;
}

fn ask_conversations(to: &str, reader: &str) -> Message {
    MessageBuilder::new(Path::new(to))
        .hop(as_map(&json!({"route": "in_conversations"})))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": [], "reader": reader,
                                  "audience_set": AUDIENCE})))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// K-14: the memory's `conversations` leaves the member only for a reader
/// outside it; the answer to a reader inside stays inside.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_conversations_leave_the_member_only_for_a_reader_outside() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events(REPLY).await;
    // Inside: the reader's question reaches the memory from within the level.
    run.h
        .send(ask_conversations(
            &format!("{M}/memory-hive"),
            READER_INSIDE,
        ))
        .await;
    let mut none = Vec::new();
    let inside = next_route(&mut run.delivered, &mut none, "conversations")
        .await
        .unwrap_or_else(|| panic!("the inside reader got its answer"));
    assert_eq!(inside.headers.hop["reader"], READER_INSIDE);
    // Outside: the question comes in at the member's door.
    run.h.send(ask_conversations(M, READER_OUTSIDE)).await;
    let mut seen = Vec::new();
    let outside = next_route(&mut run.park, &mut seen, "conversations")
        .await
        .unwrap_or_else(|| panic!("the outside reader got its answer"));
    run.h.quiesce().await;
    while let Ok(m) = run.park.try_recv() {
        seen.push(m);
    }
    let dead = dead_on(&run.h, &["conversations"]).await;
    let mut out_readers: Vec<String> = seen
        .iter()
        .chain(std::iter::once(&outside))
        .filter(|m| hop_route(m) == "conversations")
        .map(|m| m.headers.hop["reader"].as_str().unwrap_or("").to_string())
        .collect();
    out_readers.sort();
    assert_eq!(
        out_readers,
        vec![READER_OUTSIDE.to_string()],
        "only the outside reader's answer left the member"
    );
    assert!(dead.is_empty(), "{dead:?}");
    run.h.shutdown().await;
}

/// The id of the brain's call in the tool round of an inner turn.
const RECALL_CALL: &str = "call_1320";

/// K-13 after a tool round (re-check I1): an inner turn whose brain first
/// calls the member's memory and then says nothing is still silence. The
/// round leaves talky and comes back through a door that does not set
/// `inner_source` again; with the marker cleared on the way out, the second
/// brain call went out as a person's turn and the silence died there as
/// `empty_answer`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_an_inner_turn_silent_after_a_tool_round_leaves_inner_silent() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events_with(vec![
        canned_tool_calls(vec![(
            RECALL_CALL,
            "memory_recall",
            r#"{"query":"the walk"}"#,
        )]),
        canned_chat_completion(" ", "stop"),
    ])
    .await;
    run.h
        .send(turn(
            json!({"route": "in_turn", "kind": "inner", "inner_source": SOURCE,
                   "deliver_channel_node": NODE, "turn_id": "orga-1320-t"}),
            json!({"assistant": "scribe", "audience_set": AUDIENCE}),
            "The walk is due in ten minutes.",
        ))
        .await;
    let mut seen = Vec::new();
    let silent = next_route(&mut run.park, &mut seen, "inner_silent").await;
    let dead = dead_on(&run.h, &["inner_silent", "answer", "error"]).await;
    let asked = run._brain.recorded_requests().await.len();
    let silent = silent.unwrap_or_else(|| {
        panic!(
            "inner_silent left the member after the tool round; brain asked {asked}x; \
             parked: {:?}; dead: {dead:?}",
            seen.iter().map(hop_route).collect::<Vec<_>>()
        )
    });
    assert_eq!(asked, 2, "the round went to the memory and back");
    assert_eq!(silent.headers.hop["inner_source"], SOURCE);
    // The brain's second call, after the round, still carried the knob.
    let brain = format!("{M}/assistants/scribe/talky/brain");
    let at_brain = first_delivery(&run.h, &brain)
        .await
        .unwrap_or_else(|| panic!("the inner turn reached the brain"));
    assert_eq!(at_brain["hop"]["silence_ok"], "1", "{at_brain}");
    run.h.quiesce().await;
    assert!(
        run.delivered.try_recv().is_err(),
        "a silence sends nothing to a channel"
    );
    assert!(dead.is_empty(), "{dead:?}");
    run.h.shutdown().await;
}

/// K-14 along a chain (re-check m5): the context an answer leaves the member
/// with is the context the next message of its chain carries in. The stamp
/// of an outside question must not be part of it -- a reader inside asking
/// with that context would otherwise get its answer carried out as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1320_an_outside_question_leaves_no_stamp_on_its_chain() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut run = boot_events(REPLY).await;
    run.h.send(ask_conversations(M, READER_OUTSIDE)).await;
    let mut seen = Vec::new();
    let outside = next_route(&mut run.park, &mut seen, "conversations")
        .await
        .unwrap_or_else(|| panic!("the outside reader got its answer"));
    assert!(
        !outside.headers.context.contains_key("conversations_asked"),
        "the stamp left the member: {:?}",
        outside.headers.context
    );
    // The chain goes on inside: the next question carries what left.
    let mut next = ask_conversations(&format!("{M}/memory-hive"), READER_INSIDE);
    next.headers.context = outside.headers.context.clone();
    run.h.send(next).await;
    let mut none = Vec::new();
    let inside = next_route(&mut run.delivered, &mut none, "conversations")
        .await
        .unwrap_or_else(|| panic!("the inside reader got its answer"));
    assert_eq!(inside.headers.hop["reader"], READER_INSIDE);
    run.h.quiesce().await;
    while let Ok(m) = run.park.try_recv() {
        seen.push(m);
    }
    let left: Vec<&str> = seen
        .iter()
        .filter(|m| hop_route(m) == "conversations")
        .map(|m| m.headers.hop["reader"].as_str().unwrap_or(""))
        .collect();
    assert!(
        left.is_empty(),
        "the inside answer of the chain left the member: {left:?}"
    );
    run.h.shutdown().await;
}

/// talky's edges, verbatim off the shipped template.
fn talky_edges() -> Vec<Value> {
    read_json(&repo("templates/talky/config.json"))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The exits of talky that carry `inner_source` out: the round trips whose
/// answer comes back through a door that does not set it again (the CARRIED
/// row `talky/inner_source` of gh494). A pair (from, route).
const INNER_SOURCE_ROUND_TRIPS: [(&str, &str); 4] = [
    ("./curator", "recall"),
    ("./curator", "consult"),
    ("./curator", "research"),
    ("./dispatcher", "tool"),
];

/// Lock (GH #1320, re-check C1/I1): `inner_source` leaves talky only on its
/// round trips; every other exit deletes it, so the marker of an inner turn
/// never rides on a chain past the turn.
#[test]
fn gh1320_inner_source_leaves_talky_only_on_its_round_trips() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mut kept = Vec::new();
    let mut leaks = Vec::new();
    for e in talky_edges().iter().filter(|e| e["to"] == ".") {
        let from = e["from"].as_str().unwrap_or("");
        let cond = e["condition"].as_str().unwrap_or("");
        let deletes = e["modifier"]["delete_context"]
            .as_array()
            .is_some_and(|d| d.iter().any(|k| k == "inner_source"));
        let round = INNER_SOURCE_ROUND_TRIPS.iter().any(|(f, r)| {
            *f == from
                && cond.contains(&format!("hop.route == '{r}'"))
                && !cond.starts_with("has(hop.refused_subscriber)")
        });
        match (round, deletes) {
            (true, false) => kept.push(format!("{from} {cond}")),
            (true, true) => leaks.push(format!("round trip clears it: {from} {cond}")),
            (false, false) => leaks.push(format!("exit keeps it: {from} {cond}")),
            (false, true) => {}
        }
    }
    assert!(leaks.is_empty(), "{leaks:#?}");
    // recall twice (with and without a gap ask), consult/research, tool.
    assert_eq!(kept.len(), 4, "{kept:#?}");
}

/// Lock (re-check m4): the person's and the inner edge of each round
/// (`./curator -> ./brain`, `./curator -> ./quick`) are the same edge but for
/// the `inner_source` test in the condition and `set_hop.silence_ok` on the
/// inner one -- a change to one has to reach the other.
#[test]
fn gh1320_the_person_and_inner_round_edges_differ_only_in_the_knob() {
    if !shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const PERSON: &str = " && !(has(context.inner_source) && context.inner_source != '')";
    const INNER: &str = " && has(context.inner_source) && context.inner_source != ''";
    let edges = talky_edges();
    for to in ["./brain", "./quick"] {
        let pair: Vec<&Value> = edges
            .iter()
            .filter(|e| e["from"] == "./curator" && e["to"] == to)
            .collect();
        let person: Vec<&&Value> = pair
            .iter()
            .filter(|e| e["condition"].as_str().unwrap_or("").ends_with(PERSON))
            .collect();
        let inner: Vec<&&Value> = pair
            .iter()
            .filter(|e| e["condition"].as_str().unwrap_or("").ends_with(INNER))
            .collect();
        assert_eq!((person.len(), inner.len()), (1, 1), "{to}: {pair:#?}");
        let (mut p, mut i) = ((*person[0]).clone(), (*inner[0]).clone());
        assert!(p["modifier"].get("set_hop").is_none(), "{to}: {p}");
        assert_eq!(
            i["modifier"]["set_hop"],
            json!({"silence_ok": "'1'"}),
            "{to}"
        );
        i["modifier"]
            .as_object_mut()
            .expect("modifier")
            .remove("set_hop");
        let strip = |e: &mut Value, tail: &str| {
            let c = e["condition"].as_str().unwrap_or("");
            e["condition"] = Value::String(c.strip_suffix(tail).unwrap_or(c).to_string());
        };
        strip(&mut p, PERSON);
        strip(&mut i, INNER);
        assert_eq!(p, i, "{to}: the pair drifted apart");
    }
}
