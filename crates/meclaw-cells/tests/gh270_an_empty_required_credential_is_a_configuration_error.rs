//! GH #270, the other direction — a REQUIRED value that arrives empty is a
//! configuration error, and it has to be named where it is made.
//!
//! The sweep that found the `web_search` bearer asked the mirrored question:
//! `parse_params_pure` checks `endpoint.is_empty()`, so somebody thought about
//! this once — is it thought about everywhere? It is not. Five required
//! strings across three cells were taken with `.and_then(|x| x.as_str())` and
//! nothing else, so `""` passed validation and the cell came up "healthy":
//!
//! | cell | key | what an empty value did |
//! |---|---|---|
//! | `mcp` (http) | `endpoint` | every request failed at URL build, per call |
//! | `mcp` (stdio) | `command` | the child spawn failed, per call |
//! | `proxy` (telegram) | `bot_token` | polls `…/bot/getUpdates` forever, 404 |
//! | `proxy` (slack) | `app_token` | `Authorization: Bearer ` → `invalid_auth` |
//! | `proxy` (slack) | `bot_token` | `Authorization: Bearer ` → `invalid_auth` |
//!
//! Each of those declares its value as `${VAR}` without a default, so an
//! *unset* variable is already a hard, named boot failure — that part worked.
//! The hole is `VAR=` in an `.env`, which is what a half-filled copy of
//! `.env.example` looks like: the substitution yields `""`, the parser accepts
//! it, and the operator gets a running colony that fails at a third party
//! instead of a refusal that names the variable.
//!
//! # Why this pin sits at the parser and not at the wire
//!
//! The `web_search` pin (`gh270_the_shipped_search_key_reaches_the_wire`) has
//! to reach the socket, because there the claim is about the *shape of a
//! request that is still made*. Here the claim is the opposite one — that no
//! request is ever made, because the cell refuses to exist. A wire assertion
//! cannot observe a cell that never spawned; the parser IS the boundary the
//! behaviour lives on, and it is the same boundary `validate_params` calls
//! before a mutation commits.
//!
//! # Why an empty value reuses the "required" message rather than a new one
//!
//! For an operator, `bot_token=` and a missing `bot_token` are the same
//! mistake with the same fix, and the existing message already names the
//! variable to set. A second error string would be a second thing to keep in
//! sync with the templates for no gain.
//!
//! # What is deliberately NOT here: `llm`'s `api_key`
//!
//! It has the same shape — `LlmParams::parse` rejects `api_key: null` for
//! `auth: "api_key"` but accepts `""`, and the wire then sends
//! `Authorization: Bearer `. It is left alone on purpose: an
//! OpenAI-compatible server on localhost commonly ignores the header
//! altogether, `templates/builder-hive/intake-llm` points at exactly such an
//! endpoint, and an operator running keyless against it would be broken by a
//! refusal that is right for Slack and Telegram. Whether that lane should
//! reject the empty key or drop the header is a judgement about the auth
//! contract, not a defect repair, and it is reported rather than decided here.

use meclaw_cells::mcp::params::McpParams;
use meclaw_cells::proxy::params::ProxyParams;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_core::serde_json::json;

/// An empty value must fail exactly like an absent one, and the message must
/// still name the key — that name is the whole value of failing here.
fn assert_rejected_like_absent(result: Result<(), String>, key: &str, what: &str) {
    let err = match result {
        Ok(()) => panic!(
            "{what}: an empty `{key}` was accepted. The cell then comes up looking healthy and \
             fails at a third party on every call, which is the failure mode this repair exists \
             to remove — `${{VAR}}=` in an .env has to be refused where it is made"
        ),
        Err(e) => e,
    };
    assert!(
        err.contains(key),
        "{what}: an empty `{key}` was rejected with {err:?}, which does not name the key — the \
         operator has to be told which variable to fill"
    );
}

#[test]
fn an_empty_mcp_endpoint_is_refused_by_name() {
    let raw = json!({"endpoint": ""});
    assert_rejected_like_absent(McpParams::parse(&raw).map(|_| ()), "endpoint", "mcp/http");
}

#[test]
fn an_empty_mcp_command_is_refused_by_name() {
    let raw = json!({"command": ""});
    assert_rejected_like_absent(McpParams::parse(&raw).map(|_| ()), "command", "mcp/stdio");
}

#[test]
fn an_empty_telegram_bot_token_is_refused_by_name() {
    let raw = json!({"bot_token": "", "emit_to": "/worker"});
    assert_rejected_like_absent(
        ProxyParams::parse(&raw).map(|_| ()),
        "bot_token",
        "proxy/telegram",
    );
}

#[test]
fn an_empty_slack_app_token_is_refused_by_name() {
    let raw = json!({
        "app_token": "", "bot_token": "xoxb-placeholder", "emit_to": "/worker"
    });
    assert_rejected_like_absent(
        SlackParams::parse(&raw).map(|_| ()),
        "app_token",
        "proxy/slack",
    );
}

#[test]
fn an_empty_slack_bot_token_is_refused_by_name() {
    let raw = json!({
        "app_token": "xapp-placeholder", "bot_token": "", "emit_to": "/worker"
    });
    assert_rejected_like_absent(
        SlackParams::parse(&raw).map(|_| ()),
        "bot_token",
        "proxy/slack",
    );
}

/// The guard on the guard: a configured value still has to pass. Without this
/// the four checks above would be satisfied by a parser that rejects
/// everything.
#[test]
fn configured_values_still_parse() {
    assert!(McpParams::parse(&json!({"endpoint": "http://127.0.0.1:9000/rpc"})).is_ok());
    assert!(McpParams::parse(&json!({"command": "mcp-server"})).is_ok());
    assert!(
        ProxyParams::parse(&json!({"bot_token": "123:placeholder", "emit_to": "/worker"})).is_ok()
    );
    assert!(
        SlackParams::parse(&json!({
            "app_token": "xapp-placeholder",
            "bot_token": "xoxb-placeholder",
            "emit_to": "/worker"
        }))
        .is_ok()
    );
}
