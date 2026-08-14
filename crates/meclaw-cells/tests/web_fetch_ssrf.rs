//! Track C7 (GH #117) — the `web_fetch` redirect policy under the SSRF deny.
//!
//! The deny matrix itself is unit-pinned in `src/web_fetch.rs`. What this
//! battery proves is the part that only exists end to end: a redirect chain is
//! followed BY THE CELL, hop by hop, and every hop passes the deny again.
//!
//! The classic bypass this closes: a perfectly innocent public URL answers
//! `302 Location: http://169.254.169.254/latest/meta-data/`, and a client whose
//! egress check ran once — at the URL the caller wrote — walks straight into the
//! cloud metadata service.
//!
//! Hermetic by construction: every server here is a local listener, no name is
//! ever resolved against the outside world, and the one target that is *named*
//! (`169.254.169.254`) is refused before a connection to it is opened.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::WebFetchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

/// A local HTTP listener that answers each request from `reply`, which sees the
/// request path and returns the raw response head+body to write.
///
/// Deliberately hand-rolled and blocking on its own thread: the shared
/// `meclaw_testing::mock_http` server cannot emit a `Location` header, and a
/// redirect test is exactly a test about that header. The thread ends with the
/// process; no test waits on it.
fn start_scripted_server<F>(reply: F) -> SocketAddr
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            // Drain the header block so the client sees a well-formed exchange.
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) if line == "\r\n" || line == "\n" => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let _ = stream.write_all(reply(path).as_bytes());
            let _ = stream.flush();
        }
    });
    addr
}

/// `302` with a `Location`.
fn redirect_to(location: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

/// `200` with a plain-text body.
fn ok_with(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A rig whose cell may reach the local listener (`allow_private_networks`) —
/// the documented opt-out, not a softened default.
fn local_rig(max_redirects: u64) -> ToolRig {
    ToolRig::spawn(
        Arc::new(WebFetchCellFactory) as Arc<dyn CellFactory>,
        "/fetch",
        json!({
            "max_concurrency": 2,
            "external_timeout_ms": 10000,
            "allow_private_networks": true,
            "max_redirects": max_redirects
        }),
    )
}

/// THE bypass. A public-looking first hop 302s into the cloud metadata service;
/// the deny has to fire at the HOP, not only at the URL the caller wrote.
///
/// The opt-out is on, so loopback — the first hop — is reachable. Link-local
/// stays refused in both policy tiers precisely so this proof is possible
/// without a public server, and because nothing anybody deliberately runs lives
/// at `169.254.169.254`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_into_the_metadata_endpoint_is_refused_at_the_hop() {
    let addr =
        start_scripted_server(|_| redirect_to("http://169.254.169.254/latest/meta-data/iam/"));
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/harmless")}), "c1")
        .await;

    assert_error(&em, "target_blocked");
    let text = text_of(&em);
    assert!(
        text.contains("169.254.169.254"),
        "the refusal names the address it refused: {text}"
    );
}

/// The same shape one level deeper: hop 1 is fine, hop 2 is the attack. A deny
/// that only re-checks the FIRST redirect is not a per-hop deny.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_deny_fires_on_a_later_hop_too() {
    let addr = start_scripted_server(|path| match path {
        "/a" => redirect_to("/b"),
        "/b" => redirect_to("http://169.254.169.254/latest/meta-data/"),
        _ => ok_with("should never be reached"),
    });
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/a")}), "c1")
        .await;
    assert_error(&em, "target_blocked");
    assert!(text_of(&em).contains("169.254.169.254"));
}

/// Redirects inside the budget are followed, and the caller is told: a body
/// that came from somewhere other than the requested url says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_within_the_budget_is_followed_and_reported() {
    let addr = start_scripted_server(|path| match path {
        "/1" => redirect_to("/2"),
        "/2" => redirect_to("/3"),
        "/3" => ok_with("arrived"),
        _ => ok_with("unexpected"),
    });
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/1")}), "c1")
        .await;

    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "arrived");
    assert_eq!(header_of(&em, "http_status"), 200);
    assert_eq!(header_of(&em, "redirects"), 2, "two hops were followed");
    assert_eq!(
        header_of(&em, "final_url"),
        &json!(format!("http://{addr}/3")),
        "the caller can see where the body really came from"
    );
}

/// A fetch that never redirects carries no redirect bookkeeping — the headers
/// stay exactly as they were before GH #117 for the ordinary case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_direct_fetch_carries_no_redirect_headers() {
    let addr = start_scripted_server(|_| ok_with("direct"));
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/")}), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert!(header_of(&em, "redirects").is_null());
    assert!(header_of(&em, "final_url").is_null());
}

/// The budget bites, and it is a typed error — an endless chain must not become
/// an endless fetch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_past_the_budget_is_a_typed_error() {
    let addr = start_scripted_server(|_| redirect_to("/loop"));
    let mut r = local_rig(2);

    let em = r
        .call(json!({"url": format!("http://{addr}/loop")}), "c1")
        .await;

    assert_error(&em, "too_many_redirects");
    assert!(
        text_of(&em).contains("max_redirects"),
        "the refusal names the knob that stopped it: {}",
        text_of(&em)
    );
}

/// `max_redirects: 0` means what it says.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_budget_refuses_the_very_first_redirect() {
    let addr = start_scripted_server(|path| match path {
        "/start" => redirect_to("/target"),
        _ => ok_with("target reached"),
    });
    let mut r = local_rig(0);

    let em = r
        .call(json!({"url": format!("http://{addr}/start")}), "c1")
        .await;
    assert_error(&em, "too_many_redirects");
}

/// A `Location` naming a transport this cell does not speak is refused, not
/// followed and not silently dropped — the same class as `file://` at the input
/// gate (GH #110).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_to_a_foreign_scheme_is_refused() {
    let addr = start_scripted_server(|_| redirect_to("file:///etc/passwd"));
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/x")}), "c1")
        .await;
    assert_error(&em, "invalid_redirect");
    assert!(
        text_of(&em).contains("file"),
        "the refusal names the scheme it will not follow: {}",
        text_of(&em)
    );
}

/// A `3xx` WITHOUT a `Location` is not a redirect — it is a document with a 3xx
/// status, and a status is data for the caller (the cell's standing convention).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_3xx_without_a_location_stays_a_normal_result() {
    let addr = start_scripted_server(|_| {
        "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    });
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/x")}), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "http_status"), 304);
}

/// A relative `Location` resolves against the hop it came from — and is then
/// screened like any other target.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_location_resolves_against_the_current_hop() {
    let addr = start_scripted_server(|path| match path {
        "/deep/one" => redirect_to("two"),
        "/deep/two" => ok_with("relative resolved"),
        other => ok_with(other),
    });
    let mut r = local_rig(5);

    let em = r
        .call(json!({"url": format!("http://{addr}/deep/one")}), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "relative resolved");
}

/// The DEFAULT still refuses the local listener outright — none of the tests
/// above are evidence that the deny got softer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_still_refuses_the_local_listener_these_tests_opt_into() {
    let addr = start_scripted_server(|_| ok_with("never delivered"));
    let mut r = ToolRig::spawn(
        Arc::new(WebFetchCellFactory) as Arc<dyn CellFactory>,
        "/fetch",
        json!({"max_concurrency": 2, "external_timeout_ms": 10000}),
    );

    let em = r
        .call(json!({"url": format!("http://{addr}/")}), "c1")
        .await;
    assert_error(&em, "target_blocked");
}
