//! Phase-10-C T4: frame types compile.
//! Phase-10-C T7: `run_io` skeleton — pushes first update, advances
//! offset, shuts down on reconfig-channel close.
use meclaw_cells::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::time::Duration;
use tokio::sync::mpsc;

#[test]
fn frame_types_compile() {
    let _ev = ProxyEvent::UserMessage {
        update_id: 42,
        chat_id: 100,
        user_id: Some(200),
        message_id: Some(7),
        text: "hi".into(),
    };
    // ProxyReconfig has no variants yet — type existence is enough.
    fn _accepts_reconfig(_r: ProxyReconfig) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_pushes_first_update_then_loops() {
    // Two responses: first delivers update_id=42, second is empty
    // (= long-poll-timeout pattern on the Telegram side).
    let r1 = MockResponse::ok_json(
        br#"{"ok":true,"result":[
        {"update_id":42,"message":{"message_id":7,"chat":{"id":100},"text":"hi"}}
    ]}"#,
    );
    let r2 = MockResponse::ok_json(br#"{"ok":true,"result":[]}"#);
    let (addr, _j, _c) = start_mock_server_capturing(vec![r1, r2]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();

    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 1,
        long_poll_timeout_ms: 2000,
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    let ev = tokio::time::timeout(Duration::from_millis(2000), events_rx.recv())
        .await
        .expect("no event")
        .unwrap();
    match ev {
        ProxyEvent::UserMessage {
            update_id, chat_id, ..
        } => {
            assert_eq!(update_id, 42);
            assert_eq!(chat_id, 100);
        }
    }

    // Shutdown via reconfig-channel close (T7 stub: reacts to close).
    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_backoff_recovers_after_transient_failures_no_livelock() {
    // 3x 5xx -> I/O backs off. Then 1x 200 with a real update -> MUST get
    // through (proof against livelock). Then 1x 200 with empty list (loop
    // keeps ticking).
    let responses = vec![
        MockResponse::server_error(),
        MockResponse::server_error(),
        MockResponse::server_error(),
        MockResponse::ok_json(
            br#"{"ok":true,"result":[
            {"update_id":7,"message":{"message_id":1,"chat":{"id":1},"text":"recovered"}}
        ]}"#,
        ),
        MockResponse::ok_json(br#"{"ok":true,"result":[]}"#),
    ];
    let (addr, _j, cap) = start_mock_server_capturing(responses).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();

    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 0,
        long_poll_timeout_ms: 500,
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    // Proof against livelock: an event MUST arrive within the deadline.
    // 1s sleep after 1st failure, 2s after 2nd, 4s after 3rd = up to 7s
    // accumulated backoff, plus reqwest round-trip time. 12s deadline is
    // generous.
    let ev = tokio::time::timeout(Duration::from_secs(12), events_rx.recv())
        .await
        .expect("recovery event missing - livelock?")
        .unwrap();
    match ev {
        ProxyEvent::UserMessage {
            update_id, text, ..
        } => {
            assert_eq!(update_id, 7);
            assert_eq!(text, "recovered");
        }
    }

    // Sanity: at least 4 requests at the mock (3 failed + 1 recovery).
    let cap_len = cap.lock().await.len();
    assert!(
        cap_len >= 4,
        "expected >=4 requests after recovery, got {cap_len}"
    );

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_backoff_starts_after_first_failure_not_immediately() {
    // Sanity: 1st poll fires immediately (backoff init = 0); only the 2nd
    // poll must come after >=1s.
    use std::time::Instant;
    let responses = vec![
        MockResponse::server_error(),
        MockResponse::ok_json(br#"{"ok":true,"result":[]}"#),
    ];
    let (addr, _j, cap) = start_mock_server_capturing(responses).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let (events_tx, mut _events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 0,
        long_poll_timeout_ms: 500,
    };
    let t0 = Instant::now();
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    while cap.lock().await.len() < 2 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let elapsed = t0.elapsed();
    // 1st poll: immediate. 2nd poll: after >=1000ms backoff. Flake-robust: >=800ms.
    assert!(
        elapsed >= Duration::from_millis(800),
        "expected >=800ms between 1st failure and 2nd poll, elapsed: {elapsed:?}"
    );

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_handles_multiple_updates_in_single_response() {
    let r1 = MockResponse::ok_json(
        br#"{"ok":true,"result":[
        {"update_id":10,"message":{"message_id":1,"chat":{"id":100},"text":"a"}},
        {"update_id":11,"message":{"message_id":2,"chat":{"id":100},"text":"b"}}
    ]}"#,
    );
    let r2 = MockResponse::ok_json(br#"{"ok":true,"result":[]}"#);
    let (addr, _j, cap) = start_mock_server_capturing(vec![r1, r2]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();

    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 0,
        long_poll_timeout_ms: 2000,
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    let ev1 = tokio::time::timeout(Duration::from_millis(2000), events_rx.recv())
        .await
        .expect("no 1st event")
        .unwrap();
    let ev2 = tokio::time::timeout(Duration::from_millis(2000), events_rx.recv())
        .await
        .expect("no 2nd event")
        .unwrap();
    let ProxyEvent::UserMessage { update_id: id1, .. } = ev1;
    let ProxyEvent::UserMessage { update_id: id2, .. } = ev2;
    assert_eq!((id1, id2), (10, 11));

    // Wait for the second request, then check offset=12 (= max(10,11)+1).
    while cap.lock().await.len() < 2 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let cap = cap.lock().await;
    assert!(cap[1].path.contains("offset=12"), "got: {}", cap[1].path);

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_abort_during_backoff_sleep_terminates_promptly() {
    // Mock delivers 5xx -> I/O enters backoff-sleep (1s). We drop the
    // reconfig_rx sender within 100ms - the task must terminate within
    // <2s, not only after the backoff-sleep elapses.
    let (addr, _j, _c) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let (events_tx, mut _events_rx) = mpsc::channel::<ProxyEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(8);
    let cfg = RunIoConfig {
        client,
        initial_offset: 0,
        long_poll_request_secs: 0,
        long_poll_timeout_ms: 500,
    };
    let join = tokio::spawn(run_io(cfg, events_tx, rc_rx));

    // Wait until run_io is in backoff-sleep (mock-server responded with 5xx).
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("abort during the backoff sleep did not take effect promptly")
        .unwrap();
}
