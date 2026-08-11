//! P12 S14/S15: reconnect and backoff, against the hermetic fake Slack.
//!
//! Two properties are at stake and they pull in opposite directions. A bot must
//! come back promptly after Slack's routine `refresh_requested` (which arrives
//! every few hours and is not an error), and it must NOT hammer a server that
//! is failing or dropping connections instantly. The min-uptime floor is what
//! separates the two cases, and both directions are pinned below.

use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::io::{SlackIoConfig, SlackReconfig, run_slack_io};
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_testing::mock_slack::{MockSlack, SlackScript, app_mention, event_callback};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

const APP_TOKEN: &str = "xapp-reconnect";
const BOT_TOKEN: &str = "xoxb-reconnect";

fn params_for(base_url: &str) -> SlackParams {
    SlackParams::parse(&json!({
        "app_token": APP_TOKEN,
        "bot_token": BOT_TOKEN,
        "emit_to": "/agent",
        "base_url": base_url,
        "connect_timeout_ms": 3000
    }))
    .expect("params parse")
}

fn io_config(base_url: &str, min_uptime_ms: u64) -> SlackIoConfig {
    let params = params_for(base_url);
    SlackIoConfig {
        client: SlackClient::new(&params).expect("client builds"),
        bot_user_id: None,
        connect_timeout_ms: params.connect_timeout_ms,
        min_uptime_ms,
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    }
}

/// The core reconnect proof: Slack asks us to go away, and we come back on a
/// FRESH `apps.connections.open` — the old WebSocket URL carries a single-use
/// ticket and cannot be redialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disconnect_frame_is_followed_by_a_new_open_and_delivery_resumes() {
    let server = MockSlack::start().await.expect("fake slack");
    server
        .scripts_for(
            APP_TOKEN,
            vec![
                // First connection: routine refresh.
                SlackScript::new("A_SELF").disconnect("refresh_requested"),
                // Second connection: the event we must not miss.
                SlackScript::new("A_SELF").envelope(
                    "env-after-reconnect",
                    event_callback(
                        "A_SELF",
                        app_mention("C1", "U_HUMAN", "<@BOT> still there?", "9.9", None),
                    ),
                ),
            ],
        )
        .await;

    let (events_tx, mut events_rx) = mpsc::channel(8);
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(4);
    let io = tokio::spawn(run_slack_io(
        io_config(&server.base_url(), 0),
        events_tx,
        reconfig_rx,
    ));

    let inbound = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("event must arrive after the reconnect")
        .expect("channel open");
    assert_eq!(inbound.envelope_id, "env-after-reconnect");
    assert!(
        server.open_calls().await >= 2,
        "the reconnect must obtain a fresh ticket, not reuse the old URL"
    );

    io.abort();
}

/// The busy-spin pin. A server that accepts and instantly drops every socket
/// must NOT produce a reconnect storm. Without the min-uptime floor each cycle
/// would look "healthy", reset the backoff, and hammer Slack as fast as the
/// event loop allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instant_drops_do_not_become_a_reconnect_storm() {
    let server = MockSlack::start().await.expect("fake slack");
    server
        .script_for(APP_TOKEN, SlackScript::new("A_SELF").close())
        .await;

    let (events_tx, _events_rx) = mpsc::channel(8);
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(4);
    // A connection must survive 5s to count as healthy; these all die at once.
    let io = tokio::spawn(run_slack_io(
        io_config(&server.base_url(), 5000),
        events_tx,
        reconfig_rx,
    ));

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let calls = server.open_calls().await;
    io.abort();

    // First attempt is immediate, then 1s of backoff: at most a couple within
    // 1.5s. An unthrottled loop would produce hundreds.
    assert!(
        calls <= 4,
        "reconnect storm: {calls} open calls in 1.5s — backoff is not engaging"
    );
    assert!(calls >= 1, "the loop must have tried at least once");
}

/// Dead credentials must not be retried at speed. `invalid_auth` will not fix
/// itself, and fast retries against it look like an attack from Slack's side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_auth_backs_off_hard_instead_of_retrying() {
    let server = MockSlack::start().await.expect("fake slack");
    server.fail_open_with("invalid_auth").await;

    let (events_tx, _events_rx) = mpsc::channel(8);
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(4);
    let io = tokio::spawn(run_slack_io(
        io_config(&server.base_url(), 0),
        events_tx,
        reconfig_rx,
    ));

    tokio::time::sleep(Duration::from_millis(1200)).await;
    let calls = server.open_calls().await;
    io.abort();
    assert_eq!(
        calls, 1,
        "a permanently rejected token must be tried once, then left alone"
    );
}

/// Closing the reconfig channel is the shutdown signal, exactly as in the
/// Telegram I/O task. It must be honoured promptly even while the loop is
/// sitting in a backoff sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_reconfig_channel_stops_the_loop_during_backoff() {
    let server = MockSlack::start().await.expect("fake slack");
    server.fail_open_with("invalid_auth").await; // drives it into the long sleep

    let (events_tx, _events_rx) = mpsc::channel(8);
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<SlackReconfig>(4);
    let io = tokio::spawn(run_slack_io(
        io_config(&server.base_url(), 0),
        events_tx,
        reconfig_rx,
    ));

    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(reconfig_tx);

    // Must return on its own — not be aborted, and not wait out the 5min sleep.
    tokio::time::timeout(Duration::from_secs(30), io)
        .await
        .expect("io task must stop when the reconfig channel closes")
        .expect("task must not panic");
}
