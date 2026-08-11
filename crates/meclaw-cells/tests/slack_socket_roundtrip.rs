//! P12 S8–S13: the Socket Mode round trip against the hermetic fake Slack.
//!
//! Everything here is `127.0.0.1`, deterministic, and costs nothing. What it
//! proves is the part the pure wire tests cannot: that opening a connection,
//! receiving an envelope, acknowledging it and posting a reply actually compose
//! over a real socket with a real HTTP client.
//!
//! Receipts are positive throughout — an event ARRIVED on the channel, an ack
//! ARRIVED at the server, a post ARRIVED with a specific token. None of the
//! assertions lean on "nothing bad happened".

use meclaw_cells::proxy::slack::client::{SlackClient, SlackError};
use meclaw_cells::proxy::slack::io::{ConnectionEnd, connect_and_run};
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_cells::proxy::slack::wire::SlackEventKind;
use meclaw_testing::mock_slack::{
    MockSlack, SlackScript, app_mention, event_callback, message_event,
};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

const APP_TOKEN: &str = "xapp-test-token";
const BOT_TOKEN: &str = "xoxb-test-token";

fn params_for(base_url: &str) -> SlackParams {
    SlackParams::parse(&json!({
        "app_token": APP_TOKEN,
        "bot_token": BOT_TOKEN,
        "emit_to": "/agent",
        "base_url": base_url,
        "connect_timeout_ms": 5000,
        "send_timeout_ms": 5000
    }))
    .expect("params must parse")
}

/// Waits for one inbound event with a generous failure-marker timeout.
async fn recv_one(
    rx: &mut mpsc::Receiver<meclaw_cells::proxy::slack::io::SlackInbound>,
) -> meclaw_cells::proxy::slack::io::SlackInbound {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no inbound event within 30s")
        .expect("channel closed before an event arrived")
}

/// Polls the server's ack list until it holds `n` entries.
async fn wait_for_acks(server: &MockSlack, n: usize) -> Vec<String> {
    for _ in 0..300 {
        let acks = server.acks().await;
        if acks.len() >= n {
            return acks;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server never saw {n} ack(s); got {:?}", server.acks().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_connect_receive_ack_is_one_continuous_path() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server
        .script_for(
            APP_TOKEN,
            SlackScript::new("A_SELF").envelope(
                "env-1",
                event_callback(
                    "A_SELF",
                    app_mention("C1", "U_HUMAN", "<@BOT> hello", "111.000111", None),
                ),
            ),
        )
        .await;

    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    let (tx, mut rx) = mpsc::channel(8);
    let mut own_app_id = None;
    let runner = tokio::spawn(async move {
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_secs(5),
            &meclaw_colony::IoLivenessMark::disabled(),
        )
        .await
    });

    let inbound = recv_one(&mut rx).await;
    assert_eq!(inbound.envelope_id, "env-1");
    assert!(matches!(inbound.event.kind, SlackEventKind::Mention));
    assert_eq!(inbound.event.channel, "C1");
    assert_eq!(inbound.event.ts, "111.000111");

    let acks = wait_for_acks(&server, 1).await;
    assert_eq!(acks, vec!["env-1".to_string()]);
    assert_eq!(server.open_calls().await, 1);
    assert_eq!(server.ws_connects().await, 1);

    // The app-level token authenticates the open call, in the header.
    let tokens = server.open_tokens().await;
    assert_eq!(
        tokens[0].as_deref(),
        Some(format!("Bearer {APP_TOKEN}").as_str())
    );

    runner.abort();
}

/// Ignoring is not the same as failing to receive: a dropped event must still
/// be acknowledged, or Slack redelivers it three times over several minutes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_bot_events_are_still_acknowledged() {
    let server = MockSlack::start().await.expect("fake slack starts");
    let mut bot_msg = message_event("C1", "channel", "U_BOT", "I am a bot", "1.1", None);
    bot_msg["bot_id"] = json!("B999");
    server
        .script_for(
            APP_TOKEN,
            SlackScript::new("A_SELF")
                .envelope("env-bot", event_callback("A_SELF", bot_msg))
                .envelope(
                    "env-human",
                    event_callback(
                        "A_SELF",
                        app_mention("C1", "U_HUMAN", "<@BOT> hi", "2.2", None),
                    ),
                ),
        )
        .await;

    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    let (tx, mut rx) = mpsc::channel(8);
    let mut own_app_id = None;
    let runner = tokio::spawn(async move {
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_secs(5),
            &meclaw_colony::IoLivenessMark::disabled(),
        )
        .await
    });

    // The human event is the ONLY one that reaches the handler.
    let inbound = recv_one(&mut rx).await;
    assert_eq!(
        inbound.envelope_id, "env-human",
        "the bot event must never reach the handler"
    );

    // Both envelopes are acknowledged, in arrival order.
    let acks = wait_for_acks(&server, 2).await;
    assert_eq!(acks, vec!["env-bot".to_string(), "env-human".to_string()]);

    runner.abort();
}

/// The `hello` frame is what teaches the connection its own app id; rule R3
/// cannot work before it arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn own_app_id_from_hello_drives_r3() {
    let server = MockSlack::start().await.expect("fake slack starts");
    let mut self_msg = message_event("C1", "channel", "U_X", "echo", "3.3", None);
    self_msg["app_id"] = json!("A_SELF");
    server
        .script_for(
            APP_TOKEN,
            SlackScript::new("A_SELF")
                .envelope("env-self", event_callback("A_SELF", self_msg))
                .envelope(
                    "env-other",
                    event_callback(
                        "A_SELF",
                        app_mention("C1", "U_HUMAN", "<@BOT> yo", "4.4", None),
                    ),
                ),
        )
        .await;

    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    let (tx, mut rx) = mpsc::channel(8);
    let mut own_app_id = None;
    let runner = tokio::spawn(async move {
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_secs(5),
            &meclaw_colony::IoLivenessMark::disabled(),
        )
        .await
    });

    let inbound = recv_one(&mut rx).await;
    assert_eq!(
        inbound.envelope_id, "env-other",
        "our own app's message must be filtered by R3"
    );
    wait_for_acks(&server, 2).await;
    runner.abort();
}

/// A `disconnect` frame ends the connection with its reason intact, which is
/// what the reconnect decision upstream is made of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_frame_ends_the_connection_with_its_reason() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server
        .script_for(
            APP_TOKEN,
            SlackScript::new("A_SELF").disconnect("refresh_requested"),
        )
        .await;

    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    let (tx, _rx) = mpsc::channel(8);
    let mut own_app_id = None;
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_secs(5),
            &meclaw_colony::IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("connection must end promptly on disconnect");

    match end {
        ConnectionEnd::Disconnect(reason) => assert_eq!(reason, "refresh_requested"),
        other => panic!("expected Disconnect, got {other:?}"),
    }
}

/// A malformed frame must not take the socket down — the next good frame still
/// arrives. One bad byte from Slack cannot be allowed to kill a live bot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_frame_does_not_kill_the_connection() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server
        .script_for(
            APP_TOKEN,
            SlackScript::new("A_SELF")
                .raw("{ this is not json ")
                .envelope(
                    "env-after",
                    event_callback(
                        "A_SELF",
                        app_mention("C1", "U_HUMAN", "<@BOT> still here?", "5.5", None),
                    ),
                ),
        )
        .await;

    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    let (tx, mut rx) = mpsc::channel(8);
    let mut own_app_id = None;
    let runner = tokio::spawn(async move {
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_secs(5),
            &meclaw_colony::IoLivenessMark::disabled(),
        )
        .await
    });

    let inbound = recv_one(&mut rx).await;
    assert_eq!(inbound.envelope_id, "env-after");
    runner.abort();
}

// ------------------------------------------------------------ Web API client

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn posting_uses_the_bot_token_and_targets_the_thread() {
    let server = MockSlack::start().await.expect("fake slack starts");
    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");

    client
        .chat_post_message("C1", "an answer", Some("111.000111"))
        .await
        .expect("post succeeds");

    let posts = server.posts().await;
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].channel(), Some("C1"));
    assert_eq!(posts[0].text(), Some("an answer"));
    assert_eq!(posts[0].thread_ts(), Some("111.000111"));
    assert_eq!(
        posts[0].authorization.as_deref(),
        Some(format!("Bearer {BOT_TOKEN}").as_str()),
        "the reply must carry the BOT token, never the app token"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_without_a_thread_omits_thread_ts_entirely() {
    let server = MockSlack::start().await.expect("fake slack starts");
    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    client
        .chat_post_message("D1", "dm reply", None)
        .await
        .expect("post succeeds");
    let posts = server.posts().await;
    assert!(
        posts[0].body.get("thread_ts").is_none(),
        "a DM reply must not invent a thread: {:?}",
        posts[0].body
    );
}

/// Dead credentials are permanent: retrying them fast is pointless and looks
/// like an attack from Slack's side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_auth_is_permanent_not_transient() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server.fail_open_with("invalid_auth").await;
    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    match client.apps_connections_open().await {
        Err(SlackError::Permanent(m)) => assert!(m.contains("invalid_auth")),
        other => panic!("expected Permanent, got {other:?}"),
    }
}

/// Both of Slack's rate-limit spellings must classify as transient. They differ
/// by one letter and by which tier produced them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_rate_limit_spellings_are_transient() {
    for code in ["rate_limited", "ratelimited"] {
        let server = MockSlack::start().await.expect("fake slack starts");
        server.fail_post_with(200, code).await;
        let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
        match client.chat_post_message("C1", "x", None).await {
            Err(SlackError::Transient(m)) => assert!(m.contains(code)),
            other => panic!("expected Transient for {code}, got {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_in_channel_is_permanent() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server.fail_post_with(200, "not_in_channel").await;
    let client = SlackClient::new(&params_for(&server.base_url())).expect("client builds");
    match client.chat_post_message("C1", "x", None).await {
        Err(SlackError::Permanent(m)) => assert!(m.contains("not_in_channel")),
        other => panic!("expected Permanent, got {other:?}"),
    }
}

/// Hard rule 12: every external op carries its own A-timeout. Proven by making
/// the server stall well past it and checking we come back as transient rather
/// than hanging until some outer backstop fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_fires_on_a_stalled_server() {
    let server = MockSlack::start().await.expect("fake slack starts");
    server.delay_http(Duration::from_secs(20)).await;
    let mut params = params_for(&server.base_url());
    params.connect_timeout_ms = 300;
    params.send_timeout_ms = 300;
    let client = SlackClient::new(&params).expect("client builds");

    let started = std::time::Instant::now();
    match client.apps_connections_open().await {
        Err(SlackError::Transient(m)) => assert!(m.contains("timeout"), "got {m}"),
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the A-timeout must fire long before the server would answer"
    );
}
