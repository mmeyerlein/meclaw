//! Shared harness of the GH #999 locks: one turn of a real `llm` cell in a
//! colony `/brain -> /sink`, against the in-process mock provider.
//!
//! Every claim of these locks is measured AT THE RECEIVER (the sink), not at
//! the emitter: a hop key or an error kind that never arrives downstream is
//! no claim at all. Pulled in by `#[path]`, so this directory is never a test
//! binary of its own (an empty binary is a red station, GH #997).

#![allow(dead_code)]

use crate::mock_openai::MockOpenAI;
use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Failure marker, generous per the 30 s convention -- not a timing claim.
pub const MARKER: Duration = Duration::from_secs(30);

/// What one turn left behind: every request the provider got, the body of
/// the answer and its hop, both as the sink received them.
pub struct Turn {
    pub requests: Vec<Value>,
    pub body: Value,
    pub hop: Map<String, Value>,
}

impl Turn {
    /// `meta.error.kind` of an error answer, `None` when absent.
    pub fn kind(&self) -> Option<&str> {
        self.body["meta"]["error"]["kind"].as_str()
    }
}

/// The base params of these locks: a generic model with a birth temperature
/// and no list, the shape most shipped cells have.
pub fn base_params(base_url: &str) -> Value {
    json!({
        "provider": "openai", "model": "model-a", "api_key": "test-key",
        "base_url": base_url, "temperature": 0.3
    })
}

/// `base` with every key of `extra` laid over it.
pub fn overlay(mut base: Value, extra: &Value) -> Value {
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        base[k] = v;
    }
    base
}

/// A chat-completions answer around one `message` object.
pub fn completion(message: Value, finish_reason: &str) -> MockResponse {
    let body = json!({
        "id": "chatcmpl-h5", "object": "chat.completion", "model": "model-a",
        "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

/// Any status with a JSON body.
pub fn status_json(status: u16, body: Value) -> MockResponse {
    MockResponse {
        status,
        body: body.to_string().into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}

/// The input of a plain turn.
pub fn hi() -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]})
}

/// One turn with the base params plus `extra`.
pub async fn one_turn(response: MockResponse, extra: Value) -> Turn {
    turn_with(
        response,
        |base_url| overlay(base_params(base_url), &extra),
        hi(),
    )
    .await
}

/// One turn of a cell whose params `params_for(<mock base_url>)` builds, fed
/// `input` as its body.
pub async fn turn_with(
    response: MockResponse,
    params_for: impl FnOnce(&str) -> Value,
    input: Value,
) -> Turn {
    let mock = MockOpenAI::start(vec![response]).await;
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main/brain")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        meclaw_core::serde_json::to_string_pretty(&json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [{"from": "./brain", "to": "/sink"}]}}
        }))
        .unwrap(),
    )
    .unwrap();
    let params = params_for(&format!("{}/v1", mock.base_url));
    std::fs::write(
        td.path().join("main/brain/config.json"),
        meclaw_core::serde_json::to_string_pretty(&json!({
            "cell": {"type": "llm"},
            "params": params,
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        }))
        .unwrap(),
    )
    .unwrap();

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(4);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap");

    h.send(
        MessageBuilder::new(Path::new("/brain"))
            .body(Body::Inline(input))
            .build(),
    )
    .await;
    let answer = tokio::time::timeout(MARKER, sink_rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the answer reaches the sink");
    let requests = mock
        .recorded_requests()
        .await
        .into_iter()
        .map(|r| r.body)
        .collect();
    h.shutdown().await;
    let body = match &answer.body {
        Body::Inline(v) => v.clone(),
        other => panic!("an inline answer, got {other:?}"),
    };
    Turn {
        requests,
        body,
        hop: answer.headers.hop.clone(),
    }
}
