//! P15 addendum: rule-12 coverage for the `getUpdates` long poll.
//!
//! Trigger was a production hang (2026-08-11): the Telegram `proxy` silently
//! stopped polling. No log event, an open ESTAB connection to
//! `api.telegram.org`, and updates waiting on Telegram's side.
//!
//! The cause is the reach of the A-timeout in `TelegramClient::get_updates`:
//! `tokio::time::timeout(client_timeout, ...)` wrapped only `send()`, i.e. the
//! **header phase**. The body read (`resp.json().await`) ran with no deadline.
//! A half-dead socket (NAT idle timeout, no FIN/RST) delivers the headers and
//! then never the body, and the lane hangs forever.
//!
//! The fixture server models exactly that: head out, body never, socket open.

use meclaw_cells::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing_hanging_body};
use std::time::Duration;
use tokio::sync::mpsc;

/// Positive receipt: after a half-dead poll the lane stays alive and delivers
/// the next update. Without the outer deadline `run_io` sits in the body read
/// of the first poll, no event ever arrives, and this test fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_survives_a_half_dead_poll_and_delivers_the_next_update() {
    // Connection 1: head out, body never (half-dead peer). Connection 2: the
    // update. Connections 3+: empty, so the loop keeps running quietly.
    let responses = vec![
        MockResponse::ok_json(br#"{"ok":true,"result":[]}"#),
        MockResponse::ok_json(
            br#"{"ok":true,"result":[
            {"update_id":77,"message":{"message_id":1,"chat":{"id":9},"text":"alive"}}
        ]}"#,
        ),
        MockResponse::ok_json(br#"{"ok":true,"result":[]}"#),
    ];
    let (addr, _j, cap) = start_mock_server_capturing_hanging_body(1, responses).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();

    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    // `long_poll_request_secs = 0` + `long_poll_timeout_ms = 500` gives an
    // outer deadline of 0 * 1000 + 500 = 500 ms. The W7 tripwire (client >
    // server) holds; the existing `run_io` tests use the same combination.
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 0,
        long_poll_timeout_ms: 500,
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    // 500 ms deadline + 1 s transient backoff + round trip. 30 s convention
    // for the failure marker (robust against parallel cargo load).
    let ev = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("lane hung on the half-dead poll - the outer getUpdates deadline is missing")
        .unwrap();
    match ev {
        ProxyEvent::UserMessage {
            update_id, text, ..
        } => {
            assert_eq!(update_id, 77);
            assert_eq!(text, "alive");
        }
    }

    // Sanity: the half-dead poll and the successful one are two connections.
    let cap_len = cap.lock().await.len();
    assert!(
        cap_len >= 2,
        "expected >=2 requests (hung poll + recovery), got {cap_len}"
    );

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}

/// The deadline is **finite and generous**: it must not cut a legitimate long
/// poll. The fixture answers only after `long_poll_request_secs`, i.e. exactly
/// inside the allowed window, and the update must still arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_outer_deadline_does_not_cut_a_legitimate_long_poll() {
    let slow = MockResponse::ok_json(
        br#"{"ok":true,"result":[
        {"update_id":5,"message":{"message_id":1,"chat":{"id":9},"text":"slow but valid"}}
    ]}"#,
    )
    .with_delay(Duration::from_millis(1200));
    let responses = vec![slow, MockResponse::ok_json(br#"{"ok":true,"result":[]}"#)];
    let (addr, _j, _c) = start_mock_server_capturing_hanging_body(0, responses).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();

    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    // The server holds for 1 s (`long_poll_request_secs = 1`) and answers
    // after 1.2 s. Outer deadline = 1 * 1000 + 2000 = 3000 ms, so it fits.
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 1,
        long_poll_timeout_ms: 2000,
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    let ev = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("no event - the outer deadline cut a legitimate long poll")
        .unwrap();
    let ProxyEvent::UserMessage { update_id, .. } = ev;
    assert_eq!(update_id, 5);

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}
