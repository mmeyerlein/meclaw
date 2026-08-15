//! Phase-8 test helper — OpenAI Chat-Completions JSON wrapper around
//! `meclaw_testing::mock_http::start_mock_server_capturing`.
//!
//! T10: canned-response helpers (text-completion, tool-calls, error-status)
//! + `OpenAiRequestSnapshot` parser for captured request bodies.
//!
//! Used by T11 (call_openai unit tests), T20/T21 (LlmCell integration), T27-T31
//! (phase_8_demo). Future integration tests declare:
//! ```ignore
//!     #[path = "mock_openai.rs"]
//!     mod mock_openai;
//!     use mock_openai::{MockOpenAI, canned_chat_completion};
//! ```
//!
//! Layering: provider-specific knowledge (OpenAI Chat-Completions JSON shape)
//! lives here in `meclaw-cells/tests/`; the generic TCP listener +
//! request-capture mechanics live in `meclaw-testing/src/mock_http.rs`.

#![allow(dead_code)]

use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// MockOpenAI server handle. Lives for the duration of the test; the background
/// listener task is held via the `JoinHandle` field and aborts on drop only
/// when the tokio runtime tears down.
pub struct MockOpenAI {
    /// The bound socket address (`127.0.0.1:<port>`).
    pub addr: std::net::SocketAddr,
    /// Convenience: `format!("http://{addr}")` — usable as `LlmParams::base_url`.
    pub base_url: String,
    /// Captured requests in arrival order. Lock + iterate via `recorded_requests()`.
    pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

impl MockOpenAI {
    /// Start a mock OpenAI server with a sequence of canned responses (one per
    /// expected incoming request, in order).
    ///
    /// Wire-contract sharpening (Run-4b offline proof, receipt 922d93f): every
    /// request is validated against the provider's tool-call sequencing rule —
    /// an assistant message with `tool_calls` must be immediately followed by
    /// `tool` messages answering each `tool_call_id` (Form A → 400, like the
    /// real provider; Form B/C → canned response). A rejection does NOT
    /// consume the canned sequence.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let validator: RequestValidator = Arc::new(|req: &CapturedRequest| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).ok()?;
            let violation = tool_call_sequence_violation(&body)?;
            Some(MockResponse {
                status: 400,
                body: serde_json::json!({"error": {"message": format!(
                    "An assistant message with 'tool_calls' must be followed by \
                     tool messages responding to each 'tool_call_id'. {violation}"
                )}})
                .to_string()
                .into_bytes(),
                content_type: "application/json".into(),
                delay: None,
            })
        });
        let (addr, join, captured) =
            start_mock_server_capturing_with_validator(responses, Some(validator)).await;
        let base_url = format!("http://{}", addr);
        Self {
            addr,
            base_url,
            captured,
            join,
        }
    }

    /// Snapshot all captured Chat-Completions request bodies, parsed as JSON.
    /// Panics if a captured body is not valid JSON (test contract — the
    /// caller must only point real OpenAI-shaped traffic at this mock).
    pub async fn recorded_requests(&self) -> Vec<OpenAiRequestSnapshot> {
        let caps = self.captured.lock().await;
        caps.iter()
            .map(|c| {
                let body: serde_json::Value =
                    serde_json::from_slice(&c.body).expect("captured body is JSON");
                OpenAiRequestSnapshot {
                    method: c.method.clone(),
                    path: c.path.clone(),
                    headers: c.headers.clone(),
                    body,
                }
            })
            .collect()
    }
}

/// Parsed view of a captured OpenAI Chat-Completions request.
pub struct OpenAiRequestSnapshot {
    /// HTTP method (expected: `"POST"`).
    pub method: String,
    /// Request path (expected: `"/v1/chat/completions"` or similar).
    pub path: String,
    /// Request headers, names lowercased (e.g. `"authorization"`, `"content-type"`).
    pub headers: HashMap<String, String>,
    /// Parsed JSON body — the OpenAI Chat-Completions request payload.
    pub body: serde_json::Value,
}

impl OpenAiRequestSnapshot {
    /// `body.model` as `&str`, if present.
    pub fn model(&self) -> Option<&str> {
        self.body.get("model").and_then(|v| v.as_str())
    }
    /// `body.messages` as `&Vec<Value>`, if present.
    pub fn messages(&self) -> Option<&Vec<serde_json::Value>> {
        self.body.get("messages").and_then(|v| v.as_array())
    }
    /// `body.temperature` as `f64`, if present.
    pub fn temperature(&self) -> Option<f64> {
        self.body.get("temperature").and_then(|v| v.as_f64())
    }
    /// `body.tools` as `&Vec<Value>`, if present.
    pub fn tools(&self) -> Option<&Vec<serde_json::Value>> {
        self.body.get("tools").and_then(|v| v.as_array())
    }
}

/// OpenAI wire-contract check: for each assistant message with a non-empty
/// `tool_calls[]`, the immediately following consecutive `role: "tool"`
/// messages must answer every `tool_call_id`. Returns a violation description,
/// or `None` if the message sequence is wire-valid.
///
/// This is exactly the rule the real provider enforces (Run-4b: Form A → 400,
/// Form B/C → 200). Non-chat bodies (no `messages[]`) pass.
fn tool_call_sequence_violation(body: &serde_json::Value) -> Option<String> {
    let msgs = body.get("messages")?.as_array()?;
    for (i, m) in msgs.iter().enumerate() {
        if m.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(calls) = m.get("tool_calls").and_then(|v| v.as_array()) else {
            continue;
        };
        let mut pending: std::collections::HashSet<&str> = calls
            .iter()
            .filter_map(|c| c.get("id").and_then(|v| v.as_str()))
            .collect();
        for follower in &msgs[i + 1..] {
            if pending.is_empty() || follower.get("role").and_then(|v| v.as_str()) != Some("tool") {
                break;
            }
            if let Some(id) = follower.get("tool_call_id").and_then(|v| v.as_str()) {
                pending.remove(id);
            }
        }
        if !pending.is_empty() {
            let mut ids: Vec<&str> = pending.into_iter().collect();
            ids.sort_unstable();
            return Some(format!("messages[{i}]: unanswered tool_call_id(s) {ids:?}"));
        }
    }
    None
}

// ───── Canned-response builders ─────

/// Build a canned OpenAI Chat-Completions success response with the given
/// assistant text `content` and `finish_reason` (`"stop"` / `"length"` / etc.).
pub fn canned_chat_completion(content: &str, finish_reason: &str) -> MockResponse {
    let json = serde_json::json!({
        "id": "chatcmpl-test-001",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {"role": "assistant", "content": content},
            "finish_reason": finish_reason
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned OpenAI Chat-Completions tool-calls response.
/// `tool_calls` is a Vec of `(id, function_name, arguments_json)` tuples.
pub fn canned_tool_calls(tool_calls: Vec<(&str, &str, &str)>) -> MockResponse {
    let calls: Vec<serde_json::Value> = tool_calls
        .into_iter()
        .map(|(id, name, args)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args}
            })
        })
        .collect();
    let json = serde_json::json!({
        "id": "chatcmpl-test-tc-001",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": calls
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned response that carries `content` **and** `tool_calls` in the
/// SAME choice -- the shape a model produces when it says "one moment" and asks
/// for a tool in one breath (GH #28). The `llm` cell maps it to one `tool_call`
/// turn per call, followed by the text turn.
pub fn canned_content_and_tool_calls(
    content: &str,
    tool_calls: Vec<(&str, &str, &str)>,
) -> MockResponse {
    let calls: Vec<serde_json::Value> = tool_calls
        .into_iter()
        .map(|(id, name, args)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args}
            })
        })
        .collect();
    let json = serde_json::json!({
        "id": "chatcmpl-test-tc-002",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {"role": "assistant", "content": content, "tool_calls": calls},
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned error response (4xx/5xx HTTP status with a minimal OpenAI-shaped
/// error body). `MockResponse` is constructed via direct struct literal — T9 left
/// the fields public, so this is straightforward.
pub fn canned_error_status(status: u16) -> MockResponse {
    MockResponse {
        status,
        body: format!(r#"{{"error":{{"message":"mock {} error"}}}}"#, status).into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}

// ───── Self-tests ─────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream as StdTcpStream};

    /// Mirror of `meclaw_testing::mock_http::tests::raw_post_sync` — send a
    /// raw HTTP/1.1 POST with body, return the full response bytes. Stays
    /// dep-free (no reqwest in the test path).
    fn raw_post_sync(
        addr: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Vec<u8> {
        let mut stream = StdTcpStream::connect(addr).unwrap();
        let mut req = format!("POST {path} HTTP/1.1\r\nHost: x\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        buf
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_openai_start_returns_base_url() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        assert!(mock.base_url.starts_with("http://"));
        assert!(mock.base_url.contains(&mock.addr.to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recorded_requests_parses_body_as_json() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let addr = mock.addr;
        let req_body =
            r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"temperature":0.7}"#;
        let _resp = tokio::task::spawn_blocking(move || {
            raw_post_sync(
                addr,
                "/v1/chat/completions",
                &[
                    ("authorization", "Bearer test-key"),
                    ("content-type", "application/json"),
                ],
                req_body.as_bytes(),
            )
        })
        .await
        .unwrap();

        let snaps = mock.recorded_requests().await;
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].method, "POST");
        assert_eq!(snaps[0].path, "/v1/chat/completions");
        assert_eq!(snaps[0].model(), Some("gpt-4o"));
        assert_eq!(snaps[0].temperature(), Some(0.7));
        let msgs = snaps[0].messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
        // Headers are lowercased by the capturing listener.
        assert_eq!(
            snaps[0].headers.get("authorization").map(|s| s.as_str()),
            Some("Bearer test-key")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canned_tool_calls_builds_expected_shape() {
        let resp = canned_tool_calls(vec![("call_abc", "web_fetch", r#"{"url":"https://x"}"#)]);
        // Decode the response body as JSON and check the shape.
        let parsed: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        let tc = &parsed["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "web_fetch");
        assert_eq!(tc["function"]["arguments"], r#"{"url":"https://x"}"#);
        assert_eq!(parsed["choices"][0]["finish_reason"], "tool_calls");
        assert!(parsed["choices"][0]["message"]["content"].is_null());
    }

    // ───── Wire-contract sharpening (Run-4b offline proof, receipt 922d93f) ─────
    //
    // The real provider 400s Form A ("An assistant message with 'tool_calls'
    // must be followed by tool messages responding to each 'tool_call_id'");
    // the 14b mock accepted it — mock leniency hid the wire constraint.
    // These tests pin the sharpened behavior: A → 400, B/C → 200.

    fn tc(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "type": "function",
                           "function": {"name": "search", "arguments": "{}"}})
    }

    fn chat_body(messages: serde_json::Value) -> Vec<u8> {
        serde_json::json!({"model": "gpt-4o", "messages": messages})
            .to_string()
            .into_bytes()
    }

    async fn post_status(mock: &MockOpenAI, body: Vec<u8>) -> String {
        let addr = mock.addr;
        let resp = tokio::task::spawn_blocking(move || {
            raw_post_sync(
                addr,
                "/v1/chat/completions",
                &[("content-type", "application/json")],
                &body,
            )
        })
        .await
        .unwrap();
        String::from_utf8_lossy(&resp)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_a_consecutive_single_call_assistant_messages_rejected_400() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1")]},
            {"role": "assistant", "content": null, "tool_calls": [tc("c2")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let status = post_status(&mock, body).await;
        assert!(
            status.starts_with("HTTP/1.1 400"),
            "form A must 400, got {status}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_b_one_assistant_with_all_calls_accepted_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1"), tc("c2")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let status = post_status(&mock, body).await;
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "form B must 200, got {status}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_c_interleaved_call_result_pairs_accepted_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c2")]},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let status = post_status(&mock, body).await;
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "form C must 200, got {status}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canned_error_status_sets_status_and_body() {
        let resp = canned_error_status(429);
        assert_eq!(resp.status, 429);
        assert_eq!(resp.content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(parsed["error"]["message"], "mock 429 error");
    }
}
