//! Phase-10-C: `TelegramClient`. Reqwest-basierter Bot-API-Wrapper (W1:
//! reqwest only, no Telegram SDK). `get_updates` is the long poll (W2);
//! `send_message` the inbound sink call (T6). A timeouts per op via
//! `tokio::time::timeout` (W7). TLS gate: reqwest with `rustls-tls`
//! + `default-features = false` (Phase-7-TLS-Gate, archive/CLAUDE-phase-lessons.md § Phase 7).

use crate::proxy::io::{DocumentContent, ProxyEvent};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;

/// GH #907: the default ceiling of one document download, in bytes -- the
/// Bot API's own `getFile` limit of 20 MiB. The document is committed to the
/// colony blob store and the turn carries only its reference (R-FJ-1), so no
/// hop, context or message-log row grows with it; the ceiling is what one
/// download may cost, not what a turn may carry. An owner sets it lower.
pub const DEFAULT_MAX_DOCUMENT_BYTES: u64 = 20 * 1024 * 1024;

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
    /// GH #907: ceiling of one document download (`max_document_bytes`).
    max_document_bytes: u64,
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
            max_document_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
        })
    }

    /// GH #907: the same client with another document ceiling. The factory sets
    /// it from `params.max_document_bytes` at birth; the handler and the I/O
    /// task hold clones of the one client, so both see the same ceiling.
    pub fn with_max_document_bytes(mut self, max_document_bytes: u64) -> Self {
        self.max_document_bytes = max_document_bytes;
        self
    }

    /// The document ceiling this client downloads up to.
    pub fn max_document_bytes(&self) -> u64 {
        self.max_document_bytes
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
            max_document_bytes: self.max_document_bytes,
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
        Ok(results.iter().filter_map(parse_update).collect())
    }

    /// GH #907: `getFile` plus the download of one document, both steps under
    /// their own operation timeout (hard rule 12) and the whole body read
    /// inside it. Nothing is fetched past `max_document_bytes`: a size Telegram
    /// announced over the ceiling answers `TooLarge` without a request, and a
    /// body that grows past it is dropped mid-stream.
    ///
    /// The download URL carries the bot token, so no failure here says more
    /// than a fixed code (`get_file_failed`, `download_failed`, `timeout`) --
    /// a `reqwest` error prints its URL, and this string travels into a turn.
    pub async fn fetch_document(
        &self,
        file_id: &str,
        announced_size: Option<u64>,
        step_timeout: Duration,
    ) -> DocumentContent {
        let cap = self.max_document_bytes;
        if announced_size.is_some_and(|n| n > cap) {
            return DocumentContent::TooLarge;
        }
        let url = reqwest::Url::parse_with_params(
            &format!("{}/bot{}/getFile", self.base_url, self.bot_token),
            &[("file_id", file_id)],
        );
        let Ok(url) = url else {
            return DocumentContent::Failed("get_file_failed".into());
        };
        let step = async {
            let resp = self.inner.get(url).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp.json::<JsonValue>().await.ok()
        };
        let meta = match tokio::time::timeout(step_timeout, step).await {
            Err(_) => return DocumentContent::Failed("timeout".into()),
            Ok(None) => return DocumentContent::Failed("get_file_failed".into()),
            Ok(Some(v)) => v,
        };
        let result = meta.get("result");
        let Some(file_path) = result
            .and_then(|r| r.get("file_path"))
            .and_then(|v| v.as_str())
            .filter(|p| !p.is_empty())
        else {
            return DocumentContent::Failed("get_file_failed".into());
        };
        if result
            .and_then(|r| r.get("file_size"))
            .and_then(|v| v.as_u64())
            .is_some_and(|n| n > cap)
        {
            return DocumentContent::TooLarge;
        }
        let url = format!("{}/file/bot{}/{}", self.base_url, self.bot_token, file_path);
        let step = async {
            let mut resp = self
                .inner
                .get(&url)
                .send()
                .await
                .map_err(|_| "download_failed")?;
            if !resp.status().is_success() {
                return Err("download_failed");
            }
            let mut bytes: Vec<u8> = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|_| "download_failed")? {
                if (bytes.len() + chunk.len()) as u64 > cap {
                    return Err("too_large");
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        match tokio::time::timeout(step_timeout, step).await {
            Err(_) => DocumentContent::Failed("timeout".into()),
            Ok(Err("too_large")) => DocumentContent::TooLarge,
            Ok(Err(code)) => DocumentContent::Failed(code.into()),
            Ok(Ok(bytes)) => DocumentContent::Bytes(bytes),
        }
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

/// One update of `getUpdates` as the event the handler sees. GH #907: EVERY
/// update with an `update_id` yields an event, because the cursor may only
/// move past what the handler persisted -- before, a photo, a sticker or an
/// `edited_message` yielded nothing, `run_io` never moved past it and the
/// restart after it fetched the same update again, for ever. A text message is
/// a `UserMessage`, a `message.document` a `Document` (its bytes are fetched
/// by `run_io`), everything else a `Skipped` that only moves the cursor.
pub fn parse_update(v: &JsonValue) -> Option<ProxyEvent> {
    let update_id = v.get("update_id")?.as_i64()?;
    let skipped = ProxyEvent::Skipped { update_id };
    let Some(m) = v.get("message") else {
        return Some(skipped);
    };
    let Some(chat_id) = m
        .get("chat")
        .and_then(|c| c.get("id"))
        .and_then(|v| v.as_i64())
    else {
        return Some(skipped);
    };
    let user_id = m
        .get("from")
        .and_then(|f| f.get("id"))
        .and_then(|v| v.as_i64());
    let message_id = m.get("message_id").and_then(|v| v.as_i64());
    if let Some(text) = m.get("text").and_then(|v| v.as_str()) {
        return Some(ProxyEvent::UserMessage {
            update_id,
            chat_id,
            user_id,
            message_id,
            text: text.to_string(),
        });
    }
    let Some(doc) = m.get("document") else {
        return Some(skipped);
    };
    let Some(file_id) = doc.get("file_id").and_then(|v| v.as_str()) else {
        return Some(skipped);
    };
    let mime = doc
        .get("mime_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let name = doc
        .get("file_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if mime == "application/pdf" {
                "document.pdf".to_string()
            } else {
                "document".to_string()
            }
        });
    Some(ProxyEvent::Document {
        update_id,
        chat_id,
        user_id,
        message_id,
        caption: m
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        file_id: file_id.to_string(),
        name,
        mime,
        size: doc.get("file_size").and_then(|v| v.as_u64()),
        content: DocumentContent::NotFetched,
    })
}
