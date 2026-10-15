//! P10 test helper — fake Responses-API server.
//!
//! Emits SSE bodies in the shape the reference client consumes
//! (`openai/codex` @ `266c6920`, `codex-api/src/sse/responses.rs:330-480`):
//! `response.created` → deltas → `response.output_item.done` →
//! `response.completed`. The deltas are included deliberately, so the parser is
//! tested against a stream that would double the text if it folded them in.

#![allow(dead_code)]

use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Handle on a fake Responses endpoint.
pub struct MockResponses {
    /// `http://127.0.0.1:<port>` — usable as `params.base_url`.
    pub base_url: String,
    /// Captured requests in arrival order.
    pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

impl MockResponses {
    /// Start with a queue of canned responses, one per expected request.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let (addr, join, captured) = start_mock_server_capturing(responses).await;
        Self {
            base_url: format!("http://{addr}"),
            captured,
            join,
        }
    }

    /// Number of requests that reached the endpoint.
    pub async fn call_count(&self) -> usize {
        self.captured.lock().await.len()
    }

    /// Snapshots of every captured request.
    pub async fn recorded(&self) -> Vec<ResponsesRequestSnapshot> {
        self.captured
            .lock()
            .await
            .iter()
            .map(|c| ResponsesRequestSnapshot {
                method: c.method.clone(),
                path: c.path.clone(),
                headers: c.headers.clone(),
                body: serde_json::from_slice(&c.body).unwrap_or(serde_json::Value::Null),
            })
            .collect()
    }
}

/// Parsed view of a captured Responses request.
pub struct ResponsesRequestSnapshot {
    pub method: String,
    pub path: String,
    /// Header names are lowercased by the capture layer.
    pub headers: HashMap<String, String>,
    pub body: serde_json::Value,
}

impl ResponsesRequestSnapshot {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// Serialize events the way the real endpoint does.
///
/// **Each event carries an `event:` line BEFORE its `data:` line.** Verified
/// against `api.openai.com/v1/responses` on 2026-08-09 — the first paid smoke
/// failed precisely because this helper used to emit bare `data:` lines, so the
/// cell's "is this SSE?" sniff never matched a real stream. Keep the `event:`
/// lines: they are what makes this fixture faithful.
fn sse_body(events: &[serde_json::Value]) -> Vec<u8> {
    events
        .iter()
        .map(|e| {
            let name = e.get("type").and_then(|v| v.as_str()).unwrap_or("message");
            format!("event: {name}\ndata: {e}\n\n")
        })
        .collect::<String>()
        .into_bytes()
}

/// A successful streamed text answer, with deltas preceding the canonical
/// `output_item.done`.
pub fn canned_sse_text(text: &str, model: &str) -> MockResponse {
    let body = sse_body(&[
        serde_json::json!({"type":"response.created",
                           "response":{"id":"resp_mock_1","model":model}}),
        serde_json::json!({"type":"response.output_text.delta","delta":text}),
        serde_json::json!({"type":"response.output_item.done","item":{
            "type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}]}}),
        serde_json::json!({"type":"response.completed","response":{
            "id":"resp_mock_1","model":model,
            "usage":{"input_tokens":12,"output_tokens":4}}}),
    ]);
    MockResponse {
        status: 200,
        body,
        content_type: "text/event-stream".into(),
        delay: None,
    }
}

/// HTTP 401 — an expired access token.
pub fn canned_unauthorized() -> MockResponse {
    MockResponse {
        status: 401,
        body: serde_json::json!({"error": {"code": "invalid_token"}})
            .to_string()
            .into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}

/// HTTP 429 with the subscription's quota-exhausted payload.
pub fn canned_quota_exhausted(resets_at: i64, plan: &str) -> MockResponse {
    MockResponse {
        status: 429,
        body: serde_json::json!({"error": {
            "type": "usage_limit_reached", "plan_type": plan, "resets_at": resets_at}})
        .to_string()
        .into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}

/// HTTP 503 with an overload signal.
pub fn canned_overloaded() -> MockResponse {
    MockResponse {
        status: 503,
        body: serde_json::json!({"error": {"code": "server_is_overloaded"}})
            .to_string()
            .into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}
