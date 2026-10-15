//! Issue #8: rule-12 coverage for the two Slack Web API calls.
//!
//! Same defect class as the Telegram long poll fixed in `proxy/io.rs` (see
//! `tests/proxy_io_hang_deadline.rs`): `tokio::time::timeout` wrapped only
//! `send()`, i.e. the **header phase**. The body read (`resp.json().await`) ran
//! with no deadline at all. A half-dead socket — headers delivered, body never,
//! no FIN and no RST, the shape a NAT idle timeout or a blackholed path
//! produces — parks the call forever, unlogged and invisible to the watchdog.
//!
//! Hard rule 12 wants the deadline around the WHOLE external operation. The two
//! calls keep their existing budgets (`connect_timeout_ms` for
//! `apps.connections.open`, `send_timeout_ms` for `chat.postMessage`); the
//! deadline simply now covers send + status + body instead of send alone.
//!
//! The fixture server models the half-dead peer exactly.

use meclaw_cells::proxy::slack::client::{SlackClient, SlackError};
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing_hanging_body};
use serde_json::json;
use std::time::Duration;

const APP_TOKEN: &str = "xapp-hang-deadline";
const BOT_TOKEN: &str = "xoxb-hang-deadline";

fn params_for(base_url: &str, connect_timeout_ms: u64, send_timeout_ms: u64) -> SlackParams {
    SlackParams::parse(&json!({
        "app_token": APP_TOKEN,
        "bot_token": BOT_TOKEN,
        "emit_to": "/agent",
        "base_url": base_url,
        "connect_timeout_ms": connect_timeout_ms,
        "send_timeout_ms": send_timeout_ms,
    }))
    .expect("params parse")
}

/// `apps.connections.open` against a peer that sends the head and then goes
/// silent. Without the outer deadline the call sits in the body read forever
/// and the 30 s failure marker fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apps_connections_open_gives_up_on_a_half_dead_body() {
    // The head announces a perfectly valid response; only the body never comes.
    let responses = vec![MockResponse::ok_json(
        br#"{"ok":true,"url":"wss://example.invalid/link"}"#,
    )];
    let (addr, _join, _cap) = start_mock_server_capturing_hanging_body(1, responses).await;
    let client =
        SlackClient::new(&params_for(&format!("http://{addr}"), 800, 800)).expect("client builds");

    let started = std::time::Instant::now();
    // 30 s failure-marker convention (robust against parallel cargo load); the
    // semantic discriminator is the elapsed assertion below.
    let res = tokio::time::timeout(Duration::from_secs(30), client.apps_connections_open())
        .await
        .expect(
            "apps.connections.open hung on the half-dead body - \
             the deadline covers only the header phase",
        );
    match res {
        Err(SlackError::Transient(m)) => assert!(m.contains("timeout"), "got {m}"),
        other => panic!("expected a transient timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the 800 ms budget must bound the whole operation, took {:?}",
        started.elapsed()
    );
}

/// Same for `chat.postMessage` — the outbound path. A hung send means the
/// answer to a user is stuck in a call nobody can see.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_post_message_gives_up_on_a_half_dead_body() {
    let responses = vec![MockResponse::ok_json(br#"{"ok":true}"#)];
    let (addr, _join, _cap) = start_mock_server_capturing_hanging_body(1, responses).await;
    let client =
        SlackClient::new(&params_for(&format!("http://{addr}"), 800, 800)).expect("client builds");

    let started = std::time::Instant::now();
    let res = tokio::time::timeout(
        Duration::from_secs(30),
        client.chat_post_message("C1", "hello", None),
    )
    .await
    .expect(
        "chat.postMessage hung on the half-dead body - \
         the deadline covers only the header phase",
    );
    match res {
        Err(SlackError::Transient(m)) => assert!(m.contains("timeout"), "got {m}"),
        other => panic!("expected a transient timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the 800 ms budget must bound the whole operation, took {:?}",
        started.elapsed()
    );
}

/// The deadline is finite but generous: a legitimate answer that is merely slow
/// still gets through, on both calls, and the error mapping is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_deadline_does_not_cut_a_slow_but_legitimate_answer() {
    let responses = vec![
        MockResponse::ok_json(br#"{"ok":true,"url":"wss://example.invalid/socket"}"#)
            .with_delay(Duration::from_millis(1200)),
        MockResponse::ok_json(br#"{"ok":true}"#).with_delay(Duration::from_millis(1200)),
    ];
    // `hang_after_head = 0`: every connection is answered in full.
    let (addr, _join, _cap) = start_mock_server_capturing_hanging_body(0, responses).await;
    let client = SlackClient::new(&params_for(&format!("http://{addr}"), 4000, 4000))
        .expect("client builds");

    let started = std::time::Instant::now();
    let url = client
        .apps_connections_open()
        .await
        .expect("a 1.2 s answer fits into the 4 s budget");
    assert_eq!(url, "wss://example.invalid/socket");
    client
        .chat_post_message("C1", "hello", Some("9.9"))
        .await
        .expect("a 1.2 s answer fits into the 4 s budget");
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "the fixture was supposed to be slow, took {:?}",
        started.elapsed()
    );
}
