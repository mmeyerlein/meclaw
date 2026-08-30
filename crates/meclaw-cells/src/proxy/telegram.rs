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
/// against a dead token); `Conflict` → backs off like a `Transient`, but is a
/// class of its own so the caller can say what happened.
#[derive(Debug)]
pub enum TelegramError {
    /// 5xx, Timeout, Network, ungueltiges JSON, fehlendes `ok=true`.
    Transient(String),
    /// 401/403 — Token tot oder Bot gesperrt. Cell loggt + sleeped lange.
    Permanent(String),
    /// `409 Conflict` — GH #468. Telegram allows exactly ONE `getUpdates`
    /// consumer per bot token, and answers every other one with 409. It used to
    /// fall into `Transient`, which backs off on DEBUG: two pollers on one token
    /// then stole each other's updates in silence, and the symptom an operator
    /// saw was a bot answering every other message.
    ///
    /// The recovery is a `Transient`'s — the other consumer may go away, and a
    /// switchover is exactly the case where it does, so the lane must keep
    /// polling rather than fall into the 5-minute `Permanent` sleep. What the
    /// separate variant buys is the SENTENCE: the caller logs it at `warn` with
    /// `error_code = "conflict_other_poller"` instead of swallowing it.
    ///
    /// It is deliberately NOT an emission. The poll lane answers no message, so
    /// a receipt would have to be a source emission carrying `hop.error_code` —
    /// a fifth failure code every level holding a connector would owe a drain
    /// for, repeated on every backoff tick, for a condition only an operator can
    /// fix. The log line names the same thing and costs no contract.
    Conflict(String),
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
        // GH #468: 409 before the generic non-success branch — it is the one
        // status that names a cause the operator owns (a second consumer on this
        // token), and it must not disappear into the transient bucket.
        if status == reqwest::StatusCode::CONFLICT {
            return Err(TelegramError::Conflict(format!(
                "status: {status} - another getUpdates consumer holds this bot token"
            )));
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
        // GH #468: same classification on the inbound side. `handle` maps every
        // send failure onto `send_failed`, so the code the topology sees does not
        // change — but the `detail` it carries now names the conflict.
        if status == reqwest::StatusCode::CONFLICT {
            return Err(TelegramError::Conflict(format!(
                "status: {status} - another consumer holds this bot token"
            )));
        }
        if !status.is_success() {
            return Err(TelegramError::Transient(format!("status: {status}")));
        }
        Ok(())
    }

    /// POST `sendChatAction` (GH #515). Telegram renders "typing…" in the chat
    /// for roughly five seconds without a message ever being posted, which is
    /// the only way a connector can say "still working" without writing into the
    /// conversation it is supposed to carry.
    ///
    /// `chat_id` is a NUMBER in the body — same as `send_message`, and the same
    /// trap: a `chat_id` that turns into a string somewhere on the way here is
    /// rejected by the Bot API with a `chat not found`.
    ///
    /// Failures are classified like `send_message`'s, but the caller (the typing
    /// keeper) never turns them into an emission: a sign of life that fails to
    /// arrive must not cost the turn its answer. It is logged and the keeper
    /// keeps ticking — the next tick either works or the turn ends first.
    pub async fn send_chat_action(
        &self,
        chat_id: i64,
        action: &str,
        client_timeout: Duration,
    ) -> Result<(), TelegramError> {
        let url = format!("{}/bot{}/sendChatAction", self.base_url, self.bot_token);
        let body = json!({ "chat_id": chat_id, "action": action });
        let fut = self.inner.post(&url).json(&body).send();
        let resp = tokio::time::timeout(client_timeout, fut)
            .await
            .map_err(|_| TelegramError::Transient("client timeout".into()))?
            .map_err(|e| TelegramError::Transient(format!("send: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(TelegramError::Permanent(format!("auth: {status}")));
        }
        if status == reqwest::StatusCode::CONFLICT {
            return Err(TelegramError::Conflict(format!(
                "status: {status} - another consumer holds this bot token"
            )));
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
