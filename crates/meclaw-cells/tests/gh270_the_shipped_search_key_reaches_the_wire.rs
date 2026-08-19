//! GH #270 — an empty bearer is not a bearer, and the proof is the header the
//! endpoint records.
//!
//! `templates/_cell-types/web_search-min/config.json` declares the credential
//! as `"api_key": "${SEARCH_API_KEY:-}"`, and its `contract.settings` calls it
//! an *optional* bearer token with the default `""`. The shipped `.env.example`
//! goes one step further and ships the variable **set to empty**, with the
//! sentence "SEARCH_API_KEY is optional and may be left empty for an
//! unauthenticated local SearXNG instance" right above it.
//!
//! `parse_params_pure` (`crates/meclaw-cells/src/web_search.rs`) took that
//! value without an emptiness filter, so the cell held `Some("")` and every
//! search went out carrying `Authorization: Bearer ` with nothing after it —
//! `Some("")` is not `None`. Against an endpoint that would have answered
//! anonymously that header can be a flat rejection, and the rejection looks
//! like a search backend that is down rather than like a credential that was
//! never configured. #268 repaired the same defect one cell over, in `mcp`.
//!
//! # Why this file goes all the way to the socket
//!
//! Asserting that the param is empty is what passes on the broken code — it
//! *was* empty, and was sent anyway. Asserting that the parsed `api_key` is
//! `None` would be one step better and still stops short of the claim the
//! declaration makes to its reader ("leave it empty and nothing is sent"). So
//! the chain here is the shipped one end to end and nothing in it is
//! re-implemented:
//!
//! 1. the bytes of the shipped `config.json`,
//! 2. `substitute_env_only` — the colony's own late-binding pass, the same call
//!    `plan_bootstrap` makes,
//! 3. the colony instantiating that config through `add_nodes`,
//! 4. `WebSearchCellFactory` parsing the params and the live cell answering a
//!    real `tool_call` against a mock endpoint that records what it received.
//!
//! The assertion is on the recorded request header, in both directions: a key
//! that is set has to arrive, a key that is not set must produce no header at
//! all. The first half keeps a future filter from swallowing a real token; the
//! second half is the defect.

use meclaw_cells::WebSearchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::HashMap;
use std::sync::Arc;

/// The repository root, from the crate this test lives in.
fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const TEMPLATE_DIR: &str = "templates/_cell-types/web_search-min";
const CONFIG: &str = "templates/_cell-types/web_search-min/config.json";

/// The env vars the shipped config binds its two settings to. Named here so a
/// failure message can say which knob an operator turned in vain.
const ENDPOINT_VAR: &str = "SEARCH_ENDPOINT";
const KEY_VAR: &str = "SEARCH_API_KEY";

/// Whether the reference config travels with this checkout (GH #49 form):
/// `_cell-types` is not in `PUBLIC_TEMPLATES`, so in a public clone both tests
/// skip instead of failing on a dead `templates/` reference.
fn shipped() -> Option<std::path::PathBuf> {
    let path = core_root().join(TEMPLATE_DIR);
    path.join("config.json").is_file().then_some(path)
}

/// Materialise the shipped template into `td`, with `${…}` already resolved
/// against `env` — the substitution the colony performs at boot, run here so
/// the mock's port can reach `endpoint` without inventing a second code path.
fn install_resolved_template(
    td: &tempfile::TempDir,
    shipped_dir: &std::path::Path,
    env: &HashMap<String, String>,
) -> std::path::PathBuf {
    let raw = std::fs::read_to_string(shipped_dir.join("config.json"))
        .expect("the reference web_search config is on disk");
    let cfg: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&raw)
        .expect("the reference web_search config parses as JSON");
    let resolved = meclaw_colony::mutation::substitute::substitute_env_only(&cfg, env)
        .expect("the reference config substitutes");

    let templates_root = td.path().join("templates");
    let tpl = templates_root.join("web_search-min");
    std::fs::create_dir_all(&tpl).expect("template dir");
    std::fs::copy(shipped_dir.join("template.json"), tpl.join("template.json"))
        .expect("the shipped descriptor travels along");
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_vec_pretty(&resolved).expect("serialise"),
    )
    .expect("resolved config");
    templates_root
}

/// Boot a colony carrying the resolved template, instantiate it as `/search`,
/// send one `tool_call`, and hand back what the endpoint recorded.
///
/// `override_params` is deliberately absent from the mutation: the point of
/// the exercise is what the SHIPPED params do, so nothing here may re-state
/// them.
async fn search_once_and_record(
    env: HashMap<String, String>,
    endpoint_for_env: impl FnOnce(&std::net::SocketAddr) -> String,
) -> Vec<meclaw_testing::mock_http::CapturedRequest> {
    let shipped_dir = shipped().expect("caller checked");
    let td = tempfile::TempDir::new().expect("tempdir");
    let body = br#"{"results":[{"title":"A","url":"u1","snippet":"s1"}]}"#;
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(body)]).await;

    let mut env = env;
    env.insert(ENDPOINT_VAR.to_string(), endpoint_for_env(&addr));
    let templates_root = install_resolved_template(&td, &shipped_dir, &env);

    let factory: Arc<dyn CellFactory> = Arc::new(WebSearchCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("web_search".to_string(), factory)],
    );
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx.await.expect("rescan acked");

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "search", "template": "web_search-min"}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    assert!(
        matches!(
            ack_rx.await.expect("mutation acked"),
            meclaw_colony::MutationOutcome::Committed { .. }
        ),
        "the shipped web_search skeleton must instantiate"
    );
    h.add_edge(Uuid::now_v7(), Path::new("/search"), Path::new("/sink"))
        .await;

    let probe = MessageBuilder::new(Path::new("/search"))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": r#"{"query":"gh270"}"#, "id": "call-gh270"
            }]
        })))
        .build();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: probe,
        })
        .await
        .expect("probe sent");

    // Positive receipt: the cell answered, so the request below is the request
    // the search actually made — not an empty capture list read too early.
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("the search cell answered within 30s")
        .expect("the sink is still open");
    assert_eq!(
        reply.headers.hop["operation"], "web_search",
        "the reply must come from the search cell"
    );

    let seen = captured.lock().await.clone();
    h.shutdown().await;
    seen
}

/// A configured key has to arrive. Without this half, "filter the empty
/// string" could quietly become "filter everything" and nothing would notice
/// until a provider answered 401.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_key_the_reference_config_declares_arrives_as_an_authorization_header() {
    if shipped().is_none() {
        return;
    }
    let token = "gh270-token";
    let env = HashMap::from([(KEY_VAR.to_string(), token.to_string())]);
    let seen = search_once_and_record(env, |addr| format!("http://{addr}/search")).await;

    let first = seen.first().expect("the endpoint recorded the search");
    let auth = first.headers.get("authorization").map(String::as_str);
    assert_eq!(
        auth,
        Some(format!("Bearer {token}").as_str()),
        "{CONFIG} resolved {KEY_VAR} and the endpoint still saw {auth:?}"
    );
}

/// The defect. `contract.settings.api_key` calls the token optional with the
/// default `""`, the shipped params write it as `${SEARCH_API_KEY:-}`, and
/// `.env.example` ships the variable set to empty with the sentence that it
/// "may be left empty for an unauthenticated local SearXNG instance". If that
/// empty string becomes a bearer, every one of those installations sends
/// `Authorization: Bearer ` to an endpoint that would have answered
/// anonymously — and a rejection then reads as a broken search service rather
/// than as a credential nobody set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unset_search_key_sends_no_authorization_header_at_all() {
    if shipped().is_none() {
        return;
    }
    // Only the endpoint is bound — `SEARCH_API_KEY` is exactly as unset as it
    // is on a fresh install, and `${SEARCH_API_KEY:-}` falls back to `""`.
    let seen = search_once_and_record(HashMap::new(), |addr| format!("http://{addr}/search")).await;

    let first = seen.first().expect("the endpoint recorded the search");
    assert_eq!(
        first.headers.get("authorization"),
        None,
        "{CONFIG} sent an Authorization header although {KEY_VAR} was never set — an empty \
         bearer is not a bearer, and an endpoint that would have answered anonymously may \
         reject it outright"
    );
}
