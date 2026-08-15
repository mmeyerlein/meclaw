//! P10 steps B3–B6 — the token broker's refresh contract and its
//! single-refresher guarantee.
//!
//! The load-bearing test here is
//! `concurrent_stale_gets_trigger_exactly_one_refresh`: the OAuth refresh token
//! rotates, so a second concurrent refresh would answer `refresh_token_reused`
//! and permanently kill the login. Proving "exactly one network refresh" is the
//! whole point of the broker actor.

#[path = "mock_oauth.rs"]
mod mock_oauth;

use meclaw_cells::llm::auth::AuthError;
use meclaw_cells::llm::token_broker::get_token;
use mock_oauth::{MockOauth, write_token_store};
use std::time::Duration;
use tempfile::TempDir;

const CLIENT_ID: &str = "client-test";

// ───── B5: plain get, no refresh ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_without_seen_generation_reads_the_store_and_does_not_refresh() {
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");

    let snap = get_token(
        store.to_str().unwrap(),
        &oauth.token_endpoint,
        CLIENT_ID,
        None,
    )
    .await
    .unwrap();

    assert_eq!(snap.access_token, "access-dummy-0");
    assert_eq!(snap.account_id.as_deref(), Some("acct-dummy"));
    assert_eq!(snap.generation, 0);
    assert_eq!(oauth.refresh_count().await, 0, "no refresh must happen");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_store_is_reported_without_touching_the_network() {
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let missing = td.path().join("absent.json");

    let err = get_token(
        missing.to_str().unwrap(),
        &oauth.token_endpoint,
        CLIENT_ID,
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, AuthError::StoreUnavailable(_)), "{err:?}");
    assert_eq!(oauth.refresh_count().await, 0);
}

// ───── B3/B4: refresh request form + error classification ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_generation_refreshes_and_persists_the_rotated_token() {
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let path = store.to_str().unwrap();

    let first = get_token(path, &oauth.token_endpoint, CLIENT_ID, None)
        .await
        .unwrap();
    let refreshed = get_token(
        path,
        &oauth.token_endpoint,
        CLIENT_ID,
        Some(first.generation),
    )
    .await
    .unwrap();

    assert_eq!(refreshed.access_token, "access-1");
    assert_eq!(refreshed.generation, 1, "generation must advance");
    assert_eq!(oauth.refresh_count().await, 1);

    // request form is the pinned one: JSON, with client_id + grant_type.
    let bodies = oauth.refresh_bodies().await;
    assert_eq!(bodies[0]["grant_type"], "refresh_token");
    assert_eq!(bodies[0]["client_id"], CLIENT_ID);
    assert_eq!(bodies[0]["refresh_token"], "refresh-dummy-0");

    // the ROTATED refresh token must be persisted, or the next rotation dies.
    let back: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
    assert_eq!(back["tokens"]["refresh_token"], "refresh-1");
    assert_eq!(back["tokens"]["access_token"], "access-1");
    // and the store's foreign fields survived the rotation.
    assert_eq!(back["auth_mode"], "chatgpt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_refresh_failure_is_typed_permanent() {
    let oauth = MockOauth::start_permanent_failure("refresh_token_reused").await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let path = store.to_str().unwrap();

    let first = get_token(path, &oauth.token_endpoint, CLIENT_ID, None)
        .await
        .unwrap();
    let err = get_token(
        path,
        &oauth.token_endpoint,
        CLIENT_ID,
        Some(first.generation),
    )
    .await
    .unwrap_err();

    match err {
        AuthError::RefreshPermanent(code) => assert_eq!(code, "refresh_token_reused"),
        other => panic!("expected permanent, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transient_refresh_failure_is_typed_transient() {
    let oauth = MockOauth::start_transient_failure().await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let path = store.to_str().unwrap();

    let first = get_token(path, &oauth.token_endpoint, CLIENT_ID, None)
        .await
        .unwrap();
    let err = get_token(
        path,
        &oauth.token_endpoint,
        CLIENT_ID,
        Some(first.generation),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, AuthError::RefreshTransient(_)), "{err:?}");
}

// ───── B6: the single-refresher proof ─────

/// Four cells hit 401 at the same instant against one store. Exactly ONE
/// refresh may reach the endpoint; all four must end up with the same fresh
/// token. A second refresh would send the already-rotated refresh token and
/// earn `refresh_token_reused` from the real backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_stale_gets_trigger_exactly_one_refresh() {
    // The delay makes the overlap real: without it the four requests could
    // serialize by luck and the test would prove nothing.
    let oauth = MockOauth::start_rotating(Some(Duration::from_millis(150))).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let path = store.to_str().unwrap().to_string();

    // All four observed the same (stale) generation 0.
    let first = get_token(&path, &oauth.token_endpoint, CLIENT_ID, None)
        .await
        .unwrap();
    assert_eq!(first.generation, 0);

    let mut handles = Vec::new();
    for _ in 0..4 {
        let p = path.clone();
        let ep = oauth.token_endpoint.clone();
        handles.push(tokio::spawn(async move {
            get_token(&p, &ep, CLIENT_ID, Some(0)).await
        }));
    }
    let mut tokens = Vec::new();
    for h in handles {
        tokens.push(h.await.unwrap().unwrap());
    }

    assert_eq!(
        oauth.refresh_count().await,
        1,
        "exactly one refresh may reach the endpoint (rotation would break otherwise)"
    );
    for t in &tokens {
        assert_eq!(
            t.access_token, "access-1",
            "every caller gets the fresh token"
        );
        assert_eq!(t.generation, 1);
    }
}

/// The dedup rule in isolation: a caller whose `seen_generation` is already
/// behind the cache gets the newer token handed to it, without a second
/// refresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_generation_after_someone_else_refreshed_does_not_refresh_again() {
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    let store = write_token_store(td.path(), "refresh-dummy-0");
    let path = store.to_str().unwrap();

    get_token(path, &oauth.token_endpoint, CLIENT_ID, None)
        .await
        .unwrap();
    // first refresher moves generation 0 → 1
    let fresh = get_token(path, &oauth.token_endpoint, CLIENT_ID, Some(0))
        .await
        .unwrap();
    assert_eq!(fresh.generation, 1);
    assert_eq!(oauth.refresh_count().await, 1);

    // a straggler still holding generation 0 must NOT cause a second refresh
    let straggler = get_token(path, &oauth.token_endpoint, CLIENT_ID, Some(0))
        .await
        .unwrap();
    assert_eq!(straggler.access_token, "access-1");
    assert_eq!(straggler.generation, 1);
    assert_eq!(
        oauth.refresh_count().await,
        1,
        "the straggler must be served from cache, not by refreshing again"
    );

    // …whereas a caller that saw the CURRENT generation does refresh.
    let next = get_token(path, &oauth.token_endpoint, CLIENT_ID, Some(1))
        .await
        .unwrap();
    assert_eq!(next.generation, 2);
    assert_eq!(oauth.refresh_count().await, 2);
}

/// Two different stores must not share a refresh lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_stores_are_independent() {
    let oauth = MockOauth::start_rotating(None).await;
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("a")).unwrap();
    std::fs::create_dir_all(td.path().join("b")).unwrap();
    let a = write_token_store(&td.path().join("a"), "refresh-a-0");
    let b = write_token_store(&td.path().join("b"), "refresh-b-0");

    for p in [&a, &b] {
        let s = p.to_str().unwrap();
        get_token(s, &oauth.token_endpoint, CLIENT_ID, None)
            .await
            .unwrap();
        let r = get_token(s, &oauth.token_endpoint, CLIENT_ID, Some(0))
            .await
            .unwrap();
        assert_eq!(r.generation, 1, "each store has its own generation");
    }
    assert_eq!(oauth.refresh_count().await, 2, "one refresh per store");
}
