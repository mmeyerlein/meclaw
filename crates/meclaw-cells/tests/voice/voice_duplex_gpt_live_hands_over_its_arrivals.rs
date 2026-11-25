//! #1631 -- a metered duplex session hands over how its audio arrived, once,
//! however it ends; a provider that does not measure hands over nothing.

use meclaw_cells::voice::contract::{
    AudioArrivals, AudioFormat, BoxFuture, DuplexControl, DuplexError, DuplexEvent, DuplexProvider,
    DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::{DuplexParams, GptLiveParams};
use meclaw_cells::voice::providers::build_duplex;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::deadline::HANG;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Runs one metered session to its end against a client that takes everything
/// it is given, and returns the verdict, what the session handed over and how
/// many chunks of audio the client was given.
async fn metered(
    provider: &dyn DuplexProvider,
    renewed: bool,
    hang_up: bool,
) -> (
    Result<(), DuplexError>,
    Result<AudioArrivals, oneshot::error::RecvError>,
    u64,
) {
    let (audio_in_tx, audio_in) = mpsc::channel(8);
    let (audio_out, mut audio_out_rx) = mpsc::channel::<Vec<u8>>(64);
    let (events, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control) = mpsc::channel::<DuplexControl>(8);
    let (arrivals, arrivals_rx) = oneshot::channel();
    let session = DuplexSession {
        audio_in,
        audio_out,
        events,
        control,
    };
    let future = provider.run_metered_session(
        provider.format(),
        session,
        IoLivenessMark::disabled(),
        renewed,
        arrivals,
    );
    let takes = tokio::spawn(async move {
        let mut taken = 0;
        while audio_out_rx.recv().await.is_some() {
            taken += 1;
        }
        taken
    });
    let hears = tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    // The caller stays on the line for the whole session, or hangs up now.
    let _line = (!hang_up).then_some(audio_in_tx);
    let verdict = tokio::time::timeout(HANG, future)
        .await
        .expect("the session ends");
    let taken = takes.await.expect("the audio is taken");
    hears.await.expect("the events are taken");
    let handed = tokio::time::timeout(HANG, arrivals_rx)
        .await
        .expect("the hand-over is decided when the session has returned");
    (verdict, handed, taken)
}

async fn mock(end: LiveAction) -> MockGptLive {
    MockGptLive::start(LiveScript {
        actions: vec![
            LiveAction::SendAudio(vec![0; 4_800]),
            LiveAction::SendAudio(vec![0; 4_800]),
            LiveAction::SendAudio(vec![0; 4_800]),
            end,
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake")
}

fn live(mock: &MockGptLive, idle: Duration) -> GptLiveDuplex {
    let params: GptLiveParams = serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": mock.base_url(),
        "instructions": "Be brief.",
        "greeting": "Greet the caller.",
    }))
    .expect("params parse");
    GptLiveDuplex::new(params).with_timeouts(ProviderTimeouts {
        external: HANG,
        idle,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_hands_over_its_arrivals() {
    for renewed in [false, true] {
        let mock = mock(LiveAction::CloseWith {
            reason: "done",
            usage_seconds: 1.0,
        })
        .await;
        let (verdict, handed, _) = metered(&live(&mock, HANG), renewed, false).await;
        assert!(verdict.is_ok(), "an ordinary end: {verdict:?}");
        let handed = handed.expect("a session that measures hands over");
        assert_eq!(handed.chunks, 3, "every chunk is counted");
        let start = mock.session_start().await.expect("the session was opened");
        let greeting = json!([{"type": "message", "role": "developer",
                               "content": [{"type": "input_text", "text": "Greet the caller."}]}]);
        assert_eq!(
            start["session"].get("input"),
            (!renewed).then_some(&greeting),
            "a renewal greets nobody (renewed: {renewed})"
        );
    }
}

/// The way out that is an error: the far side goes silent, the idle deadline
/// ends the session, and what had arrived until then is still handed over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_ends_in_an_error_hands_over_its_arrivals() {
    let mock = mock(LiveAction::Delay(Duration::from_secs(60))).await;
    let (verdict, handed, taken) =
        metered(&live(&mock, Duration::from_secs(2)), false, false).await;
    assert!(
        matches!(verdict, Err(DuplexError::Closed(_))),
        "the idle deadline is a disturbance: {verdict:?}"
    );
    let handed = handed.expect("an error half way hands over too");
    assert_eq!(handed.chunks, taken, "what arrived until then is counted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_does_not_measure_hands_over_nothing() {
    let params: DuplexParams =
        serde_json::from_value(json!({"provider": "echo"})).expect("the block parses");
    let echo = build_duplex(&params, ProviderTimeouts::default()).expect("the echo is built");
    let (verdict, handed, _) = metered(&*echo, false, true).await;
    assert!(verdict.is_ok(), "a hang-up is an ordinary end: {verdict:?}");
    assert!(handed.is_err(), "the sender was dropped");
}

/// A provider that does not measure, and says which of its sessions ran.
struct Unmetered(Arc<Mutex<Vec<&'static str>>>);

impl Unmetered {
    fn ran(&self, which: &'static str) -> BoxFuture<Result<(), DuplexError>> {
        self.0.lock().expect("no poison").push(which);
        Box::pin(async { Ok(()) })
    }
}

impl DuplexProvider for Unmetered {
    fn name(&self) -> &'static str {
        "unmetered"
    }
    fn format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }
    fn run_session(
        &self,
        _format: AudioFormat,
        _session: DuplexSession,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        self.ran("first")
    }
    fn run_renewed_session(
        &self,
        _format: AudioFormat,
        _session: DuplexSession,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        self.ran("renewed")
    }
}

/// The trait's default keeps the two sessions apart: a renewal greets nobody,
/// metered or not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_runs_the_session_it_was_asked_for() {
    for (renewed, want) in [(false, "first"), (true, "renewed")] {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (verdict, handed, _) = metered(&Unmetered(ran.clone()), renewed, true).await;
        assert!(verdict.is_ok(), "an ordinary end: {verdict:?}");
        assert!(handed.is_err(), "the sender was dropped");
        assert_eq!(*ran.lock().expect("no poison"), vec![want]);
    }
}
