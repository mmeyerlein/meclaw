//! GH #1060 (Vault-Grants, strand V3): the `web_search` cell spends a
//! credential grant instead of reading its key out of the environment.
//!
//! Locks T1–T3 and the `web_search` half of T10 of the plan
//! (`plan-parts/V3-search-code.md` § 4). Driven through the real factory and
//! the stateless dispatcher, against a loopback stub that records the
//! `Authorization` header of every request and how many were in flight at once.
//! Secrets are stub values (`stub-secret-<n>`).

// harness_fold: #[path = "support/gh1060.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_cells::WebSearchCellFactory;
use meclaw_cells::credential::CREDENTIAL_PENDING_DETAIL;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use support::{
    Stub, error_code, is_credential_request, recipient_of, sealed_box, spawn, tool_call,
};

/// The refusal's detail ([`meclaw_cells::credential::Refusal::detail`] for a box
/// that does not open), distinct from the deadline's
/// [`CREDENTIAL_PENDING_DETAIL`].
const REFUSED: &str = "the sealed box did not open (";

const RESULTS: &str = r#"{"results":[{"title":"A","url":"u1","snippet":"s1"}]}"#;

fn params(stub: &Stub, extra: Value) -> Value {
    let mut p = json!({"endpoint": stub.url(), "external_timeout_ms": meclaw_testing::deadline::product_wait_ms()});
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
    let stub = Stub::start(RESULTS.into());
    let mut cell = spawn(
        Arc::new(WebSearchCellFactory),
        params(
            &stub,
            json!({"credential_grant_id": "g-search", "api_key": "stub-literal-1",
                   "credential_wait_ms": meclaw_testing::deadline::product_wait_ms()}),
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
    assert_eq!(stub.requests(), 0, "no request goes out without the box");

    // WHY `settle` and no quiet window (dev-rules § 11, re-check tv-br M10g):
    // the three calls and the box are four deliveries; once their four consume
    // marks are in, everything they will ever emit is in `rest`. "Asked once"
    // = no second request among them; "answered nothing before the box" =
    // every answer is a clean `web_search` reply that carried the delivered
    // value (the `stub.auth` row below) -- an answer without the box could
    // carry neither.
    cell.send(sealed_box(&recipient_of(&request), "stub-secret-1"))
        .await;
    let rest = cell.settle().await;
    assert_eq!(rest.len(), 3, "one answer per parked call: {rest:?}");
    for reply in &rest {
        assert!(!is_credential_request(reply), "no second round: {reply}");
        assert_eq!(reply["header"]["operation"], "web_search", "{reply}");
        assert_eq!(error_code(reply), None, "the parked call ran: {reply}");
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
    let stub = Stub::start(RESULTS.into());
    let mut cell = spawn(Arc::new(WebSearchCellFactory), params(&stub, json!({})));
    cell.send(search(1)).await;
    let reply = cell.next().await;
    assert_eq!(reply["header"]["operation"], "web_search", "{reply}");
    assert_eq!(error_code(&reply), None, "{reply}");
    assert_eq!(stub.auth(), vec![None], "no Authorization header at all");
    // The call's consume mark closes everything it emitted (`settle`): no
    // request before the reply and none after it, without a quiet window.
    let rest = cell.settle().await;
    assert!(
        rest.is_empty(),
        "a cell without a grant never asks: {rest:?}"
    );
}

/// T3: a box that does not open refuses the round — every parked call gets
/// the refusal as its error answer (not the deadline's), the deliverer an
/// `invalid_input`, and the provider sees nothing.
///
/// WHY the detail (re-check tv-br M8 B-M1, TV-Q1/TV-Z3): the refusal and the
/// deadline both answer `credential_pending`; only the detail tells them
/// apart -- the refusal's `the sealed box did not open (…)`, the deadline's
/// `CREDENTIAL_PENDING_DETAIL`. That is the verdict, a positive marker without
/// a clock. The deadline is 600 s, so it cannot fire in a run; the `next()`
/// mark (scaled `HANG`) stays a hang marker and decides nothing.
///
/// WHY two orders (tv-br M8, measured: red 1 in 31 on build04 in the first run
/// of M8, the third emission was a second `credential_request`): every call is
/// its own `handle` task, so the second call's `Need` can reach the slot task
/// after the box was refused -- the round is closed then, and the call opens a
/// round of its own. That is the slot working, not a defect; the test assumed
/// one interleaving. `early` sends both calls before the box and takes either
/// order; `late` forces the second one (it is sent after the deliverer's
/// refusal arrived). Every round a box does not open is refused the same way:
/// its parked calls `credential_pending`, its deliverer `invalid_input`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_web_search_refuses_on_a_box_that_does_not_open() {
    for late in [false, true] {
        let stub = Stub::start(RESULTS.into());
        let mut cell = spawn(
            Arc::new(WebSearchCellFactory),
            params(
                &stub,
                // A deadline no run reaches (as gh1092/gh1094): which of the two
                // answered is read from the detail below, never from the clock.
                json!({"credential_grant_id": "g-search", "credential_wait_ms": meclaw_testing::deadline::product_wait_ms()}),
            ),
        );
        cell.send(search(1)).await;
        if !late {
            cell.send(search(2)).await;
        }
        let request = cell.next().await;
        assert!(is_credential_request(&request), "{request}");

        let stranger = meclaw_cells::sealed::RecipientKeypair::generate().expect("keypair");
        cell.send(sealed_box(&stranger.public_hex(), "stub-secret-3"))
            .await;
        let mut boxes = 1;
        let mut second_sent = !late;
        let mut codes: Vec<String> = Vec::new();
        while codes.len() < 2 + boxes {
            let reply = cell.next().await;
            assert!(
                !reply.to_string().contains("stub-secret-3"),
                "no answer echoes the value"
            );
            if is_credential_request(&reply) {
                // The second call came after the refusal: its own round, refused
                // the same way.
                cell.send(sealed_box(&stranger.public_hex(), "stub-secret-3"))
                    .await;
                boxes += 1;
                continue;
            }
            assert_eq!(
                reply["header"]["finish_reason"], "error",
                "late {late}: {reply}"
            );
            let code = error_code(&reply).unwrap_or_default().to_string();
            let detail = reply["messages"][0]["text"].as_str().unwrap_or_default();
            assert!(
                detail.starts_with(REFUSED) && detail != CREDENTIAL_PENDING_DETAIL,
                "late {late}: every answer of the round carries the refusal, not the \
                 deadline's receipt: {code}: {detail:?}"
            );
            if !second_sent && code == "invalid_input" {
                cell.send(search(2)).await;
                second_sent = true;
            }
            codes.push(code);
        }
        codes.sort();
        let mut want = vec!["credential_pending".to_string(); 2];
        want.extend(vec!["invalid_input".to_string(); boxes]);
        assert_eq!(
            codes, want,
            "late {late}: two parked calls refused, one refusal to each of the {boxes} deliverers"
        );
        if late {
            assert_eq!(
                boxes, 2,
                "a call after the refusal opens a round of its own and asks again"
            );
        }
        assert_eq!(stub.requests(), 0, "nothing reached the provider");
    }
}

/// T10 (`web_search` half): four calls against a provider that takes its time
/// overlap — with and without a grant. A grant must not make the cell serial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1060_parallel_calls_stay_parallel_web_search() {
    for grant in [false, true] {
        let stub = Stub::start_until_overlap(2, RESULTS.into());
        let extra = if grant {
            json!({"max_concurrency": 4, "credential_grant_id": "g-search",
                   "credential_wait_ms": meclaw_testing::deadline::product_wait_ms()})
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
