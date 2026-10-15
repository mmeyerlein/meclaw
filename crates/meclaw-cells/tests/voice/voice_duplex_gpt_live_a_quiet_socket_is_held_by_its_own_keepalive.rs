//! Welle Live, zell-uhr (GH #798) — the idle deadline means "the socket is
//! dead", not "the caller stopped talking".
//!
//! `provider_idle_timeout_ms` is a guard against a socket that went away
//! without saying so. Until this file it could not tell that case from an
//! ordinary pause: session frames alone do not say which of the two a silence
//! is, so thirty seconds of silence on the line read exactly like thirty
//! seconds of dead wire, and the call was cut — which is what the owner heard.
//!
//! How quiet a live session really goes is a separate question and an open one:
//! four runs on 2026-09-21 saw no session frame inside 45 s, two runs on
//! 2026-09-22 saw no gap past 604 ms on the same endpoint, and nothing measured
//! says what makes the difference (GH #798). The locks below do not depend on
//! it — a pong answers on the transport whether the model is talking or not.
//!
//! So the adapter asks. Every `keepalive_ms` it sends a WebSocket ping carrying
//! [`KEEPALIVE_PAYLOAD`], and the pong that comes back on that payload — and
//! only that one — resets the deadline. A pong is answered by the transport
//! itself, below anything a model does or fails to do, so what the deadline now
//! measures is the socket rather than the conversation.
//!
//! What still does NOT count is a ping somebody sends US: a proxy pinging a
//! dead upstream would otherwise hold a call open for as long as it likes
//! (OR-L.zell-uhr.1). The asymmetry is the whole point — our own question, our
//! own answer.
//!
//! The two locks are the two halves of that sentence, on deadlines short enough
//! to be watched: a line where nobody speaks and the pings come back keeps the
//! call, and the same line stops keeping it the moment the pongs stop.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::{GptLiveDuplex, KEEPALIVE_PAYLOAD};
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the socket may stay quiet. The shipped number is 30 000 ms; this
/// is the mechanism, scaled so a test can watch several windows go by.
const IDLE: Duration = Duration::from_millis(600);

/// How often the adapter asks. Same ratio as the shipped pair, 10 000 against
/// 30 000: three questions fit inside one deadline.
const KEEPALIVE: Duration = Duration::from_millis(200);

/// How long the quiet line is watched in the first lock — five idle windows.
const WATCH: Duration = Duration::from_millis(3_000);

/// When the fake stops answering in the second lock. Two idle windows in, so
/// the session has demonstrably outlived the deadline before it dies of it.
const DEAF_AFTER: Duration = Duration::from_millis(1_200);

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        "keepalive_ms": KEEPALIVE.as_millis() as u64,
    }))
    .expect("params parse")
}

/// Everything one scripted session is driven and watched by.
struct Running {
    /// Held, never written: the caller is on the line and silent. Dropping it
    /// would close the call instead of leaving it quiet.
    _audio_in: mpsc::Sender<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

/// Start the adapter against a fake running `script`.
async fn run(mock: &MockGptLive) -> Running {
    let (audio_in_tx, audio_in_rx) = mpsc::channel::<Vec<u8>>(32);
    let (audio_out_tx, mut audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: IDLE,
    });
    let future = provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    );
    let join = tokio::spawn(future);
    // Nothing is produced on a silent line, but a receiver that was dropped
    // would end the session for a reason this file is not about.
    tokio::spawn(async move { while audio_out_rx.recv().await.is_some() {} });
    Running {
        _audio_in: audio_in_tx,
        events: events_rx,
        join,
    }
}

/// Wait for the session to open, failing loudly rather than hanging.
async fn started(events: &mut mpsc::Receiver<DuplexEvent>) {
    let first = tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for `Started`");
    assert!(
        matches!(first, Some(DuplexEvent::Started { .. })),
        "the session opened, got {first:?}"
    );
}

/// A line where nobody says anything keeps the call, because the pings come
/// back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_session_lives_as_long_as_its_pings_come_back() {
    // The quiet line of GH #798: the session opens and then NOTHING is sent on
    // it — no audio, no transcript, no meter, no ack — for longer than the
    // adapter's own patience. The fake answers the transport, which is all a
    // live socket does when nobody is talking.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_quiet".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(30))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut running = run(&mock).await;
    started(&mut running.events).await;

    // Five idle windows with not one session frame on the socket.
    tokio::time::sleep(WATCH).await;

    assert!(
        !running.join.is_finished(),
        "a quiet line whose keepalives come back is a live call, not a dead socket"
    );
    let pings = mock.pings().await;
    let expected = WATCH.as_millis() / KEEPALIVE.as_millis() / 2;
    assert!(
        pings.len() as u128 >= expected,
        "the adapter asked on its own account: {} pings in {WATCH:?}, expected at least \
         {expected}",
        pings.len()
    );
    assert!(
        pings.iter().all(|p| p.as_slice() == KEEPALIVE_PAYLOAD),
        "every ping carries the payload that makes the pong recognisable as the answer \
         to OUR question: {pings:?}"
    );
    running.join.abort();
}

/// And the same line dies on the deadline once the pongs stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socket_that_stops_answering_dies_on_the_deadline() {
    // Same silent line, except that the far side goes away in the middle of it:
    // the socket stays open and the pings keep going out, and nothing comes
    // back. That is the case `provider_idle_timeout_ms` exists for, and now it
    // is the only one it fires on.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_deaf".to_string(),
        actions: vec![
            LiveAction::Delay(DEAF_AFTER),
            LiveAction::GoDeaf,
            LiveAction::Delay(Duration::from_secs(30)),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let began = Instant::now();
    let mut running = run(&mock).await;
    started(&mut running.events).await;

    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, running.join)
        .await
        .expect("the idle deadline ended the session, not the test's marker")
        .expect("the session task did not panic");
    let took = began.elapsed();

    let Err(DuplexError::Closed(detail)) = &verdict else {
        panic!("a socket that stopped answering is `Closed`, not {verdict:?}");
    };
    assert!(
        detail.contains("idle"),
        "the reason names the guard that fired: {detail}"
    );
    // The load-bearing number. Without the keepalive this session would have
    // been cut at IDLE, in silence, with the far side perfectly alive; it
    // outlived two of those windows and died in the third, which is the one
    // after the answers stopped.
    assert!(
        took > DEAF_AFTER,
        "the pongs held the deadline off while they came: {took:?} against {DEAF_AFTER:?}"
    );
    assert!(
        took < DEAF_AFTER + IDLE * 3,
        "and once they stopped, the deadline was what ended it: {took:?}"
    );
}
