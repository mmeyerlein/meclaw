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

// harness_fold: #[path = "mock_mcp.rs"]
#[allow(unused_imports)]
use crate::mock_mcp; // harness_fold: mod mock_mcp;
// harness_fold: #[path = "support/gh1059_grants.rs"]
#[allow(unused_imports)]
use crate::gh1059_grants; // harness_fold: mod gh1059_grants;

use gh1059_grants::{delivery, message};
use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::io::McpEvent;
use meclaw_cells::mcp::params::{McpParams, McpTransport};
use meclaw_cells::mcp::wire::McpClient;
use meclaw_cells::proxy::io::CredentialWait;
use meclaw_colony::LongRunningCell;
use meclaw_core::{Headers, OriginSink, OutputSink, Path, Uuid};
use mock_mcp::{MockMcp, canned_initialize, canned_tools_list};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

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

const ENDPOINT_VAR: &str = "MCP_ENDPOINT";

/// GH #1235 (OR-programm-tm.H5.orch-4): the reference config declares its token
/// as a grant, not as `${MCP_BEARER}` -- the colony's environment is readable by
/// every cell it spawns. The claim it makes to its reader is now "name a grant
/// in `auth.bearer_grant_id` and the provider sees the bearer the vault hands
/// over", so the chain is: the shipped bytes pass the boot's refusal, an
/// operator's grant parses as the cell's grant, the cell built from it asks
/// for the bearer in `on_start`, takes the vault's delivery (its `"sealed"`
/// box) in `handle`, and its I/O half runs the handshake only then, with the
/// bearer on the socket. No step of the chain is called by hand. The statement
/// is reversed by the breaking cut, not loosened: the header is still
/// asserted on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_token_the_reference_config_declares_arrives_as_an_authorization_header() {
    use meclaw_colony::CellFactory;
    let Some(path) = shipped() else {
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("the reference mcp config is on disk");
    let cfg: meclaw_core::JsonValue =
        meclaw_core::serde_json::from_str(&raw).expect("the reference mcp config parses as JSON");
    meclaw_cells::McpCellFactory
        .validate_declared_params(&cfg["params"])
        .expect("the reference config names no secret through the environment");

    let server = MockMcp::start(vec![canned_initialize(), canned_tools_list(vec![])]).await;
    let token = "gh268-token";

    // The late binding the colony performs at boot, not a stand-in for it.
    let env: HashMap<String, String> =
        HashMap::from([(ENDPOINT_VAR.to_string(), server.endpoint())]);
    let mut resolved = meclaw_colony::mutation::substitute::substitute_env_only(&cfg, &env)
        .expect("the reference config substitutes");
    // What an operator sets at instantiation.
    resolved["params"]["auth"]["bearer_grant_id"] =
        meclaw_core::serde_json::json!("grant:mcp@a/tools");
    let params = resolved
        .get("params")
        .expect("the reference config carries params");

    let parsed = McpParams::parse(params).expect("the reference params parse");
    assert_eq!(parsed.bearer_grant.as_deref(), Some("grant:mcp@a/tools"));
    let McpTransport::Http { endpoint, bearer } = &parsed.transport else {
        panic!("the reference config wires the http transport");
    };
    assert_eq!(endpoint, &server.endpoint());
    assert_eq!(
        bearer.as_deref(),
        None,
        "a granted cell is built without a token"
    );

    let grant = parsed.bearer_grant.as_deref().expect("the cell's grant");
    let client = McpClient::new(endpoint, bearer.clone()).expect("client builds");
    let product_wait = meclaw_testing::deadline::product_wait_ms();
    let wait = CredentialWait {
        wait_ms: product_wait,
        backoff_max_ms: product_wait,
    };
    let mut cell = McpCell::new(client, product_wait, product_wait, "mcp".into()).with_credential(
        Path::new("/mcp"),
        grant,
        wait,
    );
    let (ev_tx, mut ev_rx) = mpsc::channel(8);
    let (_rc_keep, rc_rx) = mpsc::channel(4);
    tokio::spawn(McpCell::run_io(cell.split_io(), ev_tx, rc_rx));

    let mut db =
        meclaw_colony::DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    let (tx, mut out) = mpsc::channel(8);
    let start_sink = OriginSink::new(tx, Path::new("/mcp"), 64);
    cell.on_start(&start_sink, &mut db).await;
    let request = meclaw_testing::deadline::hang("the bearer request of on_start", out.recv())
        .await
        .expect("a granted cell asks for its bearer")
        .content;

    let (htx, _hout) = mpsc::channel(8);
    let sink = OutputSink::new(
        htx,
        Path::new("/mcp"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    let (rc_tx, _rc_rx) = mpsc::channel(4);
    cell.handle(
        message("/mcp", delivery(grant, &request, token)),
        &sink,
        &mut db,
        &rc_tx,
    )
    .await;
    let ready = meclaw_testing::deadline::hang("the handshake after the box", ev_rx.recv()).await;
    assert!(
        matches!(ready, Some(McpEvent::DiscoveryReady { .. })),
        "the I/O half ran its handshake once the box opened"
    );

    let seen = server.recorded_requests().await;
    let first = seen.first().expect("the provider recorded the call");
    let auth = first.headers.get("authorization").map(String::as_str);
    assert_eq!(
        auth,
        Some(format!("Bearer {token}").as_str()),
        "{CONFIG}: the bearer the vault delivered never reached the provider, which saw {auth:?}"
    );
}

/// The other half of the same declaration: `contract.settings.auth` says empty
/// or absent means no `Authorization` header, and the shipped params name an
/// empty grant (GH #1235; until then `${MCP_BEARER:-}`, which the substitution
/// turned into `""`). If an empty value became a token, the reference config would send `Authorization: Bearer ` to a
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
    // Only the endpoint is bound, as on a fresh install.
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
        .initialize(Duration::from_millis(
            meclaw_testing::deadline::product_wait_ms(),
        ))
        .await
        .expect("the mock provider answers initialize");

    let seen = server.recorded_requests().await;
    let first = seen.first().expect("the provider recorded the call");
    assert_eq!(
        first.headers.get("authorization"),
        None,
        "{CONFIG} sent an Authorization header although no grant was named"
    );
}
