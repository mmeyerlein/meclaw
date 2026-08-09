//! P10 steps E1–E3 + F1/F2 — the subscription lane end to end.
//!
//! Drives a real `LlmCell` with `auth: "oauth_subscription"` against a fake
//! Responses endpoint and a fake token endpoint, and pins:
//! - the request actually goes to `/responses` with the reference header set,
//! - 401 → refresh → **exactly one** retry, then a typed error,
//! - the P10 error taxonomy reaches `meta.error.kind`,
//! - **no token value ever appears** in the emitted message, in cell.db, or in
//!   an error text (secret-hygiene audit, plan design-gate 3).

#[path = "mock_oauth.rs"]
mod mock_oauth;
#[path = "mock_responses.rs"]
mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_oauth::{MockOauth, write_token_store};
use mock_responses::{
    MockResponses, canned_overloaded, canned_quota_exhausted, canned_sse_text, canned_unauthorized,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The access token the store starts with; must never surface anywhere.
const INITIAL_ACCESS: &str = "access-dummy-0";

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

struct Rig {
    cell: LlmCell,
    db: DbConn,
    _td: TempDir,
}

fn mk_oauth_cell(responses_base: &str, token_endpoint: &str, store: &std::path::Path) -> Rig {
    let td = TempDir::new().unwrap();
    let raw = json!({
        "provider": "openai",
        "model": "gpt-5-sub",
        "auth": "oauth_subscription",
        "auth_ref": store.to_str().unwrap(),
        "base_url": responses_base,
        "oauth_token_endpoint": token_endpoint,
        "oauth_client_id": "client-test",
        "max_tokens": 64,
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let cell = LlmCell::new(params, http);
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    Rig {
        cell,
        db: DbConn::wrap(conn, None),
        _td: td,
    }
}

fn inference_msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "hi"}]
        })))
        .build()
}

async fn emitted(rx: &mut mpsc::Receiver<CellEmission>) -> Value {
    rx.recv().await.expect("cell emitted nothing").content
}

// ───── E1: the request lands on /responses with the pinned headers ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oauth_cell_calls_responses_endpoint_with_reference_headers() {
    let api = MockResponses::start(vec![canned_sse_text("hello", "gpt-5-actual")]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    let reqs = api.recorded().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/responses");
    assert_eq!(
        reqs[0].header("authorization"),
        Some(&format!("Bearer {INITIAL_ACCESS}")[..])
    );
    assert_eq!(reqs[0].header("chatgpt-account-id"), Some("acct-dummy"));
    assert_eq!(reqs[0].header("originator"), Some("codex_cli_rs"));
    assert_eq!(reqs[0].header("accept"), Some("text/event-stream"));
    assert!(reqs[0].header("session-id").is_some(), "session-id missing");
    assert!(
        reqs[0]
            .header("user-agent")
            .unwrap()
            .starts_with("codex_cli_rs/"),
        "user-agent: {:?}",
        reqs[0].header("user-agent")
    );
    // body is the Responses shape, not chat-completions
    assert_eq!(reqs[0].body["store"], false);
    assert_eq!(reqs[0].body["stream"], true);
    assert!(reqs[0].body.get("messages").is_none());
    assert_eq!(reqs[0].body["input"][0]["content"][0]["type"], "input_text");

    // response.model receipt reaches the emitted header
    let out = emitted(&mut rx).await;
    let content = &out;
    assert_eq!(content["header"]["model"], "gpt-5-actual");
    assert_eq!(content["header"]["finish_reason"], "stop");
    assert_eq!(content["messages"][0]["text"], "hello");
    assert_eq!(oauth.refresh_count().await, 0, "no refresh needed");
}

// ───── E2: 401 → refresh → exactly one retry ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_refreshes_once_and_retries_on_401() {
    let api = MockResponses::start(vec![
        canned_unauthorized(),
        canned_sse_text("after refresh", "gpt-5-actual"),
    ])
    .await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    assert_eq!(api.call_count().await, 2, "one call + exactly one retry");
    assert_eq!(oauth.refresh_count().await, 1, "exactly one refresh");

    let reqs = api.recorded().await;
    assert_eq!(
        reqs[0].header("authorization"),
        Some(&format!("Bearer {INITIAL_ACCESS}")[..])
    );
    assert_eq!(
        reqs[1].header("authorization"),
        Some("Bearer access-1"),
        "retry must use the refreshed token"
    );

    let out = emitted(&mut rx).await;
    assert_eq!(out["header"]["finish_reason"], "stop");
    assert_eq!(out["messages"][0]["text"], "after refresh");
}

// ───── E3: give up after one retry ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_gives_up_after_one_retry() {
    let api = MockResponses::start(vec![canned_unauthorized(), canned_unauthorized()]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    assert_eq!(
        api.call_count().await,
        2,
        "no third attempt — one retry only"
    );
    assert_eq!(oauth.refresh_count().await, 1);

    let out = emitted(&mut rx).await;
    let content = &out;
    assert_eq!(content["header"]["finish_reason"], "error");
    assert_eq!(content["header"]["error_code"], "auth");
    assert_eq!(content["meta"]["error"]["kind"], "auth_expired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_refresh_failure_surfaces_re_login_required() {
    let api = MockResponses::start(vec![canned_unauthorized()]).await;
    let oauth = MockOauth::start_permanent_failure("refresh_token_expired").await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    assert_eq!(api.call_count().await, 1, "no retry without a fresh token");
    let out = emitted(&mut rx).await;
    assert_eq!(out["header"]["error_code"], "auth");
    assert_eq!(out["meta"]["error"]["kind"], "auth_permanent");
    assert_eq!(out["meta"]["error"]["re_login_required"], true);
}

// ───── taxonomy on the wire ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quota_exhaustion_is_a_typed_error_with_reset_time() {
    let api = MockResponses::start(vec![canned_quota_exhausted(1786000000, "plus")]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    let out = emitted(&mut rx).await;
    let content = &out;
    // spec enum stays closed …
    assert_eq!(content["header"]["error_code"], "rate_limit");
    // … the discriminator a failover edge needs lives in meta.
    assert_eq!(content["meta"]["error"]["kind"], "quota_exhausted");
    assert_eq!(content["meta"]["error"]["resets_at"], 1786000000i64);
    assert_eq!(content["meta"]["error"]["plan_type"], "plus");
    // failover is TOPOLOGY: the cell must not have retried anywhere.
    assert_eq!(api.call_count().await, 1);
    // and the input messages come back untouched (cell-types Z.166)
    assert_eq!(content["messages"][0]["text"], "hi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overload_is_transient_and_not_retried_by_the_cell() {
    let api = MockResponses::start(vec![canned_overloaded()]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    let out = emitted(&mut rx).await;
    assert_eq!(out["header"]["error_code"], "provider_error");
    assert_eq!(out["meta"]["error"]["kind"], "transient");
    assert_eq!(api.call_count().await, 1, "retry is the topology's call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_token_store_fails_without_calling_the_provider() {
    let api = MockResponses::start(vec![canned_sse_text("unused", "m")]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let absent = td.path().join("absent.json");
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &absent);
    let (sink, mut rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    assert_eq!(
        api.call_count().await,
        0,
        "no provider call without a token"
    );
    let out = emitted(&mut rx).await;
    assert_eq!(out["header"]["error_code"], "auth");
    assert_eq!(out["meta"]["error"]["kind"], "auth_store_unavailable");
}

// ───── the api_key + responses lane (what the paid smoke exercises) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_key_responses_lane_sends_no_account_header_and_no_include() {
    let api = MockResponses::start(vec![canned_sse_text("metered", "gpt-5-metered")]).await;
    let td = TempDir::new().unwrap();
    let raw = json!({
        "provider": "openai", "model": "gpt-5", "api_key": "sk-test-key",
        "wire_dialect": "responses", "base_url": api.base_url, "max_tokens": 32,
    });
    let params = LlmParams::parse(&raw).unwrap();
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();

    cell.handle(inference_msg(), &sink, &mut db).await;

    let reqs = api.recorded().await;
    assert_eq!(reqs[0].path, "/responses");
    assert_eq!(reqs[0].header("authorization"), Some("Bearer sk-test-key"));
    assert!(
        reqs[0].header("chatgpt-account-id").is_none(),
        "the metered lane has no account id"
    );
    assert!(
        reqs[0].body.get("include").is_none(),
        "reasoning include is subscription-only"
    );
    assert_eq!(reqs[0].body["store"], false);
    let out = emitted(&mut rx).await;
    assert_eq!(out["header"]["model"], "gpt-5-metered");
}

// ───── F1/F2: secret-hygiene audit ─────

/// Every emitted surface is scanned for both token values. This is the audit
/// the plan's design-gate 3 demands, on the success path AND on each failure
/// path (an error text is the classic place a credential leaks).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_token_ever_appears_in_an_emitted_message() {
    let cases: Vec<(&str, Vec<meclaw_testing::mock_http::MockResponse>)> = vec![
        ("success", vec![canned_sse_text("ok", "m")]),
        ("quota", vec![canned_quota_exhausted(1, "plus")]),
        ("overload", vec![canned_overloaded()]),
        (
            "auth_expired",
            vec![canned_unauthorized(), canned_unauthorized()],
        ),
    ];
    for (name, responses) in cases {
        let api = MockResponses::start(responses).await;
        let oauth = MockOauth::start_rotating(None).await;
        let td = TempDir::new().unwrap();
        let store = write_token_store(td.path(), "refresh-dummy-0");
        let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
        let (sink, mut rx) = mk_sink();

        rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

        let out = emitted(&mut rx).await;
        let dump = meclaw_core::serde_json::to_string(&out).unwrap();
        for secret in [INITIAL_ACCESS, "refresh-dummy-0", "access-1", "refresh-1"] {
            assert!(
                !dump.contains(secret),
                "case {name}: secret {secret} leaked into the emitted message: {dump}"
            );
        }
        assert!(
            !dump.contains("Bearer "),
            "case {name}: bearer leaked: {dump}"
        );
    }
}

/// The cell persists `system.*` and the last input in cell.db. A token must
/// never end up there — nor in a params overlay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_token_is_persisted_in_cell_db() {
    let api = MockResponses::start(vec![canned_sse_text("ok", "m")]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let dbdir = TempDir::new().unwrap();
    let dbpath = dbdir.path().join("cell.db");

    let raw = json!({
        "provider": "openai", "model": "gpt-5-sub", "auth": "oauth_subscription",
        "auth_ref": store.to_str().unwrap(), "base_url": api.base_url,
        "oauth_token_endpoint": oauth.token_endpoint, "oauth_client_id": "client-test",
    });
    let params = LlmParams::parse(&raw).unwrap();
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dbpath).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, _rx) = mk_sink();
    cell.handle(inference_msg(), &sink, &mut db).await;
    drop(db);

    let bytes = std::fs::read(&dbpath).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for secret in [INITIAL_ACCESS, "refresh-dummy-0", "access-1"] {
        assert!(!text.contains(secret), "secret {secret} reached cell.db");
    }
}

/// The token store itself must stay owner-readable after the cell rotated it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rotated_store_keeps_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let api = MockResponses::start(vec![canned_unauthorized(), canned_sse_text("ok", "m")]).await;
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut rig = mk_oauth_cell(&api.base_url, &oauth.token_endpoint, &store);
    let (sink, _rx) = mk_sink();

    rig.cell.handle(inference_msg(), &sink, &mut rig.db).await;

    let mode = std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "store must be owner-only after rotation");
}
