//! GH #268 — the reference `mcp` config must put its token where the cell
//! looks for it, and the proof is the header on the wire.
//!
//! `templates/_cell-types/mcp-min/config.json` declared the token as
//! `params.bearer`. `McpParams::parse` reads it from `params.auth.bearer`
//! (`crates/meclaw-cells/src/mcp/params.rs`, `parse_http`) — the nested shape
//! `docs/cell-types.md` § `mcp` calls "`endpoint` + `auth` (Bearer)" and the
//! one `McpOverlay::IMMUTABLE_KEYS` names. So `MCP_BEARER` resolved, was
//! stored, and was never read: no `Authorization` header went out, and nothing
//! complained, because an unread `params` key is not an error anywhere in the
//! substrate.
//!
//! # Why this file goes all the way to the socket
//!
//! Asserting that the param is present is what passed on the broken config —
//! it was present, under the wrong name. Asserting that `McpParams::parse`
//! returns `Some(bearer)` would be one step better and still stops short of
//! the claim the template makes to its reader ("set `MCP_BEARER` and the
//! provider sees a bearer token"). So the chain here is the shipped one end to
//! end and nothing in it is re-implemented:
//!
//! 1. the bytes of the shipped `config.json`,
//! 2. `substitute_env_only` — the colony's own late-binding pass, the same
//!    call `plan_bootstrap` makes,
//! 3. `McpParams::parse` — the factory's parser,
//! 4. `McpClient` + a real `initialize` against a mock provider that records
//!    what it received.
//!
//! The assertion is on the recorded request header. A future edit that moves
//! the key back to the top level, renames it, or drops the `auth` block
//! fails here with the header missing, which is exactly the symptom an
//! operator would otherwise meet as a 401 from a third party.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::params::{McpParams, McpTransport};
use meclaw_cells::mcp::wire::McpClient;
use mock_mcp::{MockMcp, canned_initialize};
use std::collections::HashMap;
use std::time::Duration;

/// The repository root, from the crate this test lives in.
fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const CONFIG: &str = "templates/_cell-types/mcp-min/config.json";

/// Whether the reference config travels with this checkout (GH #49 form):
/// `_cell-types` is not in `PUBLIC_TEMPLATES`, so in a public clone both tests
/// skip instead of failing on a dead `templates/` reference.
fn shipped() -> Option<std::path::PathBuf> {
    let path = core_root().join(CONFIG);
    path.is_file().then_some(path)
}

/// The env var the shipped config binds the token to. Named here so the
/// failure message can say which knob an operator turned in vain.
const TOKEN_VAR: &str = "MCP_BEARER";
const ENDPOINT_VAR: &str = "MCP_ENDPOINT";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_token_the_reference_config_declares_arrives_as_an_authorization_header() {
    let Some(path) = shipped() else {
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("the reference mcp config is on disk");
    let cfg: meclaw_core::JsonValue =
        meclaw_core::serde_json::from_str(&raw).expect("the reference mcp config parses as JSON");

    let server = MockMcp::start(vec![canned_initialize()]).await;
    let token = "gh268-token";

    // The late binding the colony performs at boot, not a stand-in for it.
    let env: HashMap<String, String> = HashMap::from([
        (ENDPOINT_VAR.to_string(), server.endpoint()),
        (TOKEN_VAR.to_string(), token.to_string()),
    ]);
    let resolved = meclaw_colony::mutation::substitute::substitute_env_only(&cfg, &env)
        .expect("the reference config substitutes");
    let params = resolved
        .get("params")
        .expect("the reference config carries params");

    let parsed = McpParams::parse(params).expect("the reference params parse");
    let McpTransport::Http { endpoint, bearer } = &parsed.transport else {
        panic!("the reference config wires the http transport");
    };
    assert_eq!(endpoint, &server.endpoint());

    let client = McpClient::new(endpoint, bearer.clone()).expect("client builds");
    client
        .initialize(Duration::from_secs(2))
        .await
        .expect("the mock provider answers initialize");

    let seen = server.recorded_requests().await;
    let first = seen.first().expect("the provider recorded the call");
    let auth = first.headers.get("authorization").map(String::as_str);
    assert_eq!(
        auth,
        Some(format!("Bearer {token}").as_str()),
        "{CONFIG} resolved {TOKEN_VAR} and the provider still saw {auth:?}: the cell reads the \
         token from params.auth.bearer (mcp/params.rs, parse_http), so a token declared anywhere \
         else is dropped without a word"
    );
}

/// The other half of the same declaration: `contract.settings.auth` says empty
/// or absent means no `Authorization` header, and the shipped params write the
/// token as `${MCP_BEARER:-}`, which the colony's substitution turns into `""`
/// for every operator who never set the variable. If that empty string became
/// a token, the reference config would send `Authorization: Bearer ` to a
/// provider that would have answered anonymously — the repair for #268 would
/// have traded a silent no-auth for a silent empty-auth.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unset_token_variable_sends_no_authorization_header_at_all() {
    let Some(path) = shipped() else {
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("the reference mcp config is on disk");
    let cfg: meclaw_core::JsonValue =
        meclaw_core::serde_json::from_str(&raw).expect("the reference mcp config parses as JSON");

    let server = MockMcp::start(vec![canned_initialize()]).await;
    // Only the endpoint is bound — `MCP_BEARER` is exactly as unset as it is on
    // a fresh install.
    let env: HashMap<String, String> =
        HashMap::from([(ENDPOINT_VAR.to_string(), server.endpoint())]);
    let resolved = meclaw_colony::mutation::substitute::substitute_env_only(&cfg, &env)
        .expect("the reference config substitutes");
    let params = resolved
        .get("params")
        .expect("the reference config carries params");

    let parsed = McpParams::parse(params).expect("the reference params parse");
    let McpTransport::Http { endpoint, bearer } = &parsed.transport else {
        panic!("the reference config wires the http transport");
    };
    assert_eq!(bearer.as_deref(), None, "an empty token is no token");

    let client = McpClient::new(endpoint, bearer.clone()).expect("client builds");
    client
        .initialize(Duration::from_secs(2))
        .await
        .expect("the mock provider answers initialize");

    let seen = server.recorded_requests().await;
    let first = seen.first().expect("the provider recorded the call");
    assert_eq!(
        first.headers.get("authorization"),
        None,
        "{CONFIG} sent an Authorization header although {TOKEN_VAR} was never set"
    );
}
