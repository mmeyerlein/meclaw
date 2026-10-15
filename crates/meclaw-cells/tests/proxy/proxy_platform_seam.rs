//! P12 S1 — the platform seam of the `proxy` cell type.
//!
//! `proxy` is by spec the "external-chat-platform bridge (Telegram first)"; Slack
//! is instance two of the SAME cell type, not a new one. The seam is a single
//! `params.platform` discriminator, parsed here and dispatched in the factory.
//!
//! The load-bearing rule is backward compatibility: `platform` is OPTIONAL and
//! defaults to `telegram`, so every `config.json` written before P12 keeps
//! parsing to exactly the same result. This mirrors the `mcp` cell's P7
//! transport seam (`mcp/params.rs:3-6`), which introduced `transport` under the
//! same optional-with-default rule.

use meclaw_cells::proxy::platform::{ProxyPlatform, parse_platform};
use meclaw_core::serde_json::json;

/// T-REG-1 (regression pin): a pre-P12 Telegram config carries no `platform`
/// key at all. It MUST still resolve to the Telegram path — this is the pin
/// that keeps every existing deployed topology byte-valid.
#[test]
fn absent_platform_key_defaults_to_telegram() {
    let pre_p12_config = json!({
        "bot_token": "${TELEGRAM_BOT_TOKEN}",
        "emit_to": "/main/agent"
    });
    assert_eq!(
        parse_platform(&pre_p12_config).expect("a pre-P12 config must stay valid"),
        ProxyPlatform::Telegram,
    );
}

#[test]
fn explicit_telegram_resolves_to_telegram() {
    let v = json!({ "platform": "telegram", "bot_token": "t", "emit_to": "/x" });
    assert_eq!(parse_platform(&v).unwrap(), ProxyPlatform::Telegram);
}

#[test]
fn explicit_slack_resolves_to_slack() {
    let v = json!({ "platform": "slack", "app_token": "a", "bot_token": "b", "emit_to": "/x" });
    assert_eq!(parse_platform(&v).unwrap(), ProxyPlatform::Slack);
}

/// An unknown platform is a loud reject naming the field AND the accepted
/// values — a silent fallback to Telegram would route a Slack topology into the
/// Telegram client and fail much later with a confusing auth error.
#[test]
fn unknown_platform_is_a_named_reject() {
    let err = parse_platform(&json!({ "platform": "discord" })).unwrap_err();
    assert!(err.contains("platform"), "error must name the field: {err}");
    assert!(err.contains("discord"), "error must echo the value: {err}");
    assert!(err.contains("telegram"), "error must list accepted: {err}");
    assert!(err.contains("slack"), "error must list accepted: {err}");
}

#[test]
fn non_string_platform_is_rejected() {
    let err = parse_platform(&json!({ "platform": 42 })).unwrap_err();
    assert!(err.contains("platform"), "error must name the field: {err}");
}

/// The params blob must be an object — same precondition the existing
/// `ProxyParams::parse` enforces (`proxy/params.rs:116`).
#[test]
fn non_object_params_is_rejected() {
    assert!(parse_platform(&json!("nope")).is_err());
    assert!(parse_platform(&json!([1, 2, 3])).is_err());
}
