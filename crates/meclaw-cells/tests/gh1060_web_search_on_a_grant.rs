//! GH #1060 (Vault-Grants, strand V3): the `web_search` cell spends a
//! credential grant instead of reading its key out of the environment.
//!
//! Locks T1–T3 and the `web_search` half of T10 of the plan
//! (`plan-parts/V3-search-code.md` § 4). Driven through the real factory and
//! the stateless dispatcher, against a loopback stub that records the
//! `Authorization` header of every request and how many were in flight at once.
//! Secrets are stub values (`stub-secret-<n>`).

#[path = "support/gh1060.rs"]
mod support;

use meclaw_cells::WebSearchCellFactory;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use support::{
    Stub, error_code, is_credential_request, recipient_of, sealed_box, spawn, tool_call,
};

const RESULTS: &str = r#"{"results":[{"title":"A","url":"u1","snippet":"s1"}]}"#;

fn params(stub: &Stub, extra: Value) -> Value {
    let mut p = json!({"endpoint": stub.url(), "external_timeout_ms": 10_000});
    for (k, v) in extra.as_object().expect("object").iter() {
        p[k] = v.clone();
    }
    p
}

fn search(i: usize) -> Value {
    tool_call(json!({"query": format!("q{i}")}), &format!("call-{i}"))
}

/// T1: three calls before the box ask ONCE; nothing reaches the provider
/// until the box is there; afterwards each request carries the delivered value
/// — and never the literal `api_key` beside the grant (OR-VG-4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_web_search_asks_once_and_parks() {
    let stub = Stub::start(Duration::ZERO, RESULTS.into());
    let mut cell = spawn(
        Arc::new(WebSearchCellFactory),
        params(
            &stub,
            json!({"credential_grant_id": "g-search", "api_key": "stub-literal-1"}),
        ),
    );
    for i in 0..3 {
        cell.send(search(i)).await;
    }
    let request = cell.next().await;
    assert!(
        is_credential_request(&request),
        "the first emission asks for the credential: {request}"
    );
    assert_eq!(request["header"]["grant_id"], "g-search");
    let quiet = cell.drain(Duration::from_millis(500)).await;
    assert!(
        quiet.is_empty(),
        "three parked calls ask once and answer nothing yet: {quiet:?}"
    );
    assert_eq!(stub.requests(), 0, "no request goes out without the box");

    cell.send(sealed_box(&recipient_of(&request), "stub-secret-1"))
        .await;
    for _ in 0..3 {
        let reply = cell.next().await;
        assert!(!is_credential_request(&reply), "no second round: {reply}");
        assert_eq!(reply["header"]["operation"], "web_search", "{reply}");
        assert_eq!(error_code(&reply), None, "the parked call ran: {reply}");
    }
    assert_eq!(
        stub.auth(),
        vec![Some("Bearer stub-secret-1".to_string()); 3],
        "every request carries the delivered value, none the literal"
    );

    // The slot stays open: the next call runs at once, without a new round.
    cell.send(search(9)).await;
    let reply = cell.next().await;
    assert_eq!(reply["header"]["operation"], "web_search", "{reply}");
    assert_eq!(stub.requests(), 4);
}

/// T2: no grant, no key — the call goes out without an `Authorization`
/// header, and the cell asks nobody for anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_web_search_without_grant_stays_anonymous() {
    let stub = Stub::start(Duration::ZERO, RESULTS.into());
    let mut cell = spawn(Arc::new(WebSearchCellFactory), params(&stub, json!({})));
    cell.send(search(1)).await;
    let reply = cell.next().await;
    assert_eq!(reply["header"]["operation"], "web_search", "{reply}");
    assert_eq!(error_code(&reply), None, "{reply}");
    assert_eq!(stub.auth(), vec![None], "no Authorization header at all");
    let rest = cell.drain(Duration::from_millis(300)).await;
    assert!(
        rest.iter().all(|c| !is_credential_request(c)),
        "a cell without a grant never asks: {rest:?}"
    );
}

/// T3: a box that does not open refuses the round — every parked call gets
/// its error answer at once (not at the deadline), the deliverer an
/// `invalid_input`, and the provider sees nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_web_search_refuses_on_a_box_that_does_not_open() {
    let stub = Stub::start(Duration::ZERO, RESULTS.into());
    let mut cell = spawn(
        Arc::new(WebSearchCellFactory),
        params(
            &stub,
            // A long deadline: an answer before it is the refusal, not the
            // timeout.
            json!({"credential_grant_id": "g-search", "credential_wait_ms": 60_000}),
        ),
    );
    cell.send(search(1)).await;
    cell.send(search(2)).await;
    let request = cell.next().await;
    assert!(is_credential_request(&request), "{request}");

    let stranger = meclaw_cells::sealed::RecipientKeypair::generate().expect("keypair");
    cell.send(sealed_box(&stranger.public_hex(), "stub-secret-3"))
        .await;
    let mut codes = Vec::new();
    for _ in 0..3 {
        let reply = cell.next().await;
        assert_eq!(reply["header"]["finish_reason"], "error", "{reply}");
        assert!(
            !reply.to_string().contains("stub-secret-3"),
            "no answer echoes the value"
        );
        codes.push(error_code(&reply).unwrap_or_default().to_string());
    }
    codes.sort();
    assert_eq!(
        codes,
        vec!["credential_pending", "credential_pending", "invalid_input"],
        "two parked calls refused, one refusal to the deliverer"
    );
    assert_eq!(stub.requests(), 0, "nothing reached the provider");
}

/// T10 (`web_search` half): four calls against a provider that takes its time
/// overlap — with and without a grant. A grant must not make the cell serial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_parallel_calls_stay_parallel_web_search() {
    for grant in [false, true] {
        let stub = Stub::start(Duration::from_millis(400), RESULTS.into());
        let extra = if grant {
            json!({"max_concurrency": 4, "credential_grant_id": "g-search"})
        } else {
            json!({"max_concurrency": 4})
        };
        let mut cell = spawn(Arc::new(WebSearchCellFactory), params(&stub, extra));
        for i in 0..4 {
            cell.send(search(i)).await;
        }
        if grant {
            let request = cell.next().await;
            assert!(is_credential_request(&request), "{request}");
            cell.send(sealed_box(&recipient_of(&request), "stub-secret-10"))
                .await;
        }
        for _ in 0..4 {
            let reply = cell.next().await;
            assert_eq!(error_code(&reply), None, "{reply}");
        }
        assert_eq!(stub.requests(), 4);
        assert!(
            stub.max_in_flight() >= 2,
            "grant={grant}: the four calls overlapped (max in flight {})",
            stub.max_in_flight()
        );
    }
}
