//! GH #1449 -- an `llm` cell with `package_line` on tells its package, once,
//! on the `package` lane, and the shipped brains hand that line to their
//! curator's `in_catalog` lane without a call.
//!
//! Before, the curator learned `input_soft`, `input_hard`, `chars_per_token`
//! and the rest of a model's package only from the header of an ANSWER (the
//! tap); until the first call it ran on fallback params. What is claimed here:
//!
//! 1. **The cell.** A params-only message addressed to the cell that touches a
//!    package key (set or `$reset`) is answered, once the package is applied,
//!    with exactly one message: header `route: "package"` and `package_of`
//!    (the cell's own path, from the sink the substrate stamps), body
//!    `{"messages": [], "package": {…}}` with the keys of
//!    `meclaw_cells::llm::PACKAGE_LINE_KEYS`. An empty `params` slot is the
//!    question and gets the same answer; a turn never does. Off (the default),
//!    every one of these is the silence of before.
//! 2. **The contract.** Every shipped `llm` cell with `package_line` declares
//!    the lane (`PACKAGE_HOP_KEYS` and the body slot `package`).
//! 3. **The tree.** The shipped `talky`: a push onto its `in_model` door for
//!    `./brain` puts the package line on the curator's `in_catalog` lane --
//!    without any provider call.
//!
//! Free of a paid call by construction: the only provider is the in-process
//! mock, and a push never reaches it.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::llm::{LlmCell, PACKAGE_HOP_KEYS, PACKAGE_LINE_KEYS, PACKAGE_ROUTE};
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the cell

const OWN: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";

fn sink_at(own: &str) -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(own),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn cell(td: &TempDir, base_url: &str, line: bool) -> (LlmCell, DbConn) {
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "start/model", "api_key": "sk-test",
        "base_url": base_url, "package_line": line,
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// One message with `body` and `hop`; every emission it produced.
async fn send(cell: &mut LlmCell, db: &mut DbConn, hop: Value, body: Value) -> Vec<Value> {
    let (sink, mut rx) = sink_at(OWN);
    let msg = MessageBuilder::new(Path::new(OWN))
        .hop(hop.as_object().cloned().unwrap_or_default())
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e.content);
    }
    out
}

fn push_hop() -> Value {
    json!({"route": "in_model", "subscriber": OWN, "rank": "target"})
}

const PACKAGE: &str = r#"{"model": "test/m2", "input_soft": 90000, "input_hard": 200000,
  "max_output": 8192, "cache_mode": "breakpoints", "cache_ttl_s": 300,
  "wire_dialect": "chat_completions", "supported_params": ["temperature"],
  "chars_per_token": 3.5, "cost_in": 10, "cost_cached_in": 1,
  "price_tiers": [{"above_input": 100000, "cost_in": 50, "cost_out": 250}]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_is_answered_with_one_package_line() {
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, "http://127.0.0.1:9/v1", true);
    let package: Value = meclaw_core::serde_json::from_str(PACKAGE).unwrap();
    let out = send(
        &mut c,
        &mut db,
        push_hop(),
        json!({"system": {}, "params": package}),
    )
    .await;
    assert_eq!(out.len(), 1, "one line per push: {out:?}");
    let line = &out[0];
    assert_eq!(line["header"]["route"], PACKAGE_ROUTE, "{line}");
    assert_eq!(
        line["header"]["package_of"], OWN,
        "the line names the cell it came from -- its own path: {line}"
    );
    assert!(line["header"].get("finish_reason").is_none(), "{line}");
    assert_eq!(line["messages"], json!([]), "{line}");
    let p = &line["package"];
    let keys: Vec<&str> = p
        .as_object()
        .expect("package is an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut want: Vec<&str> = PACKAGE_LINE_KEYS.to_vec();
    want.sort_unstable();
    let mut got = keys.clone();
    got.sort_unstable();
    assert_eq!(got, want, "{line}");
    assert_eq!(p["model"], "test/m2");
    assert_eq!(p["input_soft"], 90000);
    assert_eq!(p["input_hard"], 200000);
    assert_eq!(p["max_output"], 8192);
    assert_eq!(p["cache_mode"], "breakpoints");
    assert_eq!(p["cache_ttl_s"], 300);
    assert_eq!(p["wire_dialect"], "chat_completions");
    assert_eq!(p["supported_params"], json!(["temperature"]));
    assert_eq!(p["chars_per_token"], 3.5);
    // The ladder the tap stamps: the base step, then the listed steps.
    assert_eq!(
        p["price_tiers"],
        json!([{"above_input": 0, "cost_in": 10.0, "cost_cached_in": 1.0},
               {"above_input": 100000, "cost_in": 50.0, "cost_out": 250.0}]),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_and_the_empty_question_are_answered_too_a_turn_never() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi back", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, &format!("{}/v1", mock.base_url), true);
    // A `$reset` is a package change like any other: the line says what is left.
    let out = send(
        &mut c,
        &mut db,
        push_hop(),
        json!({"system": {}, "params": {"$reset": ["input_soft", "cache_mode"]}}),
    )
    .await;
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["package"]["model"], "start/model");
    assert_eq!(
        out[0]["package"]["input_soft"],
        Value::Null,
        "unstated is null"
    );
    assert_eq!(out[0]["package"]["cache_mode"], "off");
    // The question: an empty params slot, one round trip.
    let out = send(
        &mut c,
        &mut db,
        json!({}),
        json!({"system": {}, "params": {}}),
    )
    .await;
    assert_eq!(out.len(), 1, "the question is answered: {out:?}");
    assert_eq!(out[0]["header"]["route"], PACKAGE_ROUTE);
    // A params-only message without a package key moves no package: silence.
    let out = send(
        &mut c,
        &mut db,
        json!({}),
        json!({"system": {}, "params": {"x_title": "meclaw-lab test"}}),
    )
    .await;
    assert!(out.is_empty(), "no package key, no line: {out:?}");
    assert!(
        mock.recorded_requests().await.is_empty(),
        "no line costs a provider call"
    );
    // A turn -- `messages` present, even with an empty params slot -- is a
    // conversation, not a push or the question: its answer comes, no line.
    let out = send(
        &mut c,
        &mut db,
        json!({}),
        json!({"system": {}, "params": {},
               "messages": [{"origin": "user", "type": "text", "text": "Hi"}]}),
    )
    .await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        1,
        "the turn reached the model: {out:?}"
    );
    assert!(!out.is_empty(), "the turn is answered: {out:?}");
    assert!(
        out.iter().all(|e| e["header"]["route"] != PACKAGE_ROUTE),
        "a turn never gets the package line: {out:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_by_default_a_push_stays_silent() {
    let td = TempDir::new().unwrap();
    let (mut c, mut db) = cell(&td, "http://127.0.0.1:9/v1", false);
    let package: Value = meclaw_core::serde_json::from_str(PACKAGE).unwrap();
    let out = send(
        &mut c,
        &mut db,
        push_hop(),
        json!({"system": {}, "params": package}),
    )
    .await;
    assert!(out.is_empty(), "off: the silence of before: {out:?}");
    let out = send(
        &mut c,
        &mut db,
        json!({}),
        json!({"system": {}, "params": {}}),
    )
    .await;
    assert!(out.is_empty(), "off: no answer to the question: {out:?}");
}

// ───────────────────────────────────────────────────────────── the contract

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every way `config` falls short of declaring the package lane.
fn complaints(config: &Value) -> Vec<String> {
    if config.pointer("/params/package_line") != Some(&json!(true)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let hop = config.pointer("/contract/emits/hop");
    for (key, ty) in PACKAGE_HOP_KEYS {
        let Some(d) = hop.and_then(|h| h.get(*key)) else {
            out.push(format!("`contract.emits.hop` does not declare `{key}`"));
            continue;
        };
        let typed = d.get("type") == Some(&json!(ty)) && d.get("required") == Some(&json!(false));
        let listed = match d.get("values") {
            None => true,
            Some(v) => {
                *key != "route"
                    || v.as_array()
                        .is_some_and(|a| a.contains(&json!(PACKAGE_ROUTE)))
            }
        };
        if !typed || !listed {
            out.push(format!("`{key}` is {d}"));
        }
    }
    if config.pointer("/contract/emits/body/package")
        != Some(&json!({"type": "object", "required": false}))
    {
        out.push("`contract.emits.body` does not declare `package`".into());
    }
    out
}

#[test]
fn every_llm_cell_with_the_package_line_declares_the_lane() {
    let mut findings = Vec::new();
    let mut on = Vec::new();
    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("config.json") {
                continue;
            }
            let Ok(config) = std::fs::read_to_string(&path)
                .map_err(|_| ())
                .and_then(|r| meclaw_core::serde_json::from_str::<Value>(&r).map_err(|_| ()))
            else {
                continue;
            };
            if config.pointer("/cell/type").and_then(Value::as_str) != Some("llm") {
                continue;
            }
            if config.pointer("/params/package_line") == Some(&json!(true)) {
                on.push(path.display().to_string());
            }
            for c in complaints(&config) {
                findings.push(format!("{}: {c}", path.display()));
            }
        }
    }
    assert!(findings.is_empty(), "{}", findings.join("\n  "));
    // The shipped brains the curator serves tell their package.
    for brain in [
        "templates/talky/brain/config.json",
        "templates/talky/quick/config.json",
        "templates/talky/quick_spec/config.json",
        "templates/talky/quick_bridge/config.json",
        "templates/cogny/brain/config.json",
    ] {
        if repo(brain).exists() {
            assert!(
                on.iter()
                    .any(|p| p.ends_with(brain.trim_start_matches("templates"))),
                "{brain} ships without `package_line`: {on:?}"
            );
        }
    }
}

#[test]
fn the_verdict_asks_only_a_cell_with_the_line() {
    let full = json!({
        "cell": {"type": "llm"}, "params": {"package_line": true},
        "contract": {"emits": {
            "body": {"package": {"type": "object", "required": false}},
            "hop": {"route": {"type": "string", "values": ["partial", "package"], "required": false},
                    "package_of": {"type": "string", "required": false}}}}});
    assert!(complaints(&full).is_empty());
    let mut off = full.clone();
    off["params"]["package_line"] = json!(false);
    off["contract"] = json!({});
    assert!(complaints(&off).is_empty());
    let mut closed = full.clone();
    closed["contract"]["emits"]["hop"]["route"]["values"] = json!(["partial"]);
    assert_eq!(complaints(&closed).len(), 1);
    let mut bare = full;
    bare["contract"] = json!({});
    assert_eq!(complaints(&bare).len(), PACKAGE_HOP_KEYS.len() + 1);
}

// ───────────────────────────────────────────────────────────── the tree

fn templates_root() -> std::path::PathBuf {
    repo("templates")
}

fn shipped() -> bool {
    templates_root().join("talky/config.json").exists()
}

fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let name = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template")
            .split('@')
            .next()
            .unwrap_or_default()
            .to_string();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The curator's policy, replaced in the test tree by a stand-in that turns
/// whatever reaches it on `in_catalog` into one `caught` line (the policy's
/// own handling of the lane is not this file's claim).
const STAND_IN: &str = r#"
import sys, json
d = json.load(sys.stdin)
hop = ((d.get("envelope") or {}).get("header") or {}).get("hop") or {}
if hop.get("route") == "in_catalog":
    b = d.get("body") or {}
    sys.stdout.write(json.dumps({
        "header": {"route": "caught", "package_of": str(hop.get("package_of") or "")},
        "messages": [{"origin": "assistant", "type": "text",
                      "text": json.dumps(b.get("package"))}]}))
"#;

fn stand_in() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": STAND_IN, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}},
                      "hop": {"route": {"type": "string", "values": ["caught"], "required": false},
                              "package_of": {"type": "string", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false},
                                  "package": {"type": "object", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": "Test stand-in for the curator policy.",
                        "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
    })
}

const START: &str = "start/model";

fn build_tree(td: &TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./talky/curator/policy", "to": "/catalog",
             "condition": "has(hop.route) && hop.route == 'caught'"}
        ]}}}),
    );
    copy_cells(&templates_root().join("talky"), &root.join("main/talky"));
    write(root, "main/talky/curator/policy/config.json", &stand_in());
    patch(root, "main/talky/session-keeper/close/config.json", |v| {
        v["params"]["night_from_utc"] = json!("00:00");
        v["params"]["night_until_utc"] = json!("00:00");
    });
    for cell in [
        "brain",
        "quick",
        "quick_spec",
        "quick_bridge",
        "curator/summarizer",
    ] {
        patch(root, &format!("main/talky/{cell}/config.json"), |v| {
            v["params"]["base_url"] = json!(base_url);
            v["params"]["model"] = json!(START);
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_reaches_the_curators_catalog_lane_before_any_call() {
    if !shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    build_tree(&td, &format!("{}/v1", mock.base_url));
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
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (tx, mut rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/catalog"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    let package: Value = meclaw_core::serde_json::from_str(PACKAGE).unwrap();
    h.send(
        MessageBuilder::new(Path::new("/talky"))
            .hop(
                json!({"route": "in_model", "subscriber": "/talky/brain", "rank": "target"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            )
            .body(Body::Inline(json!({"system": {}, "params": package})))
            .ttl(200)
            .build(),
    )
    .await;
    let caught = tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the package line never reached the curator's in_catalog lane");
    assert_eq!(
        caught.headers.hop.get("package_of"),
        Some(&json!("/talky/brain")),
        "{:?}",
        caught.headers.hop
    );
    let Body::Inline(b) = &caught.body else {
        panic!("inline expected")
    };
    let p: Value =
        meclaw_core::serde_json::from_str(b["messages"][0]["text"].as_str().unwrap_or("null"))
            .unwrap();
    assert_eq!(p["model"], "test/m2", "{p}");
    assert_eq!(p["input_soft"], 90000, "{p}");
    assert!(
        mock.recorded_requests().await.is_empty(),
        "the line reaches the curator without a call -- none was made"
    );
    h.shutdown().await;
}
