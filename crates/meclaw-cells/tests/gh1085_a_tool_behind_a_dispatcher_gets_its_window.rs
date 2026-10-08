//! GH #1085 (R-IG-1, OR-IG-4) -- a tool behind a dispatcher learns the window
//! of the model that reads its result, on the road a colony really runs.
//!
//! A tool cuts its result to a share (10 %) of the reading model's window and
//! learns that window from `input_soft`: in `context`, or on the hop. The
//! `llm` cell stamps its catalogue row's `input_soft` on every answer, a
//! tool-call bundle included, but a hop lives for one emission -- and in every
//! shipped tool loop a dispatcher stands between the brain and its tools and
//! emits anew. Measured before the fix (review I-1 of strand G2b, review I-3 of
//! strand G3): `dispatcher@1.2.2`, the telegram-research and swarm dispatchers
//! each emitted a header of their own without it, so the stamp ended there and
//! every tool behind them answered whole, up to the 4 MiB carrier ceiling. The
//! locks of strand G2b put the stamp on the tool message by hand and so proved
//! the cell, not the road.
//!
//! Here nothing is put on a message by hand. A real `llm` cell (its provider a
//! mock) with a catalogue row of `input_soft` 10 000 tokens answers with one
//! `web_fetch` call; the shipped dispatcher cell -- and for talky and cogny the
//! shipped sidecar splitter in front of it -- routes the call to a real
//! `web_fetch` cell, which fetches 20 000 bytes from a local server. The result
//! arrives cut to 10 % of the window at three bytes a token, 3 000 bytes, and
//! marked with its total. Without a window on the brain the same road delivers
//! the body whole. The swarm example hands its tools the call as the model
//! wrote it (`{name, arguments}`, its `lookup` and `calc` read that form), so
//! no `web_fetch` can stand behind it; there a probe tool reports the window
//! it was handed.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::{LlmCellFactory, WebFetchCellFactory};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_tool_calls};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The size the local server answers with: well over the window's share.
const BODY_BYTES: usize = 20_000;
/// The brain's catalogue row: 10 000 tokens, so a tool result may take
/// 10 % x 10 000 x 3 = 3 000 bytes of it.
const INPUT_SOFT: u64 = 10_000;
const SHARE_BYTES: usize = 3_000;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped(rel: &str) -> Value {
    let p = repo(rel);
    meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap()
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The brain: a plain `llm` cell on the mock, with or without a catalogue row.
fn brain(base_url: &str, input_soft: Option<u64>) -> Value {
    let mut params = json!({"provider": "openai", "model": "gpt-4o", "api_key": "test-key",
                            "base_url": base_url});
    if let Some(n) = input_soft {
        params["input_soft"] = json!(n);
    }
    json!({
        "cell": {"type": "llm"},
        "params": params,
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

/// The tool: a real `web_fetch` cell, no `max_bytes` (the window decides), the
/// private-network deny opened for the local server (GH #117).
fn fetcher() -> Value {
    json!({
        "cell": {"type": "web_fetch"},
        "params": {"max_concurrency": 2, "external_timeout_ms": 10000,
                   "allow_private_networks": true},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}}},
            "consumes": {"body": {"messages": {"type": "array", "required": true}}}
        },
        "description": {
            "purpose": "Test stand-in for the tool behind a dispatcher.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// One road from the brain to the tool: the cells between them, as shipped,
/// in order (`(name, config path)`), and whether a probe stands in for the
/// tool.
struct Road {
    between: Vec<(&'static str, &'static str)>,
    probe: bool,
}

/// A tool that answers with the window it was handed: `input_soft` of its
/// hop and of its context, as the tool cells read them
/// (`content_budget::input_soft_of`).
const PROBE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
header = (doc.get("envelope") or {}).get("header") or {}
seen = {"hop": (header.get("hop") or {}).get("input_soft"),
        "context": (header.get("context") or {}).get("input_soft")}
sys.stdout.write(json.dumps({"header": {"probe": "1"},
    "messages": [{"origin": "tool", "type": "tool_result", "id": "probe",
                  "text": json.dumps(seen)}]}))
"#;

fn probe() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": PROBE, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}},
                      "hop": {"probe": {"type": "string", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test probe: reports the window a tool behind the road was handed.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

const TOOL_CALLS: &str = "has(hop.finish_reason) && hop.finish_reason == 'tool_calls'";

fn build_tree(td: &tempfile::TempDir, road: &Road, base_url: &str, input_soft: Option<u64>) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/brain/config.json", &brain(base_url, input_soft));
    let (tool, done) = if road.probe {
        (probe(), "has(hop.probe)")
    } else {
        (
            fetcher(),
            "has(hop.operation) && hop.operation == 'web_fetch'",
        )
    };
    write(root, "main/fetch/config.json", &tool);
    let mut edges = Vec::new();
    let mut from = "./brain".to_string();
    for (name, config) in &road.between {
        write(root, &format!("main/{name}/config.json"), &shipped(config));
        // The brain's answer enters the road on its finish reason, as in the
        // shipped composites; a splitter hands the round on unchanged.
        edges.push(json!({"from": from, "to": format!("./{name}"), "condition": TOOL_CALLS}));
        from = format!("./{name}");
    }
    edges.push(json!({"from": from, "to": "./fetch",
                      "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'"}));
    edges.push(json!({"from": "./fetch", "to": "/sink", "condition": done}));
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        ("llm".into(), Arc::new(LlmCellFactory)),
        ("code".into(), Arc::new(CodeCellFactory)),
        ("web_fetch".into(), Arc::new(WebFetchCellFactory)),
    ];
    let h = ColonyHandle::new_with_factories_at(td, factories.clone());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (k, f) in factories {
        registry.insert(k, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// A user turn with the `web_fetch` declaration, so the brain may call it.
fn ask() -> Message {
    let schema = json!({"type": "function", "function": {
        "name": "web_fetch", "description": "fetch one url",
        "parameters": {"type": "object", "properties": {"url": {"type": "string"}},
                       "required": ["url"]}}});
    MessageBuilder::new(Path::new("/brain"))
        .body(Body::Inline(json!({
            "system": {"tools": {"web_fetch": {"text": schema.to_string()}}},
            "messages": [{"origin": "user", "type": "text", "text": "read the page"}]
        })))
        .ttl(64)
        .build()
}

/// Run one road and hand back the tool result as it reached the sink.
async fn result_on(road: &Road, input_soft: Option<u64>) -> Message {
    let page = "y".repeat(BODY_BYTES);
    let (addr, _srv) = start_mock_server(MockResponse::ok(page.as_bytes())).await;
    let args = json!({"url": format!("http://{addr}/page")}).to_string();
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-page",
        "web_fetch",
        args.as_str(),
    )])])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, road, &format!("{}/v1", mock.base_url), input_soft);
    let (h, mut rx) = boot(&td).await;
    h.send(ask()).await;
    let got = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the tool result must reach the sink");
    h.shutdown().await;
    got
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn assert_cut_to_the_window(m: &Message, road: &str) {
    let text = text_of(m);
    let mark = format!("...[cut: {SHARE_BYTES} of {BODY_BYTES} bytes shown; budget of the window]");
    assert!(
        text.ends_with(&mark),
        "{road}: the result is cut to 10 % of the brain's window and marked; it ends in {:?}",
        &text[text.len().saturating_sub(80)..]
    );
    assert_eq!(text.len(), SHARE_BYTES + mark.len(), "{road}");
    assert_eq!(m.headers.hop["truncated"], json!(true), "{road}");
    assert_eq!(m.headers.hop["bytes"], json!(BODY_BYTES), "{road}");
}

/// The shared dispatcher, `dispatcher@1.2.3`, as builder and meclaw-os wire it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_dispatcher_hands_the_brains_window_to_the_tool() {
    let road = Road {
        between: vec![("dispatcher", "templates/dispatcher/config.json")],
        probe: false,
    };
    let m = result_on(&road, Some(INPUT_SOFT)).await;
    assert_cut_to_the_window(&m, "brain -> dispatcher -> web_fetch");
}

/// The road inside talky and cogny: the brain's round passes the sidecar
/// splitter and then the dispatcher before a call leaves for its tool (G3
/// review I-3: a tool behind it never got a budget).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_talky_and_cogny_road_hands_the_brains_window_to_the_tool() {
    for (composite, splitter) in [
        ("talky", "templates/talky/splitter/config.json"),
        ("cogny", "templates/cogny/splitter/config.json"),
    ] {
        let road = Road {
            between: vec![
                ("splitter", splitter),
                ("dispatcher", "templates/dispatcher/config.json"),
            ],
            probe: false,
        };
        let m = result_on(&road, Some(INPUT_SOFT)).await;
        assert_cut_to_the_window(&m, &format!("{composite}: brain -> splitter -> dispatcher"));
    }
}

/// The two example dispatchers a reader copies: telegram-research in front
/// of a real `web_fetch`, swarm in front of the probe (its tools read the
/// call in the model's own `{name, arguments}` form).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_example_dispatchers_hand_the_brains_window_to_the_tool() {
    let road = Road {
        between: vec![(
            "dispatch",
            "examples/telegram-research/main/dispatch/config.json",
        )],
        probe: false,
    };
    let m = result_on(&road, Some(INPUT_SOFT)).await;
    assert_cut_to_the_window(&m, "telegram-research: planner -> dispatch -> reader");

    let road = Road {
        between: vec![("dispatch", "examples/swarm/main/dispatch/config.json")],
        probe: true,
    };
    let m = result_on(&road, Some(INPUT_SOFT)).await;
    let seen: Value = meclaw_core::serde_json::from_str(&text_of(&m)).expect("probe answer");
    assert_eq!(
        seen["hop"],
        json!(INPUT_SOFT),
        "swarm: the tool behind the dispatch is handed the llm's window: {seen}"
    );
}

/// research-assistant's own `dispatch` (fix review G2 I-1): it emits one
/// message per call and hands the planner's window on in each header.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_research_dispatch_hands_the_planners_window_to_the_tool() {
    // GH #49: research-assistant does not travel with the public tree.
    const DISPATCH: &str = "templates/research-assistant/dispatch/config.json";
    if !repo(DISPATCH).is_file() {
        return;
    }
    let road = Road {
        between: vec![("dispatch", DISPATCH)],
        probe: false,
    };
    let m = result_on(&road, Some(INPUT_SOFT)).await;
    assert_cut_to_the_window(&m, "research-assistant: planner -> dispatch -> reader");
}

/// The builder's road to its corpus (fix review G2 I-2): `./compose` ->
/// `dispatcher` -> `./lib`, which emits the search anew on `lsearch_out`, ->
/// `./builder-librarian`. Measured before the fix: the composer's window ended
/// at `./lib` -- no `context.input_soft`, the new header without it -- so the
/// librarian's 10 % never applied. The two edges are the SHIPPED builder's
/// own; a probe stands in for the librarian and reports what it was handed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_builders_librarian_is_handed_the_composers_window() {
    let builder = shipped("templates/builder/config.json");
    let shipped_edge = |from: &str, to: &str| -> Value {
        builder["params"]["graph"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["from"] == from && e["to"] == to)
            .unwrap_or_else(|| panic!("builder edge {from} -> {to}"))
            .clone()
    };
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-lib",
        "librarian_search",
        r#"{"query": "a cell that reads a page"}"#,
    )])])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(
        root,
        "main/brain/config.json",
        &brain(&format!("{}/v1", mock.base_url), Some(INPUT_SOFT)),
    );
    write(
        root,
        "main/dispatcher/config.json",
        &shipped("templates/dispatcher/config.json"),
    );
    write(
        root,
        "main/lib/config.json",
        &shipped("templates/builder/lib/config.json"),
    );
    write(root, "main/builder-librarian/config.json", &probe());
    let edges = vec![
        json!({"from": "./brain", "to": "./dispatcher", "condition": TOOL_CALLS}),
        shipped_edge("./dispatcher", "./lib"),
        shipped_edge("./lib", "./builder-librarian"),
        json!({"from": "./builder-librarian", "to": "/sink", "condition": "has(hop.probe)"}),
    ];
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let (h, mut rx) = boot(&td).await;
    let schema = json!({"type": "function", "function": {
        "name": "librarian_search", "description": "search the corpus",
        "parameters": {"type": "object", "properties": {"query": {"type": "string"}},
                       "required": ["query"]}}});
    h.send(
        MessageBuilder::new(Path::new("/brain"))
            .body(Body::Inline(json!({
                "system": {"tools": {"librarian_search": {"text": schema.to_string()}}},
                "messages": [{"origin": "user", "type": "text", "text": "build me a reader"}]
            })))
            .ttl(64)
            .build(),
    )
    .await;
    let got = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the search must reach the librarian");
    h.shutdown().await;
    let seen: Value = meclaw_core::serde_json::from_str(&text_of(&got)).expect("probe answer");
    assert_eq!(
        seen["context"].as_f64(),
        Some(INPUT_SOFT as f64),
        "the librarian is handed the composer's window in context: {seen}"
    );
}

/// No catalogue row on the brain: no window anywhere on the road, and the
/// tool delivers whole (design § 1) -- the cut above is the window's, not a
/// number of the road's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_window_the_same_road_delivers_whole() {
    let road = Road {
        between: vec![("dispatcher", "templates/dispatcher/config.json")],
        probe: false,
    };
    let m = result_on(&road, None).await;
    assert_eq!(text_of(&m).len(), BODY_BYTES, "whole, unmarked");
    assert!(
        m.headers.hop.get("truncated").is_none(),
        "{:?}",
        m.headers.hop
    );
}

/// OR-IG-8: the exit through which talky's and cogny's brain calls leave the
/// composite also sets `context.input_soft` from the brain's stamp, so a tool
/// behind further cells of the parent still knows the window. Context lives
/// for the whole road; the hop only to the next emission.
#[test]
fn the_composites_tool_exit_sets_the_window_in_context() {
    for composite in ["talky", "cogny"] {
        let cfg = shipped(&format!("templates/{composite}/config.json"));
        let exits: Vec<&Value> = cfg["params"]["graph"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| {
                e["from"] == "./dispatcher"
                    && e["to"] == "."
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.route == 'tool'"))
            })
            .collect();
        assert_eq!(exits.len(), 1, "{composite}: one tool exit");
        assert_eq!(
            exits[0]["modifier"]["set_context"]["input_soft"],
            "has(hop.input_soft) ? hop.input_soft : ''",
            "{composite}: the tool exit carries the brain's window on in context"
        );
    }
}

/// Fix review G2 M-1: the brain's window set in context for the tool road
/// leaves talky and cogny by that road alone. Every other exit deletes it, as
/// every exit deletes `recall_input_soft` (GH #1044): a producer further on
/// would otherwise size its content by the window of a model it does not
/// serve (OR-IG-4). The builder does the same for its composer's window.
#[test]
fn the_brains_window_leaves_by_the_tool_exit_alone() {
    for composite in ["talky", "cogny", "builder"] {
        let cfg = shipped(&format!("templates/{composite}/config.json"));
        let mut exits = 0;
        for e in cfg["params"]["graph"]["edges"].as_array().unwrap() {
            let tool_exit = e["from"] == "./dispatcher"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'tool'"));
            if e["to"] != "." || tool_exit {
                continue;
            }
            exits += 1;
            let cleared = e["modifier"]["delete_context"]
                .as_array()
                .is_some_and(|d| d.iter().any(|k| k == "input_soft"));
            assert!(
                cleared,
                "{composite}: an exit lets the brain's window out: {e}"
            );
        }
        assert!(exits > 0, "{composite}: no exit found");
    }
}
