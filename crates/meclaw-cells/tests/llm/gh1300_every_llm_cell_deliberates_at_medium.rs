//! GH #1300 — every shipped `llm` cell deliberates at `medium` unless its
//! instance says otherwise.
//!
//! The owner's rule (all Anthropic models over the Anthropic API, all at
//! reasoning effort medium) did not reach the cells that left
//! `reasoning_effort` undeclared: the mutation door admits an `override_params`
//! key only when the cell's `config.json` carries it under `params`, so an
//! instance could not set it, and the provider's own default ran instead --
//! `high` on Sonnet 5.5 (cogny's summarizer, the curator's summarizer,
//! builder's composer).
//!
//! Three locks:
//!
//! 1. every `llm` cell under `templates/` (except the `decisions` lane, which
//!    refuses a reasoning field by name) ships `params.reasoning_effort:
//!    "medium"` and declares it as a string setting with that default; no
//!    provider overlay and no `params.reasoning` object carries a reasoning
//!    past it (an object outranks the shorthand) -- the one exception is
//!    talky's `quick`, reasoning off by ruling R-EF-2, pinned to exactly
//!    `{"enabled": false}`;
//! 2. a Sonnet summarizer, born from the shipped `curator/summarizer` params
//!    and moved to the native Anthropic lane, sends `output_config.effort:
//!    "medium"` -- against a strict mock that refuses any body field the
//!    Messages API does not take, so no chat-wire `reasoning` object and no
//!    `thinking: disabled` can pass unseen.
//! 3. every model package of the shipped registry seed that names a
//!    reasoning names `medium` (OR-EF-18): a push resets a cell to the
//!    package, so a package at another effort would override every cell.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use tempfile::TempDir;

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
use mock_openai::MockOpenAI;

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `llm` cell `config.json` under `templates/`, as `(relative dir, config)`.
fn shipped_llm_cells() -> Vec<(String, Value)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Value)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.file_name().is_some_and(|n| n == "config.json")
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok(cfg) = meclaw_core::serde_json::from_str::<Value>(&text)
                && cfg["cell"]["type"] == "llm"
            {
                let rel = p
                    .parent()
                    .and_then(|d| d.strip_prefix(root).ok())
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                out.push((rel, cfg));
            }
        }
    }
    let root = core_root().join("templates");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

#[test]
fn every_llm_cell_declares_reasoning_effort_and_ships_medium() {
    // The floor is the tree's own: the private tree ships 24 llm cells, the
    // export tree 18 (0.70.6 release gate: a fixed `>= 20` was green here and
    // red in the export tree, the class of GH #1101). Every cell the walk
    // sees is judged; the tree must hold at least one.
    let cells = shipped_llm_cells();
    assert!(
        !cells.is_empty(),
        "the walk must see the llm cells this tree ships, saw none"
    );
    let mut checked = 0;
    let mut decisions = 0;
    let mut missing = Vec::new();
    for (rel, cfg) in &cells {
        if cfg["params"]["provider"] == "decisions" {
            decisions += 1;
            assert!(
                cfg["params"].get("reasoning_effort").is_none(),
                "{rel}: the decisions lane refuses reasoning_effort by name"
            );
            continue;
        }
        checked += 1;
        let params = &cfg["params"];
        let setting = &cfg["contract"]["settings"]["reasoning_effort"];
        let ok = params["reasoning_effort"] == "medium"
            && setting["type"] == "string"
            && setting["secret"] == false
            && setting["default"] == "medium"
            // review nit: Opus 5.5 and its kin cannot switch thinking off, so
            // the setting says what `none` is there.
            && setting["description"].as_str().is_some_and(|d| {
                d.contains("`none` = off, or the lowest effort a model that cannot switch thinking off takes")
            });
        if !ok {
            missing.push(format!(
                "{rel}: params.reasoning_effort={} settings.reasoning_effort={}",
                params["reasoning_effort"], setting
            ));
        }
        assert!(
            params["provider_extra"].get("reasoning").is_none(),
            "{rel}: a provider_extra.reasoning overlay is sent verbatim and outranks the \
             declared effort (the Messages wire refuses it)"
        );
        if rel == "talky/quick" {
            assert_eq!(
                params["reasoning"],
                json!({"enabled": false}),
                "{rel}: the quick voice runs with reasoning off (R-EF-2), nothing else"
            );
        } else {
            assert!(
                params.get("reasoning").is_none(),
                "{rel}: a params.reasoning object outranks the declared effort: {}",
                params["reasoning"]
            );
        }
    }
    assert!(
        missing.is_empty(),
        "every llm cell declares reasoning_effort, default medium:\n{}",
        missing.join("\n")
    );
    assert!(
        checked >= 1 && checked + decisions == cells.len(),
        "checked {checked} of {} cells ({decisions} on the decisions lane)",
        cells.len()
    );
}

/// OR-EF-18: the registry's model packages are the effort a push sets on a
/// cell, so a package that names a reasoning names `medium` -- as the object
/// `{"effort": "medium"}` where the endpoint refuses to switch reasoning off
/// (the object outranks a cell's off, the shorthand would not), or as the
/// shorthand.
#[test]
fn every_registry_package_that_names_a_reasoning_names_medium() {
    let seed = core_root().join("templates/llm-registry/store/seed/models.jsonl");
    let text = std::fs::read_to_string(&seed).expect("the registry seed");
    let mut named = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).expect("a seed row");
        let Some(package) = row.get("package") else {
            continue;
        };
        let id = &row["model_id"];
        if let Some(effort) = package.get("reasoning_effort") {
            named += 1;
            assert_eq!(effort, "medium", "{id}: package.reasoning_effort");
        }
        if let Some(reasoning) = package.get("reasoning") {
            named += 1;
            assert_eq!(
                reasoning,
                &json!({"effort": "medium"}),
                "{id}: package.reasoning outranks every cell's effort"
            );
        }
        assert!(
            package["provider_extra"].get("reasoning").is_none(),
            "{id}: a provider_extra.reasoning in a package outranks every cell's effort"
        );
    }
    assert!(
        named >= 2,
        "the walk must see the packages that name a reasoning"
    );
}

/// The shipped `curator/summarizer` params with every `${…}` resolved, moved
/// to the native Anthropic lane on Sonnet 5.5 at the mock.
fn sonnet_summarizer(base_url: &str) -> Value {
    let path = core_root().join("templates/curator/summarizer/config.json");
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&path).expect("read"))
            .expect("parse");
    let mut params: Map<String, Value> = cfg["params"].as_object().expect("params").clone();
    for (key, value) in params.iter_mut() {
        let Some(text) = value.as_str() else { continue };
        let Some(inner) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
            continue;
        };
        *value = match inner.split_once(":-") {
            Some((_, default)) => json!(default),
            None if key == "model" => json!("claude-sonnet-5-5"),
            None => panic!("no test value for the placeholder of '{key}': {text}"),
        };
    }
    params.insert("provider".into(), json!("anthropic"));
    params.insert("model".into(), json!("claude-sonnet-5-5"));
    params.insert("api_key".into(), json!("test-key-1300"));
    params.insert("base_url".into(), json!(base_url));
    params.insert("max_tokens".into(), json!(256));
    Value::Object(params)
}

/// The top-level fields a Messages request may carry; a strict provider
/// answers anything else with a 400.
const MESSAGES_FIELDS: &[&str] = &[
    "model",
    "max_tokens",
    "system",
    "messages",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "temperature",
    "metadata",
    "stream",
    "stop_sequences",
    "cache_control",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sonnet_summarizer_runs_at_medium_on_the_messages_wire() {
    let answer = json!({
        "id": "msg_1300", "type": "message", "role": "assistant", "model": "claude-sonnet-5-5",
        "content": [{"type": "text", "text": "summary"}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let mock = MockOpenAI::start(vec![MockResponse::ok_json(answer.to_string().as_bytes())]).await;
    let raw = sonnet_summarizer(&mock.base_url);
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).expect("the summarizer's shipped params parse"),
        reqwest::Client::new(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/summarizer"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/summarizer"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": "Summarise."}]}),
        ))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the summarizer must emit an answer");

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    let body = &reqs[0].body;
    assert_eq!(reqs[0].path, "/messages", "the native Messages lane");
    for key in body.as_object().expect("body object").keys() {
        assert!(
            MESSAGES_FIELDS.contains(&key.as_str()),
            "a strict provider refuses the field '{key}': {body}"
        );
    }
    assert_eq!(
        body["output_config"],
        json!({"effort": "medium"}),
        "the summarizer deliberates at medium, not at Sonnet 5.5's own default: {body}"
    );
    assert!(
        body.get("thinking").is_none(),
        "no thinking override beside the effort (adaptive is the default): {body}"
    );
}
