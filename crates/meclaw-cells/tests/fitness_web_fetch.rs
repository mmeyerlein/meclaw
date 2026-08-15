//! Track T (#104) — fitness battery for the `web_fetch` cell.
//!
//! Against a local mock server (`meclaw_testing::mock_http`), LLM-free:
//!
//! - 2xx delivers the body verbatim with the full header set;
//! - non-2xx is a NORMAL tool_result with `http_status` — routing decides,
//!   the cell does not editorialise;
//! - the operation timeout (rule 12) and connect failure are typed errors;
//! - missing/invalid `url` is `invalid_input`.
//!
//! Deliberately NOT covered here: `max_bytes`/`truncated` behaviour — GH #83
//! runs in parallel (track H); this battery only pins that an UNDER-cap body
//! carries no `truncated` marker.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::WebFetchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use std::sync::Arc;
use std::time::Duration;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

fn rig_with(params: meclaw_core::JsonValue) -> ToolRig {
    ToolRig::spawn(
        Arc::new(WebFetchCellFactory) as Arc<dyn CellFactory>,
        "/fetch",
        params,
    )
}

/// GH #117: every server this battery talks to is a local mock on 127.0.0.1,
/// which the shipped DEFAULT now refuses. The battery takes the documented
/// opt-out (`allow_private_networks`) — the default is NOT softened to keep it
/// green, and `web_fetch_ssrf.rs` pins that the default still refuses exactly
/// these targets.
fn rig() -> ToolRig {
    rig_with(json!({"max_concurrency": 2, "external_timeout_ms": 10000,
                    "allow_private_networks": true}))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_2xx_body_arrives_verbatim_with_the_full_header_set() {
    let (addr, _srv) = start_mock_server(MockResponse::ok(b"<html>fixture doc</html>")).await;
    let mut r = rig();

    let em = r
        .call(json!({"url": format!("http://{addr}/doc")}), "c1")
        .await;

    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "<html>fixture doc</html>");
    assert_eq!(header_of(&em, "operation"), "web_fetch");
    assert_eq!(header_of(&em, "http_status"), 200);
    assert_eq!(header_of(&em, "bytes"), 24);
    assert!(
        header_of(&em, "content_type").is_string(),
        "content_type travels"
    );
    assert!(
        header_of(&em, "truncated").is_null(),
        "an under-cap body carries no truncated marker (cap itself is GH #83 / track H)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_404_is_a_normal_result_the_status_is_data() {
    let (addr, _srv) = start_mock_server(MockResponse::not_found()).await;
    let mut r = rig();

    let em = r
        .call(json!({"url": format!("http://{addr}/missing")}), "c1")
        .await;

    assert_normal_result(&em, "c1");
    assert_eq!(
        header_of(&em, "http_status"),
        404,
        "non-2xx is data for the caller, not a cell error"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_500_is_equally_a_normal_result() {
    let (addr, _srv) = start_mock_server(MockResponse::server_error()).await;
    let mut r = rig();

    let em = r
        .call(json!({"url": format!("http://{addr}/")}), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "http_status"), 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_server_hits_the_typed_operation_timeout() {
    // Rule 12 concept A: the cell's own timeout, not the substrate backstop.
    let (addr, _srv) =
        start_mock_server(MockResponse::ok(b"too late").with_delay(Duration::from_secs(20))).await;
    let mut r = rig_with(json!({"max_concurrency": 1, "external_timeout_ms": 300,
                                "allow_private_networks": true}));

    let started = std::time::Instant::now();
    let em = r
        .call(json!({"url": format!("http://{addr}/")}), "c1")
        .await;

    assert_error(&em, "timeout");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the timeout fired, the cell did not wait the server out"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_refused_is_a_typed_io_error() {
    // Bind-then-drop guarantees a port with no listener.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut r = rig();

    let em = r
        .call(json!({"url": format!("http://127.0.0.1:{port}/")}), "c1")
        .await;
    assert_error(&em, "io_error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_or_malformed_url_is_invalid_input() {
    let mut r = rig();

    let em = r.call(json!({}), "c1").await;
    assert_error(&em, "invalid_input");

    let em = r.call(json!({"url": ""}), "c2").await;
    assert_error(&em, "invalid_input");

    // GH #110 (redefines the T-pin assertion: this used to be `io_error`):
    // the gate PARSES the URL now. A syntax error is a broken CALL, and the
    // repair is to rewrite it — `io_error` would have read as "network
    // problem, retry", which is the wrong repair applied forever.
    for (i, bad) in [
        "not-a-url",         // no scheme at all
        "http://",           // scheme without a host
        "://example.org",    // empty scheme
        "ht tp://a.example", // space in the scheme
    ]
    .into_iter()
    .enumerate()
    {
        let em = r.call(json!({"url": bad}), &format!("c3{i}")).await;
        assert_error(&em, "invalid_input");
        assert!(
            text_of(&em).contains(bad),
            "the refusal quotes the url it could not parse: {:?}",
            text_of(&em)
        );
    }

    // A URL that parses but names a transport this cell does not speak is the
    // same class: the call is wrong, not the network.
    for (i, bad) in ["file:///etc/passwd", "ftp://example.org/x"]
        .into_iter()
        .enumerate()
    {
        let em = r.call(json!({"url": bad}), &format!("c4{i}")).await;
        assert_error(&em, "invalid_input");
    }
}
