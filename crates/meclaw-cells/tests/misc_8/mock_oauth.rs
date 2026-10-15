//! P10 test helper — fake OAuth token endpoint.
//!
//! Mirrors the reference refresh contract (`openai/codex` @ `266c6920`,
//! `login/src/auth/manager.rs:1506-1615`): a JSON request body carrying
//! `client_id` / `grant_type` / `refresh_token`, and a JSON response carrying
//! `access_token` / `refresh_token` and **no** expiry.
//!
//! The important test affordance is `refresh_count()`: the single-refresher
//! proof asserts that N concurrent 401s produce exactly ONE call here.
//!
//! Layering matches `mock_openai.rs`: provider-shaped knowledge lives in
//! `meclaw-cells/tests/`, the TCP mechanics in `meclaw-testing::mock_http`.

#![allow(dead_code)]

use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Handle on a fake token endpoint.
pub struct MockOauth {
    /// Full token-endpoint URL — usable as `params.oauth_token_endpoint`.
    pub token_endpoint: String,
    /// Every captured refresh request, in arrival order.
    pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

impl MockOauth {
    /// Start an endpoint that rotates the refresh token on every call:
    /// call *n* answers with `access-<n>` / `refresh-<n>`.
    ///
    /// `delay` slows each response down so concurrent callers genuinely
    /// overlap — without it a "concurrent" test can serialize by accident and
    /// prove nothing.
    pub async fn start_rotating(delay: Option<Duration>) -> Self {
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let validator: RequestValidator = Arc::new(move |req: &CapturedRequest| {
            // Contract check: the reference sends JSON, never form-urlencoded.
            let body: serde_json::Value = match serde_json::from_slice(&req.body) {
                Ok(b) => b,
                Err(_) => {
                    return Some(MockResponse {
                        status: 400,
                        body: serde_json::json!({"error": {"code": "invalid_request"}})
                            .to_string()
                            .into_bytes(),
                        content_type: "application/json".into(),
                        delay: None,
                    });
                }
            };
            if body.get("grant_type").and_then(|v| v.as_str()) != Some("refresh_token") {
                return Some(MockResponse {
                    status: 400,
                    body: serde_json::json!({"error": {"code": "unsupported_grant_type"}})
                        .to_string()
                        .into_bytes(),
                    content_type: "application/json".into(),
                    delay: None,
                });
            }
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let mut resp = MockResponse::ok_json(
                serde_json::json!({
                    "access_token": format!("access-{n}"),
                    "refresh_token": format!("refresh-{n}"),
                    "id_token": format!("id-{n}"),
                })
                .to_string()
                .as_bytes(),
            );
            resp.delay = delay;
            Some(resp)
        });
        Self::start_with(validator).await
    }

    /// Start an endpoint that always fails with the given permanent error code.
    pub async fn start_permanent_failure(code: &str) -> Self {
        let code = code.to_string();
        let validator: RequestValidator = Arc::new(move |_req: &CapturedRequest| {
            Some(MockResponse {
                status: 400,
                body: serde_json::json!({"error": {"code": code}})
                    .to_string()
                    .into_bytes(),
                content_type: "application/json".into(),
                delay: None,
            })
        });
        Self::start_with(validator).await
    }

    /// Start an endpoint that always fails with HTTP 500 (transient).
    pub async fn start_transient_failure() -> Self {
        let validator: RequestValidator = Arc::new(|_req: &CapturedRequest| {
            Some(MockResponse {
                status: 500,
                body: b"{}".to_vec(),
                content_type: "application/json".into(),
                delay: None,
            })
        });
        Self::start_with(validator).await
    }

    async fn start_with(validator: RequestValidator) -> Self {
        let (addr, join, captured) =
            start_mock_server_capturing_with_validator(Vec::new(), Some(validator)).await;
        Self {
            token_endpoint: format!("http://{addr}/oauth/token"),
            captured,
            join,
        }
    }

    /// How many refresh requests reached this endpoint.
    pub async fn refresh_count(&self) -> usize {
        self.captured.lock().await.len()
    }

    /// Parsed bodies of all captured refresh requests.
    pub async fn refresh_bodies(&self) -> Vec<serde_json::Value> {
        self.captured
            .lock()
            .await
            .iter()
            .filter_map(|c| serde_json::from_slice(&c.body).ok())
            .collect()
    }
}

/// Write a Codex-shaped `auth.json` into `dir` and return its path.
pub fn write_token_store(dir: &std::path::Path, refresh_token: &str) -> std::path::PathBuf {
    let p = dir.join("auth.json");
    let v = serde_json::json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": "id-dummy-0",
            "access_token": "access-dummy-0",
            "refresh_token": refresh_token,
            "account_id": "acct-dummy"
        },
        "last_refresh": "2026-08-08T20:51:57Z"
    });
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    p
}
