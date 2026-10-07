//! GH #995 -- every tool result of a run names its tool, measured at the app.
//!
//! An app that declares `runs: {at: "./inbox", brain: "cogny"}` hands the core
//! a turn; the core (a scripted model) calls one memory tool, then a library
//! tool and an object tool in ONE round, then the file space's workspace tools
//! one round after the other -- `file_ws_open`, `file_create` (a main-line
//! write, which starts a derive job), `file_ws_exec`, `file_ws_commit`,
//! `file_ws_export`, `file_ws_push` -- and answers. Each answerer is the
//! SHIPPED one: the member's memory, its librarian (with its graph space), its
//! objects and its file space with the git projection, wired by the member's
//! own edges and the installation's edges the shipped recipe renders.
//!
//! What the app hears, read off the colony's own `message_log` at the app's
//! cell (the receiver):
//!
//! - `gh995_every_run_tool_result_names_its_tool`: exactly one
//!   `run_tool_result` per call id, and its hop carries `tool_name` -- the
//!   exact name of the call it answers. Measured before the fix (lab run of a
//!   run-driven app, 2026-10-04): every `run_tool_result` of the file space and the
//!   library carried no `tool_name`, so an app could not tell a test run from
//!   a commit without parsing the result text.
//! - `gh995_the_body_text_is_the_result`: the body is the envelope the brain
//!   gets -- ONE message, `origin` `tool`, `type` `tool_result`, `id` the call
//!   id -- and its `text` is the result: for a file or library tool the
//!   answer as a JSON object whose `ok` agrees with the hop's `ok`.
//! - `gh995_an_observer_hears_every_holder_by_name`: a second app observes
//!   `file_ws_exec` and `lib_symbol` (`observes_tool_results` in its object
//!   form) for the run's generation, a third the same tools for another
//!   generation of the member. The first hears each of the two results exactly
//!   once, restamped `in_tool_result` and named; the third hears nothing.
//!   Before: the recipe tapped only the tool hive, the memory and the apps, and
//!   filtered on a `hop.tool_name` the residents never write -- an observer of
//!   `file_ws_exec` installed green and heard nothing.
//! - `gh995_a_derive_job_behind_a_run_keeps_its_budget`: the run's
//!   `file_create` moves a head, so the file space derives the file. Every
//!   `in_derive` arrives at `./derive` with the full colony TTL (a door) and
//!   no delivery of the job carries less than `JOB_FLOOR`. Before: the job
//!   inherited what the tool chain had left (measured in a lab run down to 11).
//!
//! Whether a call succeeds is not the point: a refused call is answered too,
//! and its answer must name its tool like any other. The tree is the one of
//! `gh981_a_run_reads_the_library_in_the_members_round` plus the member's
//! `./file-space` (`templates/member/file-space`, resolved like the mutation
//! door resolves a `ref`) with its projection set up as in `gh980` (a base
//! path, the git cell's own repositories beside it, an empty bare remote
//! `origin`, `python3` permitted). No provider, no network.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self, Map, Value, json};
use meclaw_core::{Body, Headers, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use road::{
    BACKGROUND_REPLY, DEADLINE, Stubs, configs_under, copy_resolved, dead_letters, factories,
    member_memory_edges, point_llms_at_stubs, quiet_timers, read_json, repo, rim_emits, write_env,
    write_json,
};
use std::collections::{BTreeMap, HashMap};
use tokio::sync::{mpsc, oneshot};

// GH #1061: the road support already loads it (`duplicate_mod`).
use road::organism_assistant;

const APP: &str = "probe-app";
const PERSON: &str = "owner";
const RUN: &str = "r-995-1";
const CORE_BRAIN: &str = "assistants/scribe/cogny/brain";
const DONE: &str = "The run is done.";
/// The observer of the run's generation, and the one of another generation.
const EAR: &str = "probe-ear";
const DEAF: &str = "probe-deaf";
const OTHER_GEN: &str = "quiet";
/// The tools both observers name.
const OBSERVED: [&str; 2] = ["file_ws_exec", "lib_symbol"];
/// What the colony keeps back for the way home (OR-BD-11, gh929): no delivery
/// of a derive job may sink below it.
const JOB_FLOOR: i64 = 16;

/// One call: (call id, tool, arguments, the answerer the run hears it from).
type Call = (&'static str, &'static str, &'static str, &'static str);

/// The rounds of the run, in order; the second carries two calls.
const ROUNDS: [&[Call]; 8] = [
    &[(
        "call-995-mem",
        "memory_recall",
        r#"{"query":"probe"}"#,
        "memory-hive",
    )],
    &[
        (
            "call-995-lib",
            "lib_symbol",
            r#"{"name":"probe"}"#,
            "librarian",
        ),
        (
            "call-995-obj",
            "object_find",
            r#"{"query":"probe"}"#,
            "objects",
        ),
    ],
    &[(
        "call-995-open",
        "file_ws_open",
        r#"{"name":"T-995"}"#,
        "file-space",
    )],
    &[(
        "call-995-create",
        "file_create",
        r##"{"path":"/probe.md","text":"# Probe\n"}"##,
        "file-space",
    )],
    &[(
        "call-995-exec",
        "file_ws_exec",
        r#"{"ws":"T-995","argv":["python3","-c","print(995)"]}"#,
        "file-space",
    )],
    &[(
        "call-995-commit",
        "file_ws_commit",
        r#"{"ws":"T-995","note":"probe"}"#,
        "file-space",
    )],
    &[(
        "call-995-export",
        "file_ws_export",
        r#"{"root":"/","note":"probe"}"#,
        "file-space",
    )],
    &[(
        "call-995-push",
        "file_ws_push",
        r#"{"root":"/"}"#,
        "file-space",
    )],
];

/// Every call of the run, in order.
fn calls() -> impl Iterator<Item = Call> {
    ROUNDS.iter().flat_map(|r| r.iter().copied())
}

/// Every template this test reads.
fn shipped() -> bool {
    if !road::shipped() {
        return false;
    }
    [
        "templates/librarian/config.json",
        "templates/graph-space/config.json",
        "templates/file-space/config.json",
        "templates/member/librarian/config.json",
        "templates/member/graph-space/config.json",
        "templates/member/file-space/config.json",
        "templates/member/objects/config.json",
        "templates/objects/config.json",
        "templates/builder/recipes/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

/// The app's runner: on `go` it raises `run_turn` with the hop's `run_id`.
const RUNNER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
env = doc.get("envelope") or {}
hop = (env.get("header") or {}).get("hop") or {}
head = {"route": "run_turn"}
if hop.get("run_id"):
    head["run_id"] = hop["run_id"]
msgs = (doc.get("body") or {}).get("messages") or []
sys.stdout.write(json.dumps([{"header": head, "messages": msgs}]))
"#;

/// The app's inbox: says what it heard, so the test has an event to wait on.
const INBOX: &str = r#"
import sys, json
doc = json.load(sys.stdin)
env = doc.get("envelope") or {}
hop = (env.get("header") or {}).get("hop") or {}
head = {"route": "heard", "heard": str(hop.get("route") or "")}
sys.stdout.write(json.dumps([{"header": head, "messages": []}]))
"#;

/// A `code` cell with a fixed script.
fn code(script: &str, purpose: &str) -> Value {
    json!({"cell": {"type": "code"},
           "params": {"runner": "python3", "script_inline": script,
                      "external_timeout_ms": 5000},
           "contract": {
               "version": "1.0.0",
               "settings": {},
               "multi_send_capable": true,
               "emits": {"body": {"messages": {"type": "array", "required": false}}},
               "consumes": {"body": {"messages": {"type": "array", "required": false}}},
               "capabilities": ["shell:exec"]},
           "description": {"purpose": purpose, "use_when": "Test fixture only.",
                           "not_in_scope": "Not a template."}})
}

fn route(r: &str) -> String {
    format!("has(hop.route) && hop.route == '{r}'")
}

/// An observing app: one `ear` that hears the observed results and says
/// nothing. The substrate judges the LANE of a v-lane fan-out, so the contract
/// accepts `tool_result` at the ear although the edge restamps the route.
fn build_observer(main: &std::path::Path, name: &str) {
    let app = main.join("apps").join(name);
    write_json(
        &app.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {
            "contract": {
                "accepts": [
                    {"route": "tool_result", "at": ["./ear"],
                     "because": "an observed tool result, restamped in_tool_result"}],
                "emits": [
                    {"route": "heard", "because": "never sent; the log is the evidence"}]},
            "graph": {"edges": []}}}),
    );
    write_json(
        &app.join("ear/config.json"),
        &code(
            "import sys\nsys.stdout.write('[]')\n",
            "hears observed results",
        ),
    );
}

/// The probe app: a runner and an inbox, no tool of its own.
fn build_app(main: &std::path::Path) {
    let app = main.join("apps").join(APP);
    write_json(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    write_json(
        &main.join("channels/config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    write_json(
        &app.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {
            "contract": {
                "accepts": [
                    {"route": "go", "because": "the test starts a run"},
                    {"route": "run_tool_result", "at": ["./inbox"], "because": "a run's tool results"},
                    {"route": "run_answer", "at": ["./inbox"], "because": "a run's end"}],
                "emits": [
                    {"route": "run_turn", "because": "the app hands the core a run"},
                    {"route": "heard", "because": "what the inbox heard, for the test"}]},
            "graph": {"edges": [
                {"from": ".", "to": "./runner", "condition": route("go")},
                {"from": "./runner", "to": ".", "condition": route("run_turn")},
                {"from": "./inbox", "to": ".", "condition": route("heard")}]}}}),
    );
    write_json(
        &app.join("runner/config.json"),
        &code(RUNNER, "raises run_turn"),
    );
    write_json(
        &app.join("inbox/config.json"),
        &code(INBOX, "hears the run"),
    );
}

/// The member's own edges between `holder` and its generations (and, for the
/// library, its graph space), verbatim off `templates/member`.
fn member_edges_of(holder: &str, with: &[&str]) -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let (f, t) = (
                e["from"].as_str().unwrap_or_default(),
                e["to"].as_str().unwrap_or_default(),
            );
            (f == holder && with.contains(&t)) || (t == holder && with.contains(&f))
        })
        .collect()
}

/// The owner's side of the file space, as in `gh980`: every cell with a
/// `base_path` gets the run's directory and the write grant on it; `git`
/// names `origin` and keeps its repositories beside `base_path`; `mat`
/// permits `python3`; `schemas` puts the projection tools on the menu;
/// summaries and embeddings stay off.
fn owner_overrides(main: &std::path::Path, base: &std::path::Path, bare: &std::path::Path) {
    let mut files = Vec::new();
    configs_under(&main.join("file-space"), &mut files);
    let git_dir = base.parent().expect("a parent").join("git-dirs");
    std::fs::create_dir_all(&git_dir).expect("git_dir");
    for f in files {
        let mut cfg = read_json(&f);
        let dir = f.parent().expect("a cell directory");
        if cfg["params"].get("base_path").is_some() {
            cfg["params"]["base_path"] = json!(base);
            cfg["params"]["sandbox"]["filesystem"]["write"] = json!([base]);
        }
        if dir.ends_with("projection/git") {
            cfg["params"]["remotes"] = json!({"origin": bare});
            cfg["params"]["git_dir"] = json!(git_dir);
            let remotes = bare.parent().expect("a remote's directory");
            cfg["params"]["sandbox"]["filesystem"]["write"] = json!([base, &git_dir, remotes]);
        }
        if dir.ends_with("projection/mat") {
            cfg["params"]["exec_allow"] = json!(["python3"]);
        }
        if dir.ends_with("file-space/schemas") {
            cfg["params"]["projection_tools"] = json!("1");
        }
        if dir.ends_with("derive") {
            cfg["params"]["summary_on_commit"] = json!("0");
            cfg["params"]["embed"] = json!("0");
        }
        write_json(&f, &cfg);
    }
}

fn git_init_bare(at: &std::path::Path) {
    std::fs::create_dir_all(at.parent().expect("a parent")).expect("the remote's directory");
    let ok = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(at)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .expect("git")
        .success();
    assert!(ok, "an empty bare remote at {}", at.display());
}

fn build(td: &tempfile::TempDir, stubs: &Stubs) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    // A second generation of the member: it never runs, it only stands, so
    // an observer installed for it has the generation's own `./tools`.
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants").join(OTHER_GEN),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    for holder in ["librarian", "graph-space", "file-space", "objects"] {
        copy_resolved(
            &repo(&format!("templates/member/{holder}")),
            &main.join(holder),
            0,
        );
    }
    let base = root.join("projection");
    std::fs::create_dir_all(&base).expect("base_path");
    let bare = root.join("remotes").join("remote.git");
    git_init_bare(&bare);
    owner_overrides(&main, &base, &bare);
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    build_app(&main);
    build_observer(&main, EAR);
    build_observer(&main, DEAF);
    let mut edges = member_memory_edges();
    edges.extend(member_edges_of(
        "./librarian",
        &["./assistants", "./graph-space"],
    ));
    edges.extend(member_edges_of("./file-space", &["./assistants"]));
    edges.extend(member_edges_of(
        "./objects",
        &["./assistants", "./graph-space", "./memory-hive"],
    ));
    let member = read_json(&repo("templates/member/config.json"));
    for e in member["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        if e["from"] == json!("./apps") && e["to"] == json!("./assistants") {
            edges.push(e.clone());
        }
    }
    edges.push(json!({"from": format!("./apps/{APP}"), "to": "/sink",
                      "condition": route("heard")}));
    edges.push(json!({"from": "./assistants", "to": "/park",
                      "condition": "has(hop.route)", "default": true}));
    // Whatever a holder says that no member edge here takes (a head move, an
    // index pull, an answer to no question of this run) is drained, never
    // dead-lettered.
    for holder in ["./librarian", "./graph-space", "./file-space", "./objects"] {
        edges.push(json!({"from": holder, "to": "/park",
                          "condition": "has(hop.route)", "default": true}));
    }
    for lane in rim_emits("memory-hive") {
        if !["bundle", "tool_result", "tool_schemas", "reject"].contains(&lane.as_str()) {
            edges.push(json!({"from": "./memory-hive", "to": "/park",
                              "condition": route(&lane)}));
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
    write_env(root, &main, &stubs.background);
}

/// The residents this member has.
const RESIDENTS: [&str; 4] = ["memory-hive", "librarian", "file-space", "objects"];

/// The edges the shipped recipe renders for one app of this member.
fn rendered(app: &str, generation: &str, declaration: Value) -> Value {
    let out = emit_all(
        &shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("utf-8"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {
                "scope": "/", "app": app, "template": format!("{app}@1.0.0"),
                "screen": "display", "generation": generation,
                "ctx": {"member_person": PERSON},
                "residents_present": RESIDENTS,
                "declaration": declaration}}).to_string()}],
        }),
    );
    let first = out.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"][0]["diff"]["add_edges"].clone()
}

/// The installation's edges: the run-driven app and the two observers.
fn installation() -> Value {
    let edges = rendered(
        APP,
        "scribe",
        json!({"runs": {"at": "./inbox", "brain": "cogny"}}),
    );
    for holder in ["./memory-hive", "./librarian", "./file-space", "./objects"] {
        let tapped = edges
            .as_array()
            .expect("the installation's edges")
            .iter()
            .filter(|e| e["from"] == json!(holder) && e["lane"] == json!("run_tool_result"))
            .count();
        assert_eq!(
            tapped, 1,
            "the recipe taps the tool results of {holder}: {edges:#}"
        );
    }
    let mut all = edges.as_array().cloned().expect("the installation's edges");
    for (app, generation) in [(EAR, "scribe"), (DEAF, OTHER_GEN)] {
        let observer = rendered(
            app,
            generation,
            json!({"observes_tool_results": {"at": "./ear", "tools": OBSERVED}}),
        );
        all.extend(observer.as_array().cloned().expect("the observer's edges"));
    }
    json!({"scope": "/", "diff": {"add_edges": all}})
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

fn go(text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("go"));
    hop.insert("run_id".into(), json!(RUN));
    MessageBuilder::new(Path::new(&format!("/apps/{APP}")))
        .headers(Headers::from_parts(Map::new(), hop))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// One logged delivery: where to, its hop, its body.
#[derive(Clone, Debug)]
struct Row {
    from: String,
    to: String,
    hop: Value,
    body: Value,
    ttl: i64,
}

impl Row {
    fn h(&self, k: &str) -> String {
        match &self.hop[k] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }
}

fn rows(root: &std::path::Path) -> Vec<Row> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare(
            "SELECT from_path, to_path, headers, body_payload, ttl \
             FROM message_log ORDER BY rowid",
        )
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, h, b, ttl)| {
        let h: Value = serde_json::from_str(&h).unwrap_or(Value::Null);
        let body = b
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or(Value::Null);
        Row {
            from,
            to,
            hop: h["hop"].clone(),
            body,
            ttl,
        }
    })
    .collect()
}

struct Done {
    rows: Vec<Row>,
    dead: Vec<(String, String, String)>,
}

/// The core's script: the calls of each round, then the answer.
fn brain() -> Vec<MockResponse> {
    let mut out: Vec<MockResponse> = ROUNDS
        .iter()
        .map(|r| {
            canned_tool_calls(
                r.iter()
                    .map(|(id, name, args, _)| (*id, *name, *args))
                    .collect(),
            )
        })
        .collect();
    out.push(canned_chat_completion(DONE, "stop"));
    out
}

/// Boot, lay the installation, send `go`, wait until the app heard a
/// `run_answer`, read the log.
async fn run() -> Done {
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let core = MockOpenAI::start(brain()).await;
    let stubs = Stubs {
        scripted: HashMap::from([(CORE_BRAIN.to_string(), core.base_url.clone())]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs);
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, mut sink) = mpsc::channel::<Message>(64);
    let (park_tx, mut park) = mpsc::channel::<Message>(4096);
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
        .expect("the member road, its holders and the app must boot");
    let outcome = mutate(&h, installation()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the installation's edges were not committed: {outcome:?}"
    );
    h.send(go("Run the probe.")).await;
    // Waits on the event (the app heard the end), bounded by DEADLINE per
    // message -- no fixed window.
    while let Ok(Some(m)) = tokio::time::timeout(DEADLINE, sink.recv()).await {
        if m.headers.hop.get("heard") == Some(&json!("run_answer")) {
            break;
        }
    }
    // The derive job the write started ends with `source_described` leaving
    // the file space (drained to the park here). Waits on that event, bounded
    // by DEADLINE per message; a job that never ends is the lock's finding.
    while let Ok(Some(m)) = tokio::time::timeout(DEADLINE, park.recv()).await {
        if m.headers.hop.get("route") == Some(&json!("source_described")) {
            break;
        }
    }
    h.shutdown().await;
    Done {
        rows: rows(td.path()),
        dead: dead_letters(td.path()),
    }
}

/// The `run_tool_result`s at the app's cell, by call id.
fn heard(done: &Done) -> BTreeMap<String, Vec<Row>> {
    let inbox = format!("/apps/{APP}/inbox");
    let mut out: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for r in &done.rows {
        if r.to == inbox && r.h("route") == "run_tool_result" && r.h("run_id") == RUN {
            out.entry(r.h("tool_call_id")).or_default().push(r.clone());
        }
    }
    out
}

/// The run ended at the app, and every call was answered there exactly once.
fn every_call_answered(done: &Done) -> BTreeMap<String, Vec<Row>> {
    let inbox = format!("/apps/{APP}/inbox");
    let trail: Vec<String> = done
        .rows
        .iter()
        .filter(|r| r.to.starts_with("/apps/") || r.h("route") == "tool_result")
        .map(|r| format!("{} {}", r.to, r.hop))
        .collect();
    let ended = done
        .rows
        .iter()
        .filter(|r| r.to == inbox && r.h("route") == "run_answer" && r.h("run_id") == RUN)
        .count();
    assert_eq!(
        ended, 1,
        "the run ended at the app: {trail:#?}\ndead letters: {:#?}",
        done.dead
    );
    let by_call = heard(done);
    for (id, name, _, answerer) in calls() {
        let got = by_call.get(id).cloned().unwrap_or_default();
        assert_eq!(
            got.len(),
            1,
            "one run_tool_result for {name} ({id}) at the app: {trail:#?}"
        );
        assert_eq!(got[0].h("answerer"), answerer, "{name}: {:?}", got[0].hop);
    }
    by_call
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh995_every_run_tool_result_names_its_tool() {
    if !shipped() {
        return;
    }
    let done = run().await;
    let by_call = every_call_answered(&done);
    for (id, name, _, _) in calls() {
        let r = &by_call[id][0];
        assert_eq!(
            r.h("tool_name"),
            name,
            "the run_tool_result of {id} names the tool the call named: hop {}",
            r.hop
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh995_the_body_text_is_the_result() {
    if !shipped() {
        return;
    }
    let done = run().await;
    let by_call = every_call_answered(&done);
    for (id, name, _, _) in calls() {
        let r = &by_call[id][0];
        let msgs = r.body["messages"].as_array().cloned().unwrap_or_default();
        assert_eq!(msgs.len(), 1, "{name}: one message in the body: {}", r.body);
        let m = &msgs[0];
        assert_eq!(m["origin"], json!("tool"), "{name}: {m}");
        assert_eq!(m["type"], json!("tool_result"), "{name}: {m}");
        assert_eq!(m["id"], json!(id), "{name}: the call id: {m}");
        let text = m["text"].as_str().unwrap_or_default();
        assert!(!text.is_empty(), "{name}: the result text: {m}");
        if name.starts_with("file_") || name.starts_with("lib_") || name.starts_with("object_") {
            let result: Value = serde_json::from_str(text)
                .unwrap_or_else(|e| panic!("{name}: the text is the result as JSON ({e}): {text}"));
            assert!(result.is_object(), "{name}: a JSON object: {text}");
            assert_eq!(
                result["ok"], r.hop["ok"],
                "{name}: the result's `ok` is the hop's: {text} / {}",
                r.hop
            );
        }
    }
}

/// What `app`'s ear heard, by call id.
fn heard_by(done: &Done, app: &str) -> BTreeMap<String, Vec<Row>> {
    let ear = format!("/apps/{app}/ear");
    let mut out: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for r in &done.rows {
        if r.to == ear {
            out.entry(r.h("tool_call_id")).or_default().push(r.clone());
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh995_an_observer_hears_every_holder_by_name() {
    if !shipped() {
        return;
    }
    let done = run().await;
    every_call_answered(&done);
    let got = heard_by(&done, EAR);
    let said: Vec<String> = got
        .values()
        .flatten()
        .map(|r| format!("{} -> {} {}", r.from, r.to, r.hop))
        .collect();
    for (id, name, _, answerer) in calls() {
        let rows = got.get(id).cloned().unwrap_or_default();
        if !OBSERVED.contains(&name) {
            assert!(
                rows.is_empty(),
                "the observer hears only the tools it names, not {name}: {said:#?}"
            );
            continue;
        }
        assert_eq!(
            rows.len(),
            1,
            "the observer hears the result of {name} ({id}) exactly once: {said:#?}"
        );
        let r = &rows[0];
        assert_eq!(r.h("route"), "in_tool_result", "{name}: {}", r.hop);
        assert_eq!(r.h("tool_name"), name, "{name}: named: {}", r.hop);
        assert_eq!(
            r.from,
            format!("/{answerer}"),
            "{name}: heard off its holder: {}",
            r.hop
        );
    }
    let other = heard_by(&done, DEAF);
    assert!(
        other.is_empty(),
        "an observer of another generation hears nothing of this run: {other:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh995_a_derive_job_behind_a_run_keeps_its_budget() {
    if !shipped() {
        return;
    }
    let done = run().await;
    every_call_answered(&done);
    let derive = "/file-space/derive";
    let job: Vec<&Row> = done
        .rows
        .iter()
        .filter(|r| r.from == derive || r.to == derive)
        .collect();
    let said: Vec<String> = job
        .iter()
        .map(|r| format!("ttl={:3} {} -> {} [{}]", r.ttl, r.from, r.to, r.h("route")))
        .collect();
    let doors: Vec<&&Row> = job
        .iter()
        .filter(|r| r.to == derive && r.h("route") == "in_derive")
        .collect();
    assert!(
        !doors.is_empty(),
        "the run's file_create starts a derive job: {said:#?}"
    );
    for r in &doors {
        assert_eq!(
            r.ttl,
            MESSAGE_DEFAULT_TTL as i64 - 1,
            "an in_derive is a door, it arrives with the colony TTL: {said:#?}"
        );
    }
    let low: Vec<&String> = job
        .iter()
        .zip(&said)
        .filter(|(r, _)| r.ttl < JOB_FLOOR)
        .map(|(_, s)| s)
        .collect();
    assert!(
        low.is_empty(),
        "no delivery of the derive job sinks below {JOB_FLOOR}: {low:#?}\nthe job: {said:#?}"
    );
    eprintln!(
        "gh995 derive: deliveries={} doors={} min_ttl={}",
        job.len(),
        doors.len(),
        job.iter().map(|r| r.ttl).min().unwrap_or_default()
    );
}
