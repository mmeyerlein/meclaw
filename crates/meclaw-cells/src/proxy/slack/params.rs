//! P12: `SlackParams` — the Slack platform variant's birth configuration.
//!
//! Identity model (P12 D-3): one cell instance == one Slack app == one bot
//! token == one bot identity. The two tokens are credentials — immutable at
//! runtime, supplied only as `${VAR}`, and never echoed into an error, a log
//! field or the `Debug` output.
//!
//! Timeout discipline (hard rule 12): every external op carries its own
//! A-timeout. `connect_timeout_ms` wraps `apps.connections.open` and the
//! WebSocket connect; `send_timeout_ms` wraps `chat.postMessage`;
//! `query_timeout_ms` wraps `cell.db` ops via `DbConn`. The Socket Mode read
//! loop has no operation boundary to wrap, so it carries the one deadline that
//! fits a stream instead: `idle_timeout_ms`, reset by every arriving frame.

use meclaw_core::Path;
use serde_json::Value as JsonValue;

/// Parsed slack params after validation. All fields owned (no borrow).
///
/// `Debug` is implemented by hand so the two tokens never reach a log line.
#[derive(Clone)]
pub struct SlackParams {
    /// App-level token (`xapp-…`) used for `apps.connections.open`. Required,
    /// immutable, `${VAR}` only.
    pub app_token: String,
    /// Bot token (`xoxb-…`) used for `chat.postMessage`. Required, immutable,
    /// `${VAR}` only. This token IS the bot's identity in the workspace.
    pub bot_token: String,
    /// Routing target for every emitted user-source message. Required.
    pub emit_to: Path,
    /// Slack Web API base URL. Default `https://slack.com/api`. Test override
    /// points at the hermetic mock server (same mechanism the Telegram variant
    /// uses for `base_url`). Trailing slashes are normalised away.
    pub base_url: String,
    /// A-timeout around `apps.connections.open` and the WebSocket connect.
    pub connect_timeout_ms: u64,
    /// Idle deadline for the Socket Mode read loop (issue #50).
    ///
    /// The stream analogue of an A-timeout: not a budget for one operation, but
    /// the longest silence an open connection may show before it is presumed
    /// dead. Every arriving frame resets it — events, `hello`, `disconnect`,
    /// and the WebSocket ping/pong control frames alike, because any of them is
    /// proof the path still carries traffic.
    ///
    /// Derivation of the 120 000 ms default. Slack keeps a Socket Mode
    /// connection audible on its own: it sends WebSocket pings on a regular
    /// cadence (roughly one every 10–30 s) and additionally recycles the
    /// connection every few minutes with a `disconnect` frame. A healthy lane
    /// is therefore never quiet for long, even in a workspace that produces no
    /// events at all — which is what makes silence a usable death signal here
    /// and would make it a useless one on a stream without keepalives. 120 s is
    /// four missed pings at the slowest documented cadence (4 × 30 s): wide
    /// enough that a hiccup, a scheduler stall or a single lost ping cannot
    /// tear down a working socket, narrow enough to bound a blackholed path
    /// (NAT idle timeout, dropped route, no FIN, no RST) to two minutes instead
    /// of forever.
    ///
    /// On elapse the connection ends as `ConnectionEnd::Transient` and the
    /// ordinary reconnect machinery takes over. It is not a panic and not a
    /// cell death.
    pub idle_timeout_ms: u64,
    /// A-timeout around `chat.postMessage`.
    pub send_timeout_ms: u64,
    /// A-timeout for `cell.db` calls via `DbConn`.
    pub query_timeout_ms: u64,
    /// Retention window for the `seen_envelopes` dedup table, in seconds.
    /// Slack redelivers an un-acked envelope a small number of times over a
    /// short window; the default is deliberately far larger than that window so
    /// a retry can never slip past the dedup and emit twice.
    pub envelope_dedup_secs: u64,
    /// Whether the bot keeps following a thread it owns after the initial
    /// mention, without needing to be mentioned again (P12 D-5). Default true.
    pub thread_follow: bool,
    /// The bot's own Slack user id (`U…`), optional. Used only for the
    /// defensive self-filter R4; own messages already carry `bot_id` and are
    /// dropped by R1, so this is belt-and-braces, not a requirement (D-3).
    pub bot_user_id: Option<String>,
}

/// Hand-written so neither token can reach a log line or a panic message.
impl std::fmt::Debug for SlackParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackParams")
            .field("app_token", &"<redacted>")
            .field("bot_token", &"<redacted>")
            .field("emit_to", &self.emit_to)
            .field("base_url", &self.base_url)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("send_timeout_ms", &self.send_timeout_ms)
            .field("query_timeout_ms", &self.query_timeout_ms)
            .field("envelope_dedup_secs", &self.envelope_dedup_secs)
            .field("thread_follow", &self.thread_follow)
            .field("bot_user_id", &self.bot_user_id)
            .finish()
    }
}

impl SlackParams {
    /// Parse + validate. Required fields are rejected with an explicit field
    /// name. No error path interpolates a token value — see the secret-hygiene
    /// tests in `tests/slack_params_parse.rs`.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;
        // An EMPTY token is not a token (GH #270). Both are declared as
        // `${SLACK_…}` without a default, so an unset variable already fails
        // loudly at boot; `SLACK_BOT_TOKEN=` in an .env slipped through and
        // sent `Authorization: Bearer ` to slack.com, which answers
        // `invalid_auth` on every call while the cell looks healthy. Same
        // message as the absent case: same mistake, same fix.
        let app_token = obj
            .get("app_token")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("app_token: required (use ${SLACK_APP_TOKEN}, an xapp- token)")?
            .to_string();
        let bot_token = obj
            .get("bot_token")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("bot_token: required (use ${SLACK_BOT_TOKEN}, an xoxb- token)")?
            .to_string();
        let emit_to_s = obj
            .get("emit_to")
            .and_then(|x| x.as_str())
            .ok_or("emit_to: required (absolute path)")?;
        let base_url = obj
            .get("base_url")
            .and_then(|x| x.as_str())
            .unwrap_or("https://slack.com/api")
            .trim_end_matches('/')
            .to_string();
        let connect_timeout_ms = obj
            .get("connect_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(15000);
        let idle_timeout_ms = obj
            .get("idle_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(120_000);
        let send_timeout_ms = obj
            .get("send_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(10000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        let envelope_dedup_secs = obj
            .get("envelope_dedup_secs")
            .and_then(|x| x.as_u64())
            .unwrap_or(900);
        let thread_follow = obj
            .get("thread_follow")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        let bot_user_id = obj
            .get("bot_user_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());

        Ok(Self {
            app_token,
            bot_token,
            emit_to: Path::new(emit_to_s),
            base_url,
            connect_timeout_ms,
            idle_timeout_ms,
            send_timeout_ms,
            query_timeout_ms,
            envelope_dedup_secs,
            thread_follow,
            bot_user_id,
        })
    }
}
