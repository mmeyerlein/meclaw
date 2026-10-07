//! GH #981 -- a run reads the member's library in the member's round.
//!
//! An app that declares `runs: {at: "./inbox", brain: "cogny"}` hands the core
//! a turn; the core calls `lib_symbol`, which the member's librarian answers
//! by asking its graph space (`pull` `lib:g:` restamped `in_graph`). The graph
//! space reads the round of a question off `context.audience_now` first, else
//! `context.audience_set` (`templates/graph-space/README.md`, `in_graph`). The
//! app's own context names a SMALLER round in `audience_now` (the generation
//! alone -- fewer people, so more rows covered); the run's door must write the
//! member's round into BOTH keys, or the app's round reaches the graph through
//! the library (review C-1 of GH #965, review of GH #981).
//!
//! What the test reads, all of it off the colony's own `message_log`:
//!
//! - the core's turn as its collector got it: `in_turn` with the member's
//!   round in `audience_now`;
//! - one `run_tool_result` at the app's cell answered by `librarian`, with the
//!   `run_id`;
//! - at least one delivery into the graph space (the library's `in_graph`
//!   question), and EVERY delivery into the graph space carrying the member's
//!   round in `audience_now` and `audience_set` and the run's `run_id`.
//!
//! The tree is the one of `gh981_a_run_reaches_the_brain_and_ends_at_the_app`
//! (the member road of `support/gh929_member_road.rs`: the generation
//! `scribe`, the member's memory, the member's edges between them, the app
//! container with the probe app, an empty channels container) plus the
//! member's `./librarian` and `./graph-space` (`templates/member/librarian`,
//! `templates/member/graph-space`, resolved like the mutation door resolves a
//! `ref`) with the member's own edges between them and the generation, drawn
//! verbatim off `templates/member/config.json`. The graph space starts empty:
//! `resolve` answers with no items, and the question still crosses the seam.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self, Map, Value, json};
use meclaw_core::{Body, Headers, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, emit_all, override_params_on_disk, shipped_script};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};
use road::{
    BACKGROUND_REPLY, DEADLINE, SURFACES, Stubs, copy_resolved, dead_letters, factories,
    member_memory_edges, point_llms_at_stubs, quiet_timers, read_json, repo, rim_emits, write_env,
    write_json,
};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

// GH #1061: the road support already loads it; a second `mod` is clippy's
// `duplicate_mod`.
use road::organism_assistant;

const APP: &str = "probe-app";
const PERSON: &str = "owner";
/// The member's round as the recipe writes it for this installation
/// (`'["agent:<gen>","member:<person>"]'`, `install_app`).
const ROUND: &str = r#"["agent:scribe","member:owner"]"#;
const RUN: &str = "r-981-lib";
const CORE_BRAIN: &str = "assistants/scribe/cogny/brain";
const LIB_TOOL: &str = "lib_symbol";
const GRAPH: &str = "/graph-space";
const DONE: &str = "The run is done.";

/// Every template this test reads.
fn shipped() -> bool {
    if !road::shipped() {
        return false;
    }
    [
        "templates/librarian/config.json",
        "templates/graph-space/config.json",
        "templates/member/librarian/config.json",
        "templates/member/graph-space/config.json",
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
for k in ("run_id", "answerer", "tool_name"):
    if k in hop:
        head["heard_" + k] = str(hop[k])
sys.stdout.write(json.dumps([{"header": head, "messages": []}]))
"#;

/// A `code` cell with a fixed script (the shape of the sibling lock).
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

/// A JSON string, or `""`.
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// Is there an edge `from -> to` whose condition names `needle`?
fn drawn(edges: &[Value], from: &str, to: &str, needle: &str) -> bool {
    edges.iter().any(|e| {
        e["from"] == json!(from) && e["to"] == json!(to) && text(&e["condition"]).contains(needle)
    })
}

/// The member's own edges between its library, its graph space and its
/// generations, verbatim off `templates/member`.
fn member_library_edges() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    let edges: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            matches!(
                (e["from"].as_str(), e["to"].as_str()),
                (Some("./librarian"), Some("./graph-space"))
                    | (Some("./graph-space"), Some("./librarian"))
                    | (Some("./assistants"), Some("./librarian"))
                    | (Some("./librarian"), Some("./assistants"))
            )
        })
        .collect();
    assert!(
        drawn(&edges, "./librarian", "./graph-space", "lib:g:"),
        "the member draws the library's question to its graph space: {edges:#?}"
    );
    assert!(
        drawn(&edges, "./assistants", "./librarian", "lib_"),
        "the member draws the `lib_` tool calls to its library: {edges:#?}"
    );
    edges
}

fn build(td: &tempfile::TempDir, stubs: &Stubs) {
    let root = td.path();
    let main = root.join("main");
    copy_resolved(
        &repo("templates/assistant"),
        &main.join("assistants/scribe"),
        0,
    );
    copy_resolved(&repo("templates/memory-hive"), &main.join("memory-hive"), 0);
    copy_resolved(
        &repo("templates/member/librarian"),
        &main.join("librarian"),
        0,
    );
    copy_resolved(
        &repo("templates/member/graph-space"),
        &main.join("graph-space"),
        0,
    );
    let grown = organism_assistant::at_the_container(&read_json(&repo(
        "examples/organism/grow-assistant.json",
    )));
    write_json(
        &main.join("assistants/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": grown["diff"]["add_edges"].clone()}}}),
    );
    for s in SURFACES {
        override_params_on_disk(
            &main.join(format!("assistants/scribe/{s}/session-keeper/close")),
            &json!({"idle_ms": 1}),
        );
    }
    build_app(&main);
    let mut edges = member_memory_edges();
    edges.extend(member_library_edges());
    // The member's own way back from its apps (`templates/member`).
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
    // Whatever the library or the graph space says that no member edge here
    // takes (an answer to no question of this run, a pull of an index that
    // never runs) is drained, never dead-lettered.
    for holder in ["./librarian", "./graph-space"] {
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

/// The installation's edges, rendered by the shipped recipe for this member
/// (the colony root) with the library among its residents, without the node:
/// the app already stands.
fn installation() -> Value {
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
                "scope": "/", "app": APP, "template": format!("{APP}@1.0.0"),
                "screen": "display", "generation": "scribe",
                "ctx": {"member_person": PERSON},
                "residents_present": ["memory-hive", "librarian"],
                "declaration": {"runs": {"at": "./inbox", "brain": "cogny"}}}}).to_string()}],
        }),
    );
    let first = out.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    let edges = first["manifest"][0]["diff"]["add_edges"].clone();
    let tapped = edges
        .as_array()
        .expect("the installation's edges")
        .iter()
        .filter(|e| e["from"] == json!("./librarian") && e["lane"] == json!("run_tool_result"))
        .count();
    assert_eq!(
        tapped, 1,
        "the recipe taps the library's tool results for the run: {edges:#}"
    );
    json!({"scope": "/", "diff": {"add_edges": edges}})
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

/// `go` at the app's rim, from a context the app's own chain might carry: a
/// wider `audience_set`, a SMALLER `audience_now` (the generation alone), a
/// speaker, a chat channel. None of it may reach the core or the library.
fn go(text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("go"));
    hop.insert("run_id".into(), json!(RUN));
    let ctx = json!({"audience_set": r#"["*"]"#, "audience_now": r#"["agent:scribe"]"#,
                     "speaker": "member:mallory",
                     "channel_node": "chat", "channel": "talky:981"});
    MessageBuilder::new(Path::new(&format!("/apps/{APP}")))
        .headers(Headers::from_parts(
            ctx.as_object().cloned().unwrap_or_default(),
            hop,
        ))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// One logged delivery with its headers.
#[derive(Clone, Debug)]
struct Row {
    to: String,
    hop: Value,
    ctx: Value,
}

impl Row {
    fn route(&self) -> &str {
        self.hop["route"].as_str().unwrap_or_default()
    }
    fn h(&self, k: &str) -> &str {
        text(&self.hop[k])
    }
    fn c(&self, k: &str) -> String {
        match &self.ctx[k] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }
}

fn rows(root: &std::path::Path) -> Vec<Row> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .map(|(to, h)| {
            let h: Value = serde_json::from_str(&h).unwrap_or(Value::Null);
            Row {
                to,
                hop: h["hop"].clone(),
                ctx: h["context"].clone(),
            }
        })
        .collect()
}

struct Done {
    rows: Vec<Row>,
    dead: Vec<(String, String, String)>,
}

/// Boot, lay the installation, send `go`, wait until the app heard a
/// `run_answer`, read the log.
async fn run(brain: Vec<MockResponse>) -> Done {
    let background =
        MockOpenAI::start(vec![canned_chat_completion(BACKGROUND_REPLY, "stop")]).await;
    let core = MockOpenAI::start(brain).await;
    let stubs = Stubs {
        scripted: HashMap::from([(CORE_BRAIN.to_string(), core.base_url.clone())]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs);
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, mut sink) = mpsc::channel::<Message>(64);
    let (park_tx, _park) = mpsc::channel::<Message>(1024);
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
        .expect("the member road, the library, the graph space and the app must boot");
    let outcome = mutate(&h, installation()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the installation's edges were not committed: {outcome:?}"
    );
    h.send(go("Where is the probe defined?")).await;
    // Waits on the event (the app heard the end), bounded by DEADLINE per
    // message -- no fixed window.
    while let Ok(Some(m)) = tokio::time::timeout(DEADLINE, sink.recv()).await {
        if m.headers.hop.get("heard") == Some(&json!("run_answer")) {
            break;
        }
    }
    h.shutdown().await;
    Done {
        rows: rows(td.path()),
        dead: dead_letters(td.path()),
    }
}

/// One line of the trail a failure prints.
fn say(r: &Row) -> String {
    let (now, set) = (r.c("audience_now"), r.c("audience_set"));
    format!(
        "{} [{}] run_id={} audience_now={now} audience_set={set}",
        r.to,
        r.route(),
        r.c("run_id")
    )
}

fn library_call() -> Vec<MockResponse> {
    vec![
        canned_tool_calls(vec![("call-981-lib", LIB_TOOL, r#"{"name":"probe"}"#)]),
        canned_chat_completion(DONE, "stop"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_reads_the_library_in_the_members_round() {
    if !shipped() {
        return;
    }
    let done = run(library_call()).await;
    let trail: Vec<String> = done.rows.iter().map(say).collect();

    // The run ended at the app (the event the test waited on).
    let inbox = format!("/apps/{APP}/inbox");
    let ended = done
        .rows
        .iter()
        .filter(|r| r.to == inbox && r.route() == "run_answer" && r.h("run_id") == RUN)
        .count();
    assert_eq!(
        ended, 1,
        "the run ended at the app: {trail:#?}\ndead letters: {:#?}",
        done.dead
    );

    // (a) The library answered the run's tool call, heard at the app.
    let from_library: Vec<&Row> = done
        .rows
        .iter()
        .filter(|r| {
            r.to == inbox && r.route() == "run_tool_result" && r.h("answerer") == "librarian"
        })
        .collect();
    assert_eq!(
        from_library.len(),
        1,
        "one tool result of the library reached the app: {trail:#?}"
    );
    // The library writes no `tool_name` on its result hop (measured, lane run
    // 04.10.); the run names the answerer, which is the fact the app keys on.
    assert_eq!(from_library[0].h("run_id"), RUN);

    // (b) Into the graph space: at least the library's question, and every
    // delivery in the member's round, under both keys the graph reads.
    let graph: Vec<&Row> = done
        .rows
        .iter()
        .filter(|r| r.to == GRAPH || r.to.starts_with(&format!("{GRAPH}/")))
        .collect();
    let questions = graph
        .iter()
        .filter(|r| r.route() == "in_graph" && r.h("op_id").starts_with("lib:g:"))
        .count();
    assert!(
        questions >= 1,
        "the library's question reached the graph space: {trail:#?}"
    );
    for r in &graph {
        assert_eq!(
            r.c("run_id"),
            RUN,
            "every delivery into the graph space is the run's: {r:?}\n{trail:#?}"
        );
        assert_eq!(
            r.c("audience_now"),
            ROUND,
            "the graph space reads the round off `audience_now` first -- the app's \
             smaller round reached it through the library: {r:?}"
        );
        assert_eq!(
            r.c("audience_set"),
            ROUND,
            "the graph space's fallback round is the member's too: {r:?}"
        );
    }

    // (c) The core's turn already carries the member's round in `audience_now`
    // (the door's stamp; the graph's seam above is the receiver it protects).
    let turn: Vec<&Row> = done
        .rows
        .iter()
        .filter(|r| r.to.ends_with("/cogny/collector") && r.route() == "in_turn")
        .collect();
    assert_eq!(turn.len(), 1, "one turn reached the core: {trail:#?}");
    assert_eq!(turn[0].c("run_id"), RUN);
    assert_eq!(
        turn[0].c("audience_now"),
        ROUND,
        "the round a resident reads first is the member's at the core, never the app's"
    );
    assert_eq!(turn[0].c("audience_set"), ROUND);
}
