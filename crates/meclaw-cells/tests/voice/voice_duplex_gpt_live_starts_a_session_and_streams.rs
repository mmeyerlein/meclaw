//! Welle Live, L2a -- voice_duplex_gpt_live_starts_a_session_and_streams.
//!
//! The session opens with `session.start` carrying the configured rate and
//! `delegation.type: client` (R-25-4), the adapter reports `Started` with the
//! provider's own id, and every item out of `audio_in` becomes exactly one
//! `session.input_audio.append` -- never a collection, never a re-cut (R-L4).
//!
//! The append count is the point of this file. A buffer in front of the socket
//! would be invisible in every other test here and would show up in a call as
//! latency nobody ordered, so the fake keeps one entry per append and this test
//! compares the list, not its length.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::{GptLiveDuplex, session_url};
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// The params of a session against the fake: a credential that is not one, the
/// fake's address, and the two fields that have no default.
fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

/// A running session: the four channel ends the connection would hold, plus the
/// join handle of the session future.
struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    #[allow(dead_code)]
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

/// Opens a session against `mock`, held to the two deadlines given in ms.
fn start(mock: &MockGptLive, external_ms: u64, idle_ms: u64) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(external_ms),
        idle: Duration::from_millis(idle_ms),
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
    Session {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
        join: tokio::spawn(future),
    }
}

/// The next event, or a failed test -- never a hang.
async fn next_event(events: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for a duplex event")
        .expect("event channel closed early")
}

/// Polls `f` until it yields a value or the failure timeout elapses.
async fn eventually<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + FAILURE_TIMEOUT;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_starts_a_session_and_streams() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_live_1".to_string(),
        // The script does nothing: this test is about what the CLIENT sends.
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut session.events).await,
        DuplexEvent::Started {
            session_id: "sess_live_1".to_string()
        },
        "the session identity is the provider's, never one this adapter made up"
    );

    let opening = mock.session_start().await.expect("a session.start arrived");
    assert_eq!(opening["type"], "session.start");
    assert_eq!(opening["session"]["model"], "gpt-live-1");
    assert_eq!(opening["session"]["instructions"], "Be Egon.");
    assert_eq!(opening["session"]["audio"]["format"]["type"], "audio/pcm");
    assert_eq!(
        opening["session"]["audio"]["format"]["rate"], 16_000,
        "one format for both directions, at the negotiated rate"
    );
    assert_eq!(opening["session"]["audio"]["output"]["voice"], "marin");
    assert_eq!(
        opening["session"]["delegation"]["type"], "client",
        "the backend is this colony; OpenAI gets no second agent (R-25-4)"
    );
    assert!(
        mock.authorization_is_bearer().await,
        "the credential travels as a bearer header and nowhere else"
    );

    // Ten distinct frames in, ten appends out, in order and byte for byte.
    let frames: Vec<Vec<u8>> = (0..10u8)
        .map(|i| {
            vec![
                i,
                i.wrapping_add(17),
                i.wrapping_add(34),
                i.wrapping_add(51),
            ]
        })
        .collect();
    for frame in &frames {
        session
            .audio_in
            .send(frame.clone())
            .await
            .expect("the session still takes audio");
    }

    let appended = eventually("ten input appends", || async {
        let audio = mock.received_audio().await;
        (audio.len() >= frames.len()).then_some(audio)
    })
    .await;
    assert_eq!(
        appended, frames,
        "one client item is one append: a merged list here is a buffer in front of the socket (R-L4)"
    );

    session.join.abort();
}

/// R-V10 at the pure function rather than over the socket: the fake records no
/// path, and extending its surface to observe one would be a test fixture
/// grown for the sake of a test (OR-L55).
#[test]
fn the_base_url_gets_this_adapters_own_path() {
    assert_eq!(
        session_url("wss://api.openai.com"),
        "wss://api.openai.com/v1/live/sessions"
    );
    assert_eq!(
        session_url("http://127.0.0.1:8080"),
        "ws://127.0.0.1:8080/v1/live/sessions"
    );
}
