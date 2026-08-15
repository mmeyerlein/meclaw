//! P12 S10/S11: the Slack Web API client — `apps.connections.open` and
//! `chat.postMessage`.
//!
//! Both tokens travel in the `Authorization` header and nowhere else. Slack
//! rejects an app-level token passed as a POST parameter outright, and the
//! header is also the only placement that keeps the secret out of URLs, redirect
//! traces and error strings. No error path in this module interpolates a token;
//! the Telegram variant cannot make that promise because its token sits in the
//! URL path, which is precisely why the two clients stay separate.
//!
//! Every call carries its own A-timeout (hard rule 12), and that deadline
//! covers the COMPLETE operation — send, status and body read — not merely the
//! header phase (issue #8, see `with_deadline`).

use super::params::SlackParams;
use serde_json::{Value as JsonValue, json};
use std::time::Duration;

/// Error classification driving the backoff decision.
#[derive(Debug)]
pub enum SlackError {
    /// Network, 5xx, timeout, rate limit — retry with exponential backoff.
    Transient(String),
    /// Dead token, missing scope, or a request that will never succeed as
    /// written. Retrying at speed is pointless and looks like an attack.
    Permanent(String),
}

impl std::fmt::Display for SlackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(m) => write!(f, "transient: {m}"),
            Self::Permanent(m) => write!(f, "permanent: {m}"),
        }
    }
}

/// Slack error codes that will not fix themselves by retrying.
fn is_permanent(code: &str) -> bool {
    matches!(
        code,
        "invalid_auth"
            | "not_authed"
            | "account_inactive"
            | "token_revoked"
            | "token_expired"
            | "not_allowed_token_type"
            | "missing_scope"
            | "no_permission"
            | "channel_not_found"
            | "not_in_channel"
            | "is_archived"
            | "no_text"
            | "msg_too_long"
            | "restricted_action"
            | "cannot_reply_to_message"
    )
}

/// Hard rule 12 envelope: bounds the WHOLE external operation, not just its
/// header phase.
///
/// A `tokio::time::timeout` around `send()` alone ends when the response
/// headers are in. The body read that follows (`resp.json().await`) then runs
/// with no deadline, and `reqwest::Client::builder().build()` sets none of its
/// own. A peer that answers with a head and then goes silent (NAT idle timeout,
/// blackholed path: no FIN, no RST) parks the call forever — no log line, no
/// progress, and the message-timeout backstop cannot see it because this call
/// lives in the long-running cell's I/O task, not in `handle()`. The Telegram
/// twin of this bug was measured in production on 2026-08-11; this is the same
/// fix in the same shape.
///
/// The budget is the call's existing A-timeout param — it simply covers the
/// whole operation now instead of half of it. Elapsed is classified
/// `Transient`, so the backoff ladder takes it and the lane keeps living.
async fn with_deadline<T>(
    budget: Duration,
    op_name: &str,
    op: impl std::future::Future<Output = Result<T, SlackError>>,
) -> Result<T, SlackError> {
    match tokio::time::timeout(budget, op).await {
        Ok(res) => res,
        Err(_) => {
            // Own, greppable line: this is the operation deadline, not a
            // connect error and not a Slack-side rejection.
            tracing::warn!(
                op = op_name,
                deadline_ms = budget.as_millis() as u64,
                "slack: call hung - killed by the operation deadline (transient)"
            );
            Err(SlackError::Transient(format!(
                "{op_name}: timeout after {} ms (whole operation)",
                budget.as_millis()
            )))
        }
    }
}

/// Web API client for one bot identity.
#[derive(Clone)]
pub struct SlackClient {
    inner: reqwest::Client,
    base_url: String,
    app_token: String,
    bot_token: String,
    connect_timeout: Duration,
    send_timeout: Duration,
}

impl SlackClient {
    /// Builds the client from validated params.
    pub fn new(params: &SlackParams) -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest build: {e}"))?;
        Ok(Self {
            inner,
            base_url: params.base_url.clone(),
            app_token: params.app_token.clone(),
            bot_token: params.bot_token.clone(),
            connect_timeout: Duration::from_millis(params.connect_timeout_ms),
            send_timeout: Duration::from_millis(params.send_timeout_ms),
        })
    }

    /// β (path B): rebuild with a new `base_url`, keeping both tokens from THIS
    /// client's own state.
    ///
    /// The tokens are deliberately re-held rather than re-read from the update:
    /// `app_token` and `bot_token` are immutable params and a runtime update
    /// carrying either is rejected, so a secret can never enter the process
    /// through the params surface.
    pub fn with_base_url(&self, base_url: &str) -> Self {
        Self {
            inner: self.inner.clone(),
            base_url: base_url.trim_end_matches('/').to_string(),
            app_token: self.app_token.clone(),
            bot_token: self.bot_token.clone(),
            connect_timeout: self.connect_timeout,
            send_timeout: self.send_timeout,
        }
    }

    /// Opens a Socket Mode connection and returns the WebSocket URL.
    ///
    /// The app-level token goes in the header — passing it as a POST parameter
    /// is an error on Slack's side, not merely bad style.
    pub async fn apps_connections_open(&self) -> Result<String, SlackError> {
        let url = format!("{}/apps.connections.open", self.base_url);
        let op = async {
            let resp = self
                .inner
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.app_token))
                .header("Content-Type", "application/json")
                .body("{}")
                .send()
                .await
                .map_err(|e| SlackError::Transient(format!("apps.connections.open: {e}")))?;

            let status = resp.status();
            let body: JsonValue = resp
                .json()
                .await
                .map_err(|e| SlackError::Transient(format!("apps.connections.open: body: {e}")))?;
            if status.as_u16() == 429 {
                return Err(SlackError::Transient(
                    "apps.connections.open: ratelimited".into(),
                ));
            }
            if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                return body
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        SlackError::Transient("apps.connections.open: ok without url".into())
                    });
            }
            let code = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let msg = format!("apps.connections.open: {code}");
            Err(if is_permanent(code) {
                SlackError::Permanent(msg)
            } else {
                SlackError::Transient(msg)
            })
        };
        with_deadline(self.connect_timeout, "apps.connections.open", op).await
    }

    /// Posts a message. `thread_ts` set means the reply lands in that thread.
    pub async fn chat_post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<(), SlackError> {
        let url = format!("{}/chat.postMessage", self.base_url);
        let mut payload = json!({ "channel": channel, "text": text });
        if let Some(t) = thread_ts {
            payload["thread_ts"] = json!(t);
        }
        let op = async {
            let resp = self
                .inner
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.bot_token))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| SlackError::Transient(format!("chat.postMessage: {e}")))?;

            let status = resp.status();
            if status.as_u16() == 429 {
                return Err(SlackError::Transient(
                    "chat.postMessage: ratelimited".into(),
                ));
            }
            let body: JsonValue = resp
                .json()
                .await
                .map_err(|e| SlackError::Transient(format!("chat.postMessage: body: {e}")))?;
            if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                return Ok(());
            }
            // Slack spells its rate limit two ways: `rate_limited` on the app
            // tier and `ratelimited` on the HTTP 429 path. Both are transient.
            let code = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let msg = format!("chat.postMessage: {code}");
            Err(if is_permanent(code) {
                SlackError::Permanent(msg)
            } else {
                SlackError::Transient(msg)
            })
        };
        with_deadline(self.send_timeout, "chat.postMessage", op).await
    }
}
