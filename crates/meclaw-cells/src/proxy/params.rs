//! Phase-10-C: `ProxyParams`. Pflichtfelder `bot_token` + `emit_to` (W4);
//! Defaults from W7. W7 tripwire: `long_poll_timeout_ms > long_poll_request_secs * 1000`
//! — the client timeout must be strictly greater than the
//! `getUpdates?timeout=<sec>` value sent to Telegram, otherwise the client cuts
//! off its own valid poll. `base_url` is optional (default
//! `https://api.telegram.org`, an override for mock tests).

use meclaw_core::Path;
use serde_json::Value as JsonValue;

/// Parsed proxy params after validation. All fields owned (no borrow).
#[derive(Debug, Clone)]
pub struct ProxyParams {
    /// Bot token resolved via `${TELEGRAM_BOT_TOKEN}` substitution
    /// (Colony-resolved before hand-off). Required field.
    pub bot_token: String,
    /// Routing target for every emitted user-source message (W4). Cell sets
    /// `target = emit_to`; edge modifier can override. Required field.
    pub emit_to: Path,
    /// Client-side A-timeout (`tokio::time::timeout`) wrapping the
    /// `getUpdates` call. **W7-tripwire**: must be strictly greater than
    /// `long_poll_request_secs * 1000`. Default 35000.
    pub long_poll_timeout_ms: u64,
    /// Telegram-side `timeout=<sec>` param in the `getUpdates` call
    /// (server-side long-poll wait). Default 30.
    pub long_poll_request_secs: u64,
    /// Client-side A-timeout wrapping the `sendMessage` call. Default 10000.
    pub send_timeout_ms: u64,
    /// A-timeout for `cell.db` calls via `DbConn`. Default 5000.
    pub query_timeout_ms: u64,
    /// Telegram API base URL. Default `https://api.telegram.org`. Test
    /// override for `mock_http` server (e.g. `http://127.0.0.1:<port>`).
    pub base_url: String,
}

/// β: the `proxy` runtime-overlay projection — the mutable, runtime-tunable
/// fields. Only `bot_token` + `emit_to` (credential / routing identity) are
/// immutable. `base_url` is a config URL (like `llm.base_url`), NOT a
/// credential — mutable, path B (the client is rebuilt live, the immutable
/// `bot_token` rehold from existing state, never from the update). A minimal
/// projection (not full `ProxyParams`) so the round-trip never carries
/// `bot_token`/`emit_to`. `KNOWN_KEYS` lists the immutable keys so an update
/// touching them is a loud `Immutable` reject.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProxyOverlay {
    /// Telegram API base URL. Mutable, path B (I/O task + handler swap the client
    /// live). NOT a credential — a config URL like `llm.base_url`.
    pub base_url: String,
    /// Client-side long-poll A timeout. Mutable, path B (I/O task, live).
    pub long_poll_timeout_ms: u64,
    /// Telegram-side long-poll wait. Mutable, path B (I/O task, live).
    pub long_poll_request_secs: u64,
    /// `sendMessage` A timeout. Mutable, path A (handle side, live).
    pub send_timeout_ms: u64,
    /// cell.db A timeout. Mutable, path C (DbConn, live).
    pub query_timeout_ms: u64,
}

impl crate::params_overlay::OverlayParams for ProxyOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "bot_token",
        "emit_to",
        "base_url",
        "long_poll_timeout_ms",
        "long_poll_request_secs",
        "send_timeout_ms",
        "query_timeout_ms",
    ];
    const IMMUTABLE_KEYS: &'static [&'static str] = &["bot_token", "emit_to"];
    fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params: must be object")?;
        let base_url = obj
            .get("base_url")
            .and_then(|x| x.as_str())
            .unwrap_or("https://api.telegram.org")
            .to_string();
        let long_poll_timeout_ms = obj
            .get("long_poll_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(35000);
        let long_poll_request_secs = obj
            .get("long_poll_request_secs")
            .and_then(|x| x.as_u64())
            .unwrap_or(30);
        let send_timeout_ms = obj
            .get("send_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(10000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        // W7-tripwire holds at runtime too (same invariant as spawn-time parse).
        if long_poll_timeout_ms <= long_poll_request_secs * 1000 {
            return Err(format!(
                "W7-Tripwire: long_poll_timeout_ms ({long_poll_timeout_ms}) \
                 must be strictly > long_poll_request_secs ({long_poll_request_secs}) * 1000 \
                 = {}",
                long_poll_request_secs * 1000
            ));
        }
        Ok(Self {
            base_url,
            long_poll_timeout_ms,
            long_poll_request_secs,
            send_timeout_ms,
            query_timeout_ms,
        })
    }
}

impl ProxyParams {
    /// Parse + validate. Required fields rejected with explicit field name;
    /// W7-tripwire rejected with both field names + comparison values.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;
        // An EMPTY token is not a token (GH #270). `${TELEGRAM_BOT_TOKEN}`
        // without a default already fails loudly when the variable is unset;
        // `TELEGRAM_BOT_TOKEN=` in an .env slipped through and left the poller
        // asking `.../bot/getUpdates` — a 404 loop against a bot that has no
        // name. Same message as the absent case: same mistake, same fix.
        let bot_token = obj
            .get("bot_token")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("bot_token: required (use ${TELEGRAM_BOT_TOKEN})")?
            .to_string();
        let emit_to_s = obj
            .get("emit_to")
            .and_then(|x| x.as_str())
            .ok_or("emit_to: required (absolute path)")?;
        let long_poll_timeout_ms = obj
            .get("long_poll_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(35000);
        let long_poll_request_secs = obj
            .get("long_poll_request_secs")
            .and_then(|x| x.as_u64())
            .unwrap_or(30);
        let send_timeout_ms = obj
            .get("send_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(10000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        let base_url = obj
            .get("base_url")
            .and_then(|x| x.as_str())
            .unwrap_or("https://api.telegram.org")
            .to_string();

        // W7-tripwire (strictly >, not >=): if client == server, the client
        // cuts off the last-millisecond tick of the legitimate Telegram poll.
        if long_poll_timeout_ms <= long_poll_request_secs * 1000 {
            return Err(format!(
                "W7-Tripwire: long_poll_timeout_ms ({long_poll_timeout_ms}) \
                 must be strictly > long_poll_request_secs ({long_poll_request_secs}) * 1000 \
                 = {}",
                long_poll_request_secs * 1000
            ));
        }

        Ok(Self {
            bot_token,
            emit_to: Path::new(emit_to_s),
            long_poll_timeout_ms,
            long_poll_request_secs,
            send_timeout_ms,
            query_timeout_ms,
            base_url,
        })
    }
}
