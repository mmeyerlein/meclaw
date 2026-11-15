//! GH #1528 (d58-3 ballast) -- the call of the judge names its app at the
//! provider.
//!
//! WHY this file exists: every request to a model carries `X-Title` and
//! `HTTP-Referer` (AGENTS.md rule 15, GH #1251), and the judge is a new
//! model call per round. Its llm cell carries both params in the reference
//! form of `memory-hive/closer` (an environment default the instance
//! overrides) and declares its `reasoning_effort`, like every llm cell of
//! the curator.
//!
//! Pinned: `templates/curator/relevance/config.json` is an llm cell with a
//! model, `x_title` and `http_referer` in the reference form and a
//! `reasoning_effort`; on the wire of a stub provider its shipped params send
//! `X-Title: MeClaw` and `HTTP-Referer: https://meclaw.ai`, a set's values
//! replace them, and neither key leaks into the request body. No model is
//! called: the provider is `mock_openai`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use curator_hive::{read_json, repo};
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;

const CELL: &str = "templates/curator/relevance/config.json";
const REFERENCE: &str = "templates/memory-hive/closer/config.json";

/// The shipped params of the judge with every `${…}` resolved to its
/// default, pointed at the stub, without a credential grant, `extra` on top
/// the way a set's `override_params` is.
fn shipped_params(base_url: &str, extra: &Value) -> Value {
    let config = read_json(&repo(CELL));
    let mut params: Map<String, Value> = config["params"].as_object().expect("params").clone();
    for value in params.values_mut() {
        let Some(text) = value.as_str() else { continue };
        let Some(inner) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
            continue;
        };
        *value = match inner.split_once(":-") {
            Some((_, default)) => json!(default),
            None => json!(""),
        };
    }
    params.insert("base_url".into(), json!(base_url));
    params.insert("model".into(), json!("gpt-4o"));
    params.insert("api_key".into(), json!("sk-test"));
    params.insert("credential_grant_id".into(), json!(""));
    for (key, value) in extra.as_object().expect("extra object") {
        params.insert(key.clone(), value.clone());
    }
    Value::Object(params)
}

/// One request of the judge through its cell; the attribution headers and
/// the body the provider saw.
async fn captured(extra: Value) -> (Option<String>, Option<String>, Value) {
    let verdict = json!({"verdict": "asked", "line": "asked the time"}).to_string();
    let mock = MockOpenAI::start(vec![canned_chat_completion(&verdict, "stop")]).await;
    let raw = shipped_params(&format!("{}/v1", mock.base_url), &extra);
    let mut llm = LlmCell::new(
        LlmParams::parse(&raw).unwrap_or_else(|e| panic!("the judge's params parse: {e:?}")),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, _rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/relevance"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let body = json!({"messages": [
        {"origin": "user", "type": "text", "text": "What time is it now?"},
        {"origin": "assistant", "type": "text", "text": "It is ten past eight."}
    ]});
    let msg = MessageBuilder::new(Path::new("/relevance"))
        .reply_to(Path::new("/policy"))
        .body(Body::Inline(body))
        .build();
    llm.handle(msg, &sink, &mut conn).await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let snap = &snaps[0];
    (
        snap.headers.get("http-referer").cloned(),
        snap.headers.get("x-title").cloned(),
        snap.body.clone(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_judge_call_names_its_app() {
    let config = read_json(&repo(CELL));
    assert_eq!(
        config["cell"]["type"],
        json!("llm"),
        "the judge is an llm cell"
    );
    let reference = read_json(&repo(REFERENCE));
    for key in ["x_title", "http_referer"] {
        assert_eq!(
            config["params"][key], reference["params"][key],
            "the judge carries {key} in the reference form"
        );
    }
    for key in ["model", "reasoning_effort"] {
        assert!(
            config["params"][key]
                .as_str()
                .is_some_and(|v| !v.trim().is_empty()),
            "the judge declares its {key}: {}",
            config["params"]
        );
    }

    let (referer, title, body) = captured(json!({})).await;
    assert_eq!(
        referer.as_deref(),
        Some("https://meclaw.ai"),
        "shipped HTTP-Referer"
    );
    assert_eq!(title.as_deref(), Some("MeClaw"), "shipped X-Title");
    for key in ["x_title", "http_referer"] {
        assert!(
            body.get(key).is_none(),
            "{key} leaked into the body: {body}"
        );
    }
    let (referer, title, _) =
        captured(json!({"x_title": "meclaw-lab ballast", "http_referer": "https://example.com"}))
            .await;
    assert_eq!(
        referer.as_deref(),
        Some("https://example.com"),
        "a set's HTTP-Referer"
    );
    assert_eq!(
        title.as_deref(),
        Some("meclaw-lab ballast"),
        "a set's X-Title"
    );
}
