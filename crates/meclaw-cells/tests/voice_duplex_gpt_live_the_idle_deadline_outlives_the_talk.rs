//! Welle Live, L2a -- voice_duplex_gpt_live_the_idle_deadline_outlives_the_talk.
//!
//! The idle deadline lives OUTSIDE the streaming loop and is reset only by a
//! frame that arrived. This file is the lock on that sentence: the caller talks
//! without a pause, the model says nothing at all, and the session still ends
//! on the deadline.
//!
//! It is the one shape of the guard that a plausible refactor breaks in
//! silence. A `timeout(idle, read.next())` inside the `select!` arm reads the
//! same and does nothing: every time the audio arm wins -- three thousand
//! times a minute against a talking caller -- the timeout future is dropped
//! and built again, so against a dead socket and a live caller it never fires.
//! That is precisely the case it exists for: a connection task waiting on a
//! provider that stopped answering holds an open line and a paying session.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the socket may stay quiet. Far below the failure marker and far
/// above the pause between two client frames, so what fires is never in doubt.
const IDLE: Duration = Duration::from_millis(500);

/// One 20 ms frame of 16 kHz PCM16 -- the size the connection really sends.
const FRAME_BYTES: usize = 640;

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_the_idle_deadline_outlives_the_talk() {
    // The fake upgrades, answers `session.start` and then says nothing for a
    // minute: a socket that is open and dead.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_idle".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
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
    let began = std::time::Instant::now();
    let join = tokio::spawn(future);

    // The caller keeps talking for as long as anybody listens. The task ends
    // when the adapter drops its receiver, and it reports how much it got in.
    let feeder = tokio::spawn(async move {
        let mut frames = 0usize;
        while audio_in_tx.send(vec![7u8; FRAME_BYTES]).await.is_ok() {
            frames += 1;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        frames
    });

    assert!(
        matches!(
            tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv())
                .await
                .expect("timed out waiting for `Started`"),
            Some(DuplexEvent::Started { .. })
        ),
        "the session opened before it went quiet"
    );

    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, join)
        .await
        .expect("the idle deadline ended the session, not the test's marker")
        .expect("the session task did not panic")
        .expect_err("a socket that says nothing for half a second is not a session");
    let took = began.elapsed();

    let DuplexError::Closed(detail) = &verdict else {
        panic!("a dead socket under a talking caller is `Closed`, not {verdict:?}");
    };
    assert!(
        detail.contains("idle"),
        "the reason names the guard that fired: {detail}"
    );
    assert!(
        took < Duration::from_secs(10),
        "the deadline ended it, not the fake's minute of silence: {took:?}"
    );

    // The load-bearing half: the audio arm really was winning the whole time.
    // Without this the test would also pass against a guard that any frame in
    // either direction keeps alive.
    let frames = tokio::time::timeout(FAILURE_TIMEOUT, feeder)
        .await
        .expect("the feeder stopped when the adapter let go")
        .expect("the feeder did not panic");
    assert!(
        frames > 20,
        "the caller was talking while the deadline ran down: {frames} frames"
    );
    assert!(
        mock.received_audio().await.len() > 20,
        "and the frames reached the socket: {} appends",
        mock.received_audio().await.len()
    );
}
