//! P12 S2 — the factory dispatches validation on `params.platform`.
//!
//! This is THE seam. `validate_params` must route a Telegram config through the
//! unchanged `ProxyParams::parse` and a Slack config through `SlackParams::parse`,
//! while a config that names no platform keeps behaving exactly as it did before
//! P12.
//!
//! The parser invariant from `meclaw_colony::CellFactory` still holds per branch:
//! `validate_params` routes through the same parse path the spawn will use, so a
//! config that validates cannot fail to spawn for a params reason.

use meclaw_cells::proxy::ProxyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;

fn factory() -> Arc<ProxyCellFactory> {
    Arc::new(ProxyCellFactory)
}

/// T-REG-2 (regression pin): the historical minimal Telegram config — exactly
/// the shape asserted in the pre-P12 test `proxy_factory.rs` / `factory.rs`
/// unit test — must still validate, and the missing-`bot_token` error text must
/// still name `bot_token`.
#[test]
fn telegram_path_is_unchanged_without_a_platform_key() {
    let f = factory();
    f.clone()
        .validate_params(&json!({"bot_token": "t", "emit_to": "/x"}))
        .expect("the historical minimal Telegram config must still validate");

    let err = f
        .validate_params(&json!({"emit_to": "/x"}))
        .expect_err("missing bot_token must still be rejected");
    assert!(
        err.contains("bot_token"),
        "the pre-P12 error text must be preserved: {err}"
    );
}

/// The W7 tripwire is Telegram-specific and must keep firing on the Telegram
/// branch — proof that the dispatch did not swap in a laxer parser.
#[test]
fn telegram_w7_tripwire_still_fires_through_the_factory() {
    let f = factory();
    let err = f
        .validate_params(&json!({
            "bot_token": "t", "emit_to": "/x",
            "long_poll_timeout_ms": 30000,
            "long_poll_request_secs": 30
        }))
        .expect_err("W7 tripwire must still reject client <= server timeout");
    assert!(err.contains("W7"), "W7 tripwire text expected: {err}");
}

#[test]
fn explicit_telegram_platform_uses_the_telegram_parser() {
    let f = factory();
    f.clone()
        .validate_params(&json!({
            "platform": "telegram", "bot_token": "t", "emit_to": "/x"
        }))
        .expect("explicit telegram must validate");

    // A Slack-shaped config declared as telegram must fail on the TELEGRAM
    // parser (missing bot_token is a Telegram requirement here).
    let err = f
        .validate_params(&json!({
            "platform": "telegram", "app_token": "a", "emit_to": "/x"
        }))
        .expect_err("telegram parser must demand bot_token");
    assert!(err.contains("bot_token"), "{err}");
}

#[test]
fn slack_platform_uses_the_slack_parser() {
    let f = factory();
    f.clone()
        .validate_params(&json!({
            "platform": "slack",
            "app_token": "${SLACK_APP_TOKEN}",
            "bot_token": "${SLACK_BOT_TOKEN}",
            "emit_to": "/main/agent"
        }))
        .expect("a valid slack config must validate");

    // Telegram's long-poll fields are meaningless for Slack; their absence must
    // NOT be demanded, and Slack's own required field must be.
    let err = f
        .validate_params(&json!({
            "platform": "slack", "bot_token": "b", "emit_to": "/x"
        }))
        .expect_err("slack parser must demand app_token");
    assert!(err.contains("app_token"), "{err}");
}

/// A Telegram config that would trip W7 must NOT trip it when declared as
/// slack — the Slack branch has no long-poll semantics at all. This proves the
/// two parsers are genuinely separate, not one parser with extra fields.
#[test]
fn slack_branch_does_not_apply_the_telegram_tripwire() {
    let f = factory();
    f.validate_params(&json!({
        "platform": "slack",
        "app_token": "a", "bot_token": "b", "emit_to": "/x",
        "long_poll_timeout_ms": 30000,
        "long_poll_request_secs": 30
    }))
    .expect("long_poll_* are inert on the slack branch");
}

#[test]
fn unknown_platform_is_rejected_by_the_factory() {
    let f = factory();
    let err = f
        .validate_params(&json!({"platform": "discord", "emit_to": "/x"}))
        .expect_err("unknown platform must be rejected");
    assert!(err.contains("platform"), "{err}");
}
