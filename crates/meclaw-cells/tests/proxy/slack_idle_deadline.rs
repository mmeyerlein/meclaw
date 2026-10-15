//! Issue #50: the Socket Mode read loop needs an idle deadline.
//!
//! Sibling class to issue #8, different fix. #8 wrapped a bounded request in an
//! operation deadline. A stream has no operation boundary to wrap, so the guard
//! here is an idle deadline: no frame for `idle_timeout_ms` means the socket is
//! presumed dead, `connect_and_run` returns `ConnectionEnd::Transient`, and the
//! existing reconnect machinery takes over — no panic, no cell death.
//!
//! The defect this pins: on a blackholed path (NAT idle timeout, dropped route,
//! no FIN, no RST) nothing ever arrives — no frame, no ping, no error — so the
//! unguarded `read.next().await` parked forever. The lane looked idle from the
//! outside while it was dead, and none of the `ConnectionEnd` return paths that
//! drive a reconnect was ever reached.
//!
//! The opposite direction is pinned just as hard, because it is the failure
//! mode a badly chosen deadline produces: a healthy connection that carries no
//! events must NOT be torn down. Slack keeps such a connection audible with
//! WebSocket pings, and a ping is a life sign like any other frame.
//!
//! Timing convention: the deadline under test is sub-second so the suite stays
//! fast; failure markers stay at the 30 s convention. Every semantic timing
//! discriminator is justified where it appears.

use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::io::{ConnectionEnd, connect_and_run};
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_testing::mock_slack::{MockSlack, SlackScript, app_mention, event_callback};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

const APP_TOKEN: &str = "xapp-idle-deadline";
const BOT_TOKEN: &str = "xoxb-idle-deadline";

/// The deadline under test. Sub-second so the whole file runs in a few seconds;
/// the production default is 120 s and derived in `SlackParams`.
const IDLE_MS: u64 = 400;

fn params_for(base_url: &str) -> SlackParams {
    SlackParams::parse(&json!({
        "app_token": APP_TOKEN,
        "bot_token": BOT_TOKEN,
        "emit_to": "/agent",
        "base_url": base_url,
        "connect_timeout_ms": 5000,
        "idle_timeout_ms": IDLE_MS
    }))
    .expect("params must parse")
}

/// Runs one connection under the small idle deadline, with the 30 s failure
/// marker around it. Returns how the connection ended.
async fn run_one(server: &MockSlack, marker: &str) -> (ConnectionEnd, Duration) {
    let params = params_for(&server.base_url());
    let client = SlackClient::new(&params).expect("client builds");
    let (tx, _rx) = mpsc::channel(8);
    let mut own_app_id = None;

    let started = std::time::Instant::now();
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_millis(params.connect_timeout_ms),
            Duration::from_millis(params.idle_timeout_ms),
            &meclaw_colony::IoLivenessMark::disabled(),
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("{marker}"));
    (end, started.elapsed())
}

/// (a) The blackhole. A server that completes the handshake, says `hello`, and
/// then goes silent forever. The read loop must end the connection on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_socket_ends_the_connection_instead_of_hanging() {
    let server = MockSlack::start().await.expect("fake slack starts");
    // No actions: `hello` goes out, then the socket stays open and mute.
    server
        .script_for(APP_TOKEN, SlackScript::new("A_SELF"))
        .await;

    let (end, elapsed) = run_one(
        &server,
        "the read loop hung on a blackholed socket - no idle deadline",
    )
    .await;

    match end {
        ConnectionEnd::Transient(m) => assert!(
            m.contains("idle"),
            "an idle trip must be nameable in the log: {m}"
        ),
        other => panic!("expected a transient idle end, got {other:?}"),
    }
    // Semantic discriminator: the 400 ms budget must be what ends this, not the
    // 30 s marker. Ten seconds is two orders of magnitude of slack over the
    // budget and still an order of magnitude under the marker.
    assert!(
        elapsed < Duration::from_secs(10),
        "the idle budget must bound the silence, took {elapsed:?}"
    );
}

/// (b) The life sign. Slack pings a quiet Socket Mode connection; a ping is a
/// frame and must reset the deadline exactly like an event does. Without that,
/// the default would disconnect every healthy bot in a quiet workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regular_pings_keep_a_quiet_connection_alive() {
    let server = MockSlack::start().await.expect("fake slack starts");
    // 16 pings, one every 150 ms, then a `disconnect` as the positive receipt.
    // 16 × 150 ms = 2.4 s of pinging = six full 400 ms deadline periods, so a
    // deadline that ignored control frames could not survive this script.
    let mut script = SlackScript::new("A_SELF");
    for _ in 0..16 {
        script = script.delay_ms(150).ping();
    }
    server
        .script_for(APP_TOKEN, script.delay_ms(150).disconnect("ping_survivor"))
        .await;

    let (end, elapsed) = run_one(&server, "the ping-fed connection hung").await;

    // Positive receipt: the connection lived long enough to reach the frame at
    // the END of the script. An idle trip would have returned Transient before.
    match end {
        ConnectionEnd::Disconnect(reason) => assert_eq!(reason, "ping_survivor"),
        other => panic!("pings must reset the idle deadline, got {other:?}"),
    }
    // Semantic discriminator: it really did outlive several deadline periods
    // rather than racing through the script.
    assert!(
        elapsed >= Duration::from_millis(2 * IDLE_MS),
        "the script was supposed to span several deadline periods, took {elapsed:?}"
    );
}

/// (c) Regression: ordinary event frames reset the deadline just as well, over
/// a span several deadline periods long. This is the traffic case — a busy lane
/// must never be cut by the guard that exists for a dead one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_event_frames_reset_the_deadline_too() {
    let server = MockSlack::start().await.expect("fake slack starts");
    let mut script = SlackScript::new("A_SELF");
    for i in 0..8 {
        script = script.delay_ms(300).envelope(
            &format!("env-{i}"),
            event_callback(
                "A_SELF",
                app_mention(
                    "C1",
                    "U_HUMAN",
                    "<@BOT> still here",
                    &format!("{i}.1"),
                    None,
                ),
            ),
        );
    }
    // 8 × 300 ms = 2.4 s, i.e. six 400 ms deadline periods, every gap below it.
    server
        .script_for(APP_TOKEN, script.delay_ms(300).disconnect("event_survivor"))
        .await;

    let params = params_for(&server.base_url());
    let client = SlackClient::new(&params).expect("client builds");
    let (tx, mut rx) = mpsc::channel(16);
    let mut own_app_id = None;

    let started = std::time::Instant::now();
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        connect_and_run(
            &client,
            &tx,
            &mut own_app_id,
            None,
            Duration::from_millis(params.connect_timeout_ms),
            Duration::from_millis(params.idle_timeout_ms),
            &meclaw_colony::IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the event-fed connection hung");
    let elapsed = started.elapsed();

    match end {
        ConnectionEnd::Disconnect(reason) => assert_eq!(reason, "event_survivor"),
        other => panic!("event frames must reset the idle deadline, got {other:?}"),
    }
    // Positive receipt: every scripted event actually came through the loop.
    drop(tx);
    let mut ids = Vec::new();
    while let Some(inbound) = rx.recv().await {
        ids.push(inbound.envelope_id);
    }
    assert_eq!(
        ids,
        (0..8).map(|i| format!("env-{i}")).collect::<Vec<_>>(),
        "all eight events must have been delivered before the disconnect"
    );
    assert!(
        elapsed >= Duration::from_millis(2 * IDLE_MS),
        "the script was supposed to span several deadline periods, took {elapsed:?}"
    );
}

/// The default is the operator-facing half of the fix: 120 s, four missed pings
/// at Slack's slowest documented cadence. A default that could cut a healthy
/// quiet socket would be worse than no deadline at all.
#[test]
fn the_idle_default_is_several_missed_ping_intervals() {
    let p = SlackParams::parse(&json!({
        "app_token": "a", "bot_token": "b", "emit_to": "/x"
    }))
    .expect("minimal params parse");
    assert_eq!(p.idle_timeout_ms, 120_000);
    assert!(
        p.idle_timeout_ms >= 4 * 30_000,
        "the default must survive several missed pings at the slowest documented cadence"
    );
}
