//! GH #963 -- a REAL tool round reaches the presenter: installed by `install_app`, it
//! hears the generation's own tool hive the way production does, and what it keeps
//! carries the round of the result.
//!
//! # Why this file exists
//!
//! The presenter locks of `support/presenter_colony.rs` write the observed call and its
//! result straight onto `presenter/stage`, with `hop.tool_name`, `hop.exit_code` and
//! `context.audience_set` set by the test. That is how GH #937 stayed green while the
//! object form heard nothing real: the doubles said what the real cells never say
//! (review of #963, I-1). Three things only a real round can show, and the presenter
//! rests on each of them:
//!
//! 1. the call id the presenter pairs on is the `id` of the result's `tool_result` turn,
//!    as the real cell writes it (`crates/meclaw-cells/src/tool.rs`, the hop does not
//!    survive the cell);
//! 2. `hop.exit_code` of the real `bash` cell survives the tool hive's exit edge;
//! 3. `context.audience_set` of the call is still on the real result when it arrives --
//!    without it every hit falls (fail-closed, OR-DP.Q.3) and the topic `search` is dead.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant` (holders and brains doubled, as in
//! `gh937_an_app_observes_another_apps_tool.rs`), the SHIPPED tool hive with its REAL
//! `bash` (sandbox `trusted`, the host's kernel is not this lock's subject) and its REAL
//! `web_search` against a loopback fixture, and the SHIPPED presenter installed by the
//! builder's own `install_app` with the observation words of its `template.json` -- the
//! screen, the shows and the residents are other locks' roads and stay out. The decider
//! is held (`start_mock_server_held`), so the turn stays open while the tools run.
//!
//! # What is measured, at the receiver (`presenter/store`)
//!
//! - the `bash` call's `work` row ends `failed` with exit code 3, named "shell command";
//! - the `web_search` call's row ends `done`;
//! - the turn's pending record holds the fixture's hits, each in the round the call
//!   carried -- the round of the result.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{BashCellFactory, LlmCellFactory, WebSearchCellFactory};
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server, start_mock_server_held};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const MEMBER: &str = "/person";
const AGENT: &str = "gen1";
const PRESENTER: &str = "/person/apps/presenter";
/// The round of the conversation, as the member writes it into the context.
const ROUND: &str = r#"["agent:gen1","member:p"]"#;
const MARKER: Duration = Duration::from_secs(30);

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/assistant/config.json",
        "templates/tools/config.json",
        "templates/presenter/template.json",
        "templates/builder/recipes/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn patch(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(&v).expect("serialise"),
    )
    .expect("write");
}

/// Copy every `.json` file of a template, cell by cell (`config.json` and the
/// `template.json` the template scan reads).
fn copy_json(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_json(&from, &dst.join(entry.file_name()));
        } else if from.extension().is_some_and(|e| e == "json") {
            std::fs::copy(&from, dst.join(entry.file_name())).expect("copy");
        }
    }
}

// ══════════════════════════════════════════════════════════════ the doubles

const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

const FIREWALL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "pass"},
    "messages": doc["body"].get("messages", [])}))
"#;

/// The conversation surface of the generation: on `in_wire` it calls the tool the
/// driver named, with the arguments it carried, shaped like a brain's call (one
/// `tool_call` turn); on `in_tool` it reports the call id of the result's turn.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
route = str(hop.get("route") or "")
if route == "in_wire":
    name = str(hop.get("lane") or "")
    cid = "c-" + name
    args = json.loads(str(hop.get("args") or "{}"))
    sys.stdout.write(json.dumps({
        "header": {"route": "tool", "tool_name": name, "tool_call_id": cid},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": cid,
                      "text": json.dumps(args)}],
        "arguments": args}))
elif route == "in_tool":
    got = ""
    for m in doc["body"].get("messages") or []:
        if isinstance(m, dict) and m.get("type") == "tool_result":
            got = str(m.get("id") or "")
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_tool", "got_call_id": got},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "in_wire", "lane": str(hop.get("lane") or ""),
               "args": str(hop.get("args") or "{}")},
    "messages": []}))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

/// The search provider's answer: three hits, one with a link that is no link.
fn fixture() -> Value {
    json!({"results": [
        {"title": "Actors in Rust", "url": "https://docs.example/actors", "snippet": "first"},
        {"title": "Mailboxes", "url": "https://docs.example/mailboxes", "snippet": "second"},
        {"title": "Bad link", "url": "javascript:alert(1)", "snippet": "third"}
    ]})
}

// ═════════════════════════════════════════════════════════════════ the tree

fn assistant_edges() -> Vec<Value> {
    vec![
        json!({"from": "./assistants", "to": format!("./assistants/{AGENT}"),
               "condition": format!("has(hop.route) && hop.route == 'in_tool' && \
                   (!has(context.assistant) || context.assistant == '{AGENT}')")}),
        json!({"from": format!("./assistants/{AGENT}"), "to": "./assistants",
               "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"}),
    ]
}

fn main_config() -> Value {
    let mut edges = vec![json!({
        "from": "./driver", "to": format!("./person/assistants/{AGENT}/talky"),
        "condition": "has(hop.route) && hop.route == 'in_wire'"
    })];
    for lane in [
        "answer",
        "error",
        "reject",
        "write",
        "turn_write",
        "build",
        "in_show",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(root: &std::path::Path, search: &str, decider: &str) {
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver."),
    );
    copy_json(&repo("templates/member"), &root.join("main/person"));
    std::fs::remove_file(root.join("main/person/template.json")).ok();
    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "file-space",
        "graph-space",
        "librarian",
        "objects",
    ] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this round never reaches."),
        );
    }
    write(
        root,
        "main/person/firewall/config.json",
        &double(FIREWALL, "Test double for the member's screen."),
    );

    // The generation: brains doubled, the tool hive SHIPPED.
    let gen_rel = format!("main/person/assistants/{AGENT}");
    let gen_at = root.join(&gen_rel);
    copy_json(&repo("templates/assistant"), &gen_at);
    std::fs::remove_file(gen_at.join("template.json")).ok();
    write(
        root,
        &format!("{gen_rel}/talky/config.json"),
        &double(SURFACE, "The calling surface."),
    );
    for brain in ["talky-chat", "cogny"] {
        write(
            root,
            &format!("{gen_rel}/{brain}/config.json"),
            &double(INERT, "A quiet brain."),
        );
    }
    let tools = gen_at.join("tools");
    std::fs::remove_dir_all(&tools).ok();
    copy_json(&repo("templates/tools"), &tools);
    std::fs::remove_file(tools.join("template.json")).ok();
    patch(&tools.join("bash/config.json"), |v| {
        v["params"]["sandbox"] = json!({"trust": "trusted"});
    });
    patch(&tools.join("web_search/config.json"), |v| {
        v["params"]["endpoint"] = json!(search);
        v["params"]["api_key"] = json!("");
    });
    for occupant in ["web_fetch", "file", "edit"] {
        write(
            root,
            &format!("{gen_rel}/tools/{occupant}/config.json"),
            &double(
                INERT,
                "Inert double for a tool occupant no round here calls.",
            ),
        );
    }

    let cfg = root.join("main/person/config.json");
    patch(&cfg, |v| {
        v["params"]["graph"]["edges"]
            .as_array_mut()
            .expect("the member ships a graph")
            .extend(assistant_edges());
    });

    // The presenter as a template the install grows: decider held, deadline wide.
    let tpl = root.join("templates/presenter");
    copy_json(&repo("templates/presenter"), &tpl);
    patch(&tpl.join("decide/config.json"), |v| {
        v["params"]["model"] = json!("mock-decider");
        v["params"]["api_key"] = json!("fake-key");
        v["params"]["base_url"] = json!(decider);
        v["params"]["external_timeout_ms"] = json!(60_000);
        v["cell"]["message_timeout"] = json!(90_000);
    });
    patch(&tpl.join("stage/config.json"), |v| {
        v["params"]["budget_ms"] = json!(60_000);
    });
    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

/// `install_app` as the builder renders it, over the observation words of the
/// presenter's own `template.json`.
fn rendered_diff() -> Value {
    let block = read_json(&repo("templates/presenter/template.json"))["app"].clone();
    let declaration = json!({
        "observes_tool_calls": block["observes_tool_calls"],
        "observes_tool_results": block["observes_tool_results"],
    });
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": MEMBER, "app": "presenter",
                                                    "template": "presenter@1.1.0",
                                                    "screen": "display-main",
                                                    "generation": AGENT,
                                                    "declaration": declaration}})
                                      .to_string()}],
        }),
    );
    let first = out.first().expect("the recipe emitted nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the presenter's observation words: {first}"
    );
    first["manifest"][0]["diff"].clone()
}

// ═════════════════════════════════════════════════════════════════ the colony

struct Run {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    sink: mpsc::Receiver<Message>,
}

async fn start(search: &str, decider: &str) -> Run {
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(td.path(), search, decider);
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
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (tx, sink) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member, assistant and tool hive boot");

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": MEMBER, "diff": rendered_diff()}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    let outcome = ack_rx.await.expect("the mutation answers");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "installing the presenter with its observation words is one ordinary mutation: \
         {outcome:?}"
    );
    Run { _td: td, h, sink }
}

impl Run {
    async fn log(&self, to_prefix: &str) -> Vec<MessageLogDto> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    to_path_prefix: Some(to_prefix.to_string()),
                    limit: 1000,
                    scan_budget: 50_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(MARKER, ack_rx)
            .await
            .expect("the log answers")
            .expect("ack delivered");
        reply.entries
    }

    /// Every store call `stage` sent to `store`, oldest first.
    async fn store_calls(&self) -> Vec<Value> {
        let mut rows = self.log(&format!("{PRESENTER}/store")).await;
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let mut out = Vec::new();
        for row in rows {
            let body: Value = row
                .body_payload
                .as_deref()
                .and_then(|b| meclaw_core::serde_json::from_str(b).ok())
                .unwrap_or_default();
            for turn in body["messages"].as_array().into_iter().flatten() {
                if turn["type"] == "tool_call"
                    && let Some(text) = turn["text"].as_str()
                    && let Ok(call) = meclaw_core::serde_json::from_str::<Value>(text)
                {
                    out.push(call);
                }
            }
        }
        out
    }

    /// The last `work` row written for `call_id`, once it is no longer `running`.
    async fn settled_row(&self, call_id: &str) -> Value {
        let deadline = Instant::now() + MARKER;
        loop {
            let last = self.store_calls().await.into_iter().rfind(|c| {
                c["operation"] == "insert" && c["table"] == "work" && c["row"]["call_id"] == call_id
            });
            if let Some(c) = last
                && c["row"]["state"] != "running"
            {
                return c["row"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "no settled work row for {call_id} within 30s; DLQ {:?}",
                self.h.drain_dead_letters().await
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The last pending record written for `turn_id`.
    async fn pending(&self, turn_id: &str) -> Value {
        self.store_calls()
            .await
            .into_iter()
            .rfind(|c| {
                c["operation"] == "insert"
                    && c["table"] == "pending"
                    && c["row"]["turn_id"] == turn_id
            })
            .map(|c| c["row"]["value"].clone())
            .unwrap_or_default()
    }

    /// One tool call from the surface, carried in the conversation's round, and the
    /// positive control: the surface gets the result back under the same call id.
    async fn call(&mut self, tool: &str, args: Value) {
        let mut hop = Map::new();
        hop.insert("lane".into(), json!(tool));
        hop.insert("args".into(), json!(args.to_string()));
        let mut ctx = Map::new();
        ctx.insert("audience_set".into(), json!(ROUND));
        ctx.insert("session_id".into(), json!("s1"));
        self.h
            .send(
                MessageBuilder::new(Path::new("/driver"))
                    .hop(hop)
                    .context(ctx)
                    .body(Body::Inline(json!({"messages": []})))
                    .ttl(200)
                    .build(),
            )
            .await;
        let want = format!("c-{tool}");
        let deadline = Instant::now() + MARKER;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = tokio::time::timeout(left, self.sink.recv())
                .await
                .unwrap_or_else(|_| panic!("the {tool} result never came back to the surface"))
                .expect("the sink stands");
            let hop = &m.headers.hop;
            if hop.get("error_code").and_then(Value::as_str) == Some("surface_got_in_tool")
                && hop.get("got_call_id").and_then(Value::as_str) == Some(want.as_str())
            {
                return;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════ the measurement

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_tool_round_reaches_the_presenter_in_the_round_of_its_result() {
    if !shipped() {
        eprintln!("SKIP: the templates did not travel into this tree (GH #49)");
        return;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: no python3");
        return;
    }
    let (search_addr, _search) =
        start_mock_server(MockResponse::ok_json(fixture().to_string().as_bytes())).await;
    let (decider_addr, _decider_join, mut decider) = start_mock_server_held()
        .await
        .expect("the held decider binds on 127.0.0.1");
    let mut run = start(
        &format!("http://{search_addr}/search"),
        &format!("http://{decider_addr}"),
    )
    .await;

    // A turn of the conversation: the presenter asks its decider, which holds -- the
    // turn stays open while the tools run. The held request is the sentinel.
    let mut ctx = Map::new();
    ctx.insert("audience_set".into(), json!(ROUND));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("turn"));
    hop.insert("turn_id".into(), json!("t1"));
    run.h
        .send(
            MessageBuilder::new(Path::new(PRESENTER))
                .hop(hop)
                .context(ctx)
                .body(Body::Inline(json!({"messages": [
                    {"origin": "user", "type": "text", "text": "look up rust actors"}]})))
                .ttl(24)
                .build(),
        )
        .await;
    let held = tokio::time::timeout(MARKER, decider.recv())
        .await
        .expect("the decider is asked within 30s")
        .expect("the decider stands");

    // A real web search, then a real shell command that fails.
    run.call("web_search", json!({"query": "rust actors"}))
        .await;
    run.call("bash", json!({"command": "exit 3"})).await;

    let searched = run.settled_row("c-web_search").await;
    assert_eq!(
        searched["state"], "done",
        "the search call's row is paired with its real result (the id of the result's \
         turn): {searched}"
    );
    let shell = run.settled_row("c-bash").await;
    assert_eq!(
        (&shell["state"], &shell["exit_code"], &shell["summary"]),
        (&json!("failed"), &json!(3), &json!("shell command")),
        "the real bash cell's exit code survives the tool hive's exit edge: {shell}"
    );

    let rec = run.pending("t1").await;
    let hits = rec["observed"]["hits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        hits.iter().map(|h| h["title"].clone()).collect::<Vec<_>>(),
        vec![
            json!("Actors in Rust"),
            json!("Mailboxes"),
            json!("Bad link")
        ],
        "the fixture's hits reach the turn: {rec}"
    );
    let round: Value = meclaw_core::serde_json::from_str(ROUND).expect("the round parses");
    for h in &hits {
        assert_eq!(
            h["audience_set"], round,
            "every hit carries the round the REAL result arrived with: {h}"
        );
    }
    assert_eq!(
        hits[2]["url"], "",
        "a link that is no http(s) link is no link at all (#868): {}",
        hits[2]
    );

    held.release(MockResponse::server_error());
    run.h.shutdown().await;
}
