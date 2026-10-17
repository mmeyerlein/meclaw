//! GH #1235 (T6-22) — a secret param takes no `${VAR}`: the secret comes
//! sealed from the vault under a credential grant, and `.env` carries only
//! what is not a secret.
//!
//! Every cell type below has a grant key next to its secret (GH #1059, #1060,
//! #1106), the `meclaw` peer proxy's `auth` included: `client_secret` next to
//! `client_secret_grant_id` (#1105) and the static `value` next to
//! `value_grant_id` (OR-programm-tm.H5.orch-3). A `${VAR}` in the secret key itself bound the secret from
//! the process environment, which every cell of the colony process can read;
//! the grant hands it only to the cell it is granted to. The refusal runs on
//! the params as the file declares them (`validate_declared_params`), names
//! the key and never echoes a value. The app-id params (`x_title`,
//! `http_referer`) are no secrets and keep their environment default.

use meclaw_cells::{
    LlmCellFactory, McpCellFactory, ProxyCellFactory, VoiceCellFactory, WebSearchCellFactory,
};
use meclaw_colony::{CellFactory, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;

/// The variable name of the one `${VAR}` / `${VAR:-default}` leaf in `v`.
fn env_var_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => s
            .strip_prefix("${")
            .and_then(|rest| rest.split([':', '}']).next())
            .map(str::to_string),
        Value::Object(m) => m.values().find_map(env_var_of),
        Value::Array(a) => a.iter().find_map(env_var_of),
        _ => None,
    }
}

#[test]
fn every_secret_param_refuses_an_environment_token() {
    let proxy = ProxyCellFactory::new(Arc::new(SurfaceRegistry::new()));
    let voice = VoiceCellFactory::new(Arc::new(SurfaceRegistry::new()));
    let cases: Vec<(&str, &dyn CellFactory, Value)> = vec![
        (
            "api_key",
            &LlmCellFactory,
            json!({"provider": "openai", "model": "m", "api_key": "${OPENROUTER_API_KEY}"}),
        ),
        (
            "api_key",
            &WebSearchCellFactory,
            json!({"api_key": "${BRAVE_API_KEY}"}),
        ),
        (
            "duplex.api_key",
            &voice,
            json!({"duplex": {"provider": "gpt_live", "api_key": "${OPENAI_API_KEY}"}}),
        ),
        (
            "bot_token",
            &proxy,
            json!({"bot_token": "${TELEGRAM_BOT_TOKEN}", "emit_to": "/a"}),
        ),
        (
            "app_token",
            &proxy,
            json!({"platform": "slack", "app_token": "${SLACK_APP_TOKEN}", "bot_token_grant_id": "g"}),
        ),
        (
            "verify.secret",
            &proxy,
            json!({"platform": "webhook", "verify": {"kind": "token", "secret": "${HOOK_SECRET}"}}),
        ),
        (
            "auth.client_secret",
            &proxy,
            json!({"platform": "meclaw", "auth": {"token_url": "https://idp.example/token",
                   "client_id": "north", "client_secret": "${PEER_CLIENT_SECRET}"}}),
        ),
        (
            "auth.value",
            &proxy,
            json!({"platform": "meclaw",
                   "auth": {"header": "X-Peer-Credential", "value": "${PEER_HEADER_SECRET}"}}),
        ),
        (
            "auth.bearer",
            &McpCellFactory,
            json!({"endpoint": "http://127.0.0.1:9000/rpc",
                   "auth": {"bearer": "${MCP_BEARER_TOKEN:-}"}}),
        ),
    ];
    for (key, factory, declared) in cases {
        let err = factory
            .validate_declared_params(&declared)
            .expect_err(&format!("{key}: a ${{VAR}} secret must be refused"));
        assert!(err.contains(key), "the refusal names the key {key}: {err}");
        let var = env_var_of(&declared).expect("each case declares one ${VAR} token");
        assert!(
            !err.contains(&var),
            "{key}: the refusal echoes no value ({var}): {err}"
        );
    }

    // The grant is the way, and the app id keeps its environment default
    // (`AGENTS.md` rule 15): neither is a secret.
    LlmCellFactory
        .validate_declared_params(&json!({
            "provider": "openai", "model": "m", "credential_grant_id": "grant:llm@a/b",
            "x_title": "${OPENROUTER_X_TITLE:-MeClaw}",
            "http_referer": "${OPENROUTER_HTTP_REFERER:-https://meclaw.ai}"
        }))
        .expect("a grant and the app-id params pass");
    // Both forms of the peer proxy's credential have their grant.
    for auth in [
        json!({"token_url": "https://idp.example/token", "client_id": "north",
               "client_secret_grant_id": "grant:peer@a/friend"}),
        json!({"header": "X-Peer-Credential", "value_grant_id": "grant:peer@a/friend"}),
    ] {
        proxy
            .validate_declared_params(&json!({"platform": "meclaw", "auth": auth}))
            .expect("a peer credential under a grant passes");
    }
    // The mcp bearer has its grant too (OR-programm-tm.H5.orch-4).
    let mcp = json!({"endpoint": "http://127.0.0.1:9000/rpc",
                     "auth": {"bearer_grant_id": "grant:mcp@a/tools"}});
    McpCellFactory
        .validate_declared_params(&mcp)
        .expect("an mcp bearer under a grant passes");
    McpCellFactory
        .validate_params(&mcp)
        .expect("an mcp bearer under a grant parses");
}
