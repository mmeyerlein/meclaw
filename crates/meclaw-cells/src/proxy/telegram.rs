//! Phase-10-C: `TelegramClient`. Reqwest-basierter Bot-API-Wrapper (W1:
//! reqwest only, no Telegram SDK). `get_updates` is the long poll (W2);
//! `send_message` the inbound sink call (T6). A timeouts per op via
//! `tokio::time::timeout` (W7). TLS gate: reqwest with `rustls-tls`
//! + `default-features = false` (Phase-7-TLS-Gate, archive/CLAUDE-phase-lessons.md § Phase 7).

use crate::proxy::io::ProxyEvent;
use serde_json::{Value as JsonValue, json};
use std::time::Duration;

/// Error classification for the backoff decision (W8). `Transient` →
/// exponential backoff (1s → 60s); `Permanent` → a constant 5 min (no busy spin
/// against a dead token).
#[derive(Debug)]
pub enum TelegramError {
    /// 5xx, Timeout, Network, ungueltiges JSON, fehlendes `ok=true`.
    Transient(String),
    /// 401/403 — Token tot oder Bot gesperrt. Cell loggt + sleeped lange.
    Permanent(String),
}

/// Telegram-Bot-API-Client.
///
/// Holds a `reqwest::Client` (Arc internally) + bot token + `base_url`. `Clone`
/// is cheap (Arc internally). One client is built per cell instance in the
/// factory; `ProxyCell` and `ProxyIo` each hold a clone.
#[derive(Clone)]
pub struct TelegramClient {
    inner: reqwest::Client,
    base_url: String,
    bot_token: String,
}

impl TelegramClient {
    /// Builds the client. A build failure (e.g. TLS init) yields a string error
    /// for the factory spawn path.
    pub fn new(base_url: &str, bot_token: &str) -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest build: {e}"))?;
        Ok(Self {
            inner,
            base_url: base_url.trim_end_matches('/').to_string(),
            bot_token: bot_token.to_string(),
        })
    }

    /// β (path B): rebuild with a new `base_url`, keeping the **immutable**
    /// `bot_token` + the (Arc-internal) reqwest client. Used when a runtime
    /// params-update changes `base_url` — the I/O-task and the handler swap their
    /// client live. The `bot_token` is rehold from THIS client's internal state,
    /// never from the update (a `bot_token` update key stays a reject), so the
    /// secret never crosses the params surface.
    pub fn with_base_url(&self, base_url: &str) -> Self {
        Self {
            inner: self.inner.clone(),
            base_url: base_url.trim_end_matches('/').to_string(),
            bot_token: self.bot_token.clone(),
        }
    }

    /// Long-Poll `getUpdates`. A-Timeout via `tokio::time::timeout`. Telegram-
    /// side timeout via the query param `timeout=<sec>`. The W7 tripwire is
    /// validated in `ProxyParams::parse` — here it is only respected.
    pub async fn get_updates(
        &self,
        offset: i64,
        long_poll_request_secs: u64,
        client_timeout: Duration,
    ) -> Result<Vec<ProxyEvent>, TelegramError> {
        let url = format!(
            "{}/bot{}/getUpdates?offset={}&timeout={}",
            self.base_url, self.bot_token, offset, long_poll_request_secs
        );
        let fut = self.inner.get(&url).send();
        let resp = tokio::time::timeout(client_timeout, fut)
            .await
            .map_err(|_| TelegramError::Transient("client timeout".into()))?
            .map_err(|e| TelegramError::Transient(format!("send: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(TelegramError::Permanent(format!("auth: {status}")));
        }
        if !status.is_success() {
            return Err(TelegramError::Transient(format!("status: {status}")));
        }
        let json: JsonValue = resp
            .json()
            .await
            .map_err(|e| TelegramError::Transient(format!("json: {e}")))?;
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(TelegramError::Transient(format!("ok=false: {json}")));
        }
        let results = json
            .get("result")
            .and_then(|v| v.as_array())
            .ok_or_else(|| TelegramError::Transient("missing result array".into()))?;
        Ok(results.iter().filter_map(parse_update_text_only).collect())
    }

    /// POST `sendMessage`. A-Timeout via `tokio::time::timeout`. 401/403 →
    /// `Permanent` (the inbound sink caller W6 maps this onto a `send_failed`
    /// error reply regardless of the classification — backoff classification is a
    /// long-poll concern, not an inbound one).
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        client_timeout: Duration,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/bot{}/sendMessage", self.base_url, self.bot_token);
        let body = json!({ "chat_id": chat_id, "text": text });
        let fut = self.inner.post(&url).json(&body).send();
        let resp = tokio::time::timeout(client_timeout, fut)
            .await
            .map_err(|_| TelegramError::Transient("client timeout".into()))?
            .map_err(|e| TelegramError::Transient(format!("send: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(TelegramError::Permanent(format!("auth: {status}")));
        }
        if !status.is_success() {
            return Err(TelegramError::Transient(format!("status: {status}")));
        }
        Ok(())
    }
}

/// Extracts `message.text` updates only. `edited_message`, `callback_query` etc.
/// are ignored (10-C scope).
fn parse_update_text_only(v: &JsonValue) -> Option<ProxyEvent> {
    let update_id = v.get("update_id")?.as_i64()?;
    let m = v.get("message")?;
    let text = m.get("text")?.as_str()?.to_string();
    let chat_id = m.get("chat")?.get("id")?.as_i64()?;
    let user_id = m
        .get("from")
        .and_then(|f| f.get("id"))
        .and_then(|v| v.as_i64());
    let message_id = m.get("message_id").and_then(|v| v.as_i64());
    Some(ProxyEvent::UserMessage {
        update_id,
        chat_id,
        user_id,
        message_id,
        text,
    })
}
