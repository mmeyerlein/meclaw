//! P12: the platform seam of the `proxy` cell type.
//!
//! `proxy` is by spec the external-chat-platform bridge ("Telegram first"), and
//! Slack is instance two of the SAME cell type — not a new one. This module
//! holds the single discriminator that the factory dispatches on.
//!
//! `platform` is OPTIONAL and defaults to [`ProxyPlatform::Telegram`], so every
//! `config.json` written before P12 keeps parsing to exactly the same result.
//! The `mcp` cell introduced its `transport` seam under the same rule in P7
//! (see `crate::mcp::params`), and this follows that precedent deliberately.

use serde_json::Value as JsonValue;

/// Which external chat platform a `proxy` instance bridges to.
///
/// One instance bridges exactly one platform. For Slack this also means one
/// instance == one Slack app == one bot identity (P12 multi-bot model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyPlatform {
    /// Telegram Bot API over HTTP long poll. The default when `platform` is absent.
    Telegram,
    /// Slack over Socket Mode (outbound WebSocket, app-level token).
    Slack,
}

impl ProxyPlatform {
    /// The wire spelling used in `params.platform` and in the emitted
    /// `platform` header.
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyPlatform::Telegram => "telegram",
            ProxyPlatform::Slack => "slack",
        }
    }
}

/// Reads `params.platform`. Absent → [`ProxyPlatform::Telegram`] (backward
/// compatibility, load-bearing). An unknown or non-string value is a loud
/// reject that names the field, echoes the offending value and lists the
/// accepted spellings — a silent fallback would route a Slack topology into the
/// Telegram client and only surface much later as a confusing auth error.
pub fn parse_platform(params: &JsonValue) -> Result<ProxyPlatform, String> {
    let obj = params.as_object().ok_or("params: must be object")?;
    match obj.get("platform") {
        None => Ok(ProxyPlatform::Telegram),
        Some(JsonValue::String(s)) if s == "telegram" => Ok(ProxyPlatform::Telegram),
        Some(JsonValue::String(s)) if s == "slack" => Ok(ProxyPlatform::Slack),
        Some(JsonValue::String(s)) => Err(format!(
            "platform: unknown value {s:?} (accepted: \"telegram\", \"slack\")"
        )),
        Some(other) => Err(format!(
            "platform: must be a string, got {other} (accepted: \"telegram\", \"slack\")"
        )),
    }
}
