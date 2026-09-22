//! Welle Live, L2a -- voice_duplex_gpt_live_times_out_a_silent_handshake.
//!
//! The handshake and every send carry the A-timeout (hard rule 12), so a
//! silent peer is `DuplexError::Timeout` rather than a task parked for ever.
//!
//! Two peers are silent in two different ways and both are here: one never
//! completes the TCP handshake, and one completes it, takes the
//! `session.start` and then says nothing. The second is the one a backstop
//! alone would miss for as long as `cell.message_timeout` -- and a connection
//! task waiting on it holds a caller on an open line.

use meclaw_cells::voice::contract::{
    DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

/// Runs one session against `mock` with a tight operation timeout and returns
/// the verdict and how long it took to reach it.
async fn verdict_of(mock: &MockGptLive, external_ms: u64) -> (DuplexError, Duration) {
    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, _events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(external_ms),
        // Far beyond the failure marker: if the idle deadline were what ended
        // this session, the test would time out instead of passing.
        idle: Duration::from_secs(600),
    });
    let began = std::time::Instant::now();
    let verdict = tokio::time::timeout(
        FAILURE_TIMEOUT,
        provider.run_session(
            provider.format(),
            DuplexSession {
                audio_in: audio_in_rx,
                audio_out: audio_out_tx,
                events: events_tx,
                control: control_rx,
            },
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the operation timeout ended the wait, not the test's")
    .expect_err("a silent peer is never a session");
    (verdict, began.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_times_out_a_silent_handshake() {
    // A peer that never finishes the upgrade.
    let stalling = MockGptLive::start(LiveScript {
        accept_delay: Some(Duration::from_secs(20)),
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let (verdict, took) = verdict_of(&stalling, 250).await;
    assert!(
        matches!(verdict, DuplexError::Timeout),
        "a stalled handshake is a timeout: {verdict:?}"
    );
    assert!(
        took < Duration::from_secs(10),
        "the A-timeout ended it, not the far side: {took:?}"
    );

    // A peer that upgrades, takes the `session.start` and never answers it.
    let mute = MockGptLive::start(LiveScript {
        started: false,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let (verdict, took) = verdict_of(&mute, 250).await;
    assert!(
        matches!(verdict, DuplexError::Timeout),
        "a session.start nobody answers is a timeout: {verdict:?}"
    );
    assert!(
        took < Duration::from_secs(10),
        "the A-timeout ended it, not the far side: {took:?}"
    );
    assert!(
        mute.session_start().await.is_some(),
        "the adapter did open the session before it gave up on the answer"
    );
}
