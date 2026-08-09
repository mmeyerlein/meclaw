//! P12 S3 — `SlackParams::parse`.
//!
//! Mandatory: `app_token` (xapp-, opens the Socket Mode connection), `bot_token`
//! (xoxb-, posts messages) and `emit_to` (routing target). Both tokens are
//! credentials: immutable at runtime and only ever supplied as `${VAR}`.
//!
//! Secret hygiene is a tested property here, not a convention — a params error
//! must never echo a token value, because params errors travel into the message
//! log and the operator UI.

use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_core::serde_json::json;

fn minimal() -> meclaw_core::JsonValue {
    json!({
        "app_token": "${SLACK_APP_TOKEN}",
        "bot_token": "${SLACK_BOT_TOKEN}",
        "emit_to": "/main/agent"
    })
}

#[test]
fn parses_minimal_config_with_defaults() {
    let p = SlackParams::parse(&minimal()).expect("minimal config must parse");
    assert_eq!(p.app_token, "${SLACK_APP_TOKEN}");
    assert_eq!(p.bot_token, "${SLACK_BOT_TOKEN}");
    assert_eq!(p.emit_to.as_str(), "/main/agent");
    // Defaults.
    assert_eq!(p.base_url, "https://slack.com/api");
    assert_eq!(p.connect_timeout_ms, 15000);
    assert_eq!(p.send_timeout_ms, 10000);
    assert_eq!(p.query_timeout_ms, 5000);
    assert_eq!(p.envelope_dedup_secs, 900);
    assert!(p.thread_follow, "thread_follow defaults to true (D-5)");
    assert!(p.bot_user_id.is_none(), "bot_user_id is optional (D-3)");
}

#[test]
fn app_token_is_required_and_named_in_the_error() {
    let mut v = minimal();
    v.as_object_mut().unwrap().remove("app_token");
    let err = SlackParams::parse(&v).unwrap_err();
    assert!(
        err.contains("app_token"),
        "error must name the field: {err}"
    );
}

#[test]
fn bot_token_is_required_and_named_in_the_error() {
    let mut v = minimal();
    v.as_object_mut().unwrap().remove("bot_token");
    let err = SlackParams::parse(&v).unwrap_err();
    assert!(
        err.contains("bot_token"),
        "error must name the field: {err}"
    );
}

#[test]
fn emit_to_is_required_and_named_in_the_error() {
    let mut v = minimal();
    v.as_object_mut().unwrap().remove("emit_to");
    let err = SlackParams::parse(&v).unwrap_err();
    assert!(err.contains("emit_to"), "error must name the field: {err}");
}

#[test]
fn optional_fields_are_read_when_present() {
    let v = json!({
        "app_token": "a", "bot_token": "b", "emit_to": "/x",
        "base_url": "http://127.0.0.1:9/api",
        "connect_timeout_ms": 1234,
        "send_timeout_ms": 2345,
        "query_timeout_ms": 3456,
        "envelope_dedup_secs": 60,
        "thread_follow": false,
        "bot_user_id": "U0BOTA"
    });
    let p = SlackParams::parse(&v).unwrap();
    assert_eq!(p.base_url, "http://127.0.0.1:9/api");
    assert_eq!(p.connect_timeout_ms, 1234);
    assert_eq!(p.send_timeout_ms, 2345);
    assert_eq!(p.query_timeout_ms, 3456);
    assert_eq!(p.envelope_dedup_secs, 60);
    assert!(!p.thread_follow);
    assert_eq!(p.bot_user_id.as_deref(), Some("U0BOTA"));
}

/// `base_url` is a config URL (like `llm.base_url` and the Telegram proxy's
/// `base_url`), NOT a credential — a trailing slash must not produce a double
/// slash in the built request URL.
#[test]
fn base_url_trailing_slash_is_normalised() {
    let v = json!({
        "app_token": "a", "bot_token": "b", "emit_to": "/x",
        "base_url": "https://slack.com/api/"
    });
    let p = SlackParams::parse(&v).unwrap();
    assert_eq!(p.base_url, "https://slack.com/api");
}

/// Secret hygiene (tested, not assumed): a parse error must never echo a token.
#[test]
fn parse_errors_never_echo_token_values() {
    let secret = "xoxb-1111-2222-thismustnotleak";
    // A bad neighbouring field forces an error while the tokens are present.
    let v = json!({
        "app_token": secret, "bot_token": secret, "emit_to": "/x",
        "connect_timeout_ms": "not-a-number-but-a-string"
    });
    if let Err(err) = SlackParams::parse(&v) {
        assert!(
            !err.contains(secret),
            "token leaked into params error: {err}"
        );
    }
    // And a missing-field error must not carry the other token either.
    let v2 = json!({ "app_token": secret, "emit_to": "/x" });
    let err = SlackParams::parse(&v2).unwrap_err();
    assert!(
        !err.contains(secret),
        "token leaked into params error: {err}"
    );
}

/// The `Debug` impl is the other place a secret escapes into logs.
#[test]
fn debug_impl_redacts_both_tokens() {
    let secret_app = "xapp-1-A0000-secretappvalue";
    let secret_bot = "xoxb-9999-secretbotvalue";
    let v = json!({ "app_token": secret_app, "bot_token": secret_bot, "emit_to": "/x" });
    let p = SlackParams::parse(&v).unwrap();
    let shown = format!("{p:?}");
    assert!(
        !shown.contains(secret_app),
        "app_token leaked via Debug: {shown}"
    );
    assert!(
        !shown.contains(secret_bot),
        "bot_token leaked via Debug: {shown}"
    );
}
