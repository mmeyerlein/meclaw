//! Welle Live, L2a -- voice_duplex_gpt_live_sends_the_greeting_once.
//!
//! A non-empty `greeting` becomes exactly one append after `session.started`,
//! and an empty one sends nothing at all (OR-L25).
//!
//! It travels on `commentary`, not on `instructions` as the contract first
//! wrote it: the S0 measurement spoke the greeting 3 times out of 3 with a
//! median of 938 ms on the commentary channel and 2 times out of 9 on the
//! documented one (OR-L51). `commentary` is the channel for "say this next",
//! which is what a greeting is; `instructions` changes how the model behaves
//! and leaves it to the model when to act on it.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

/// What the twin's manifest says, in English and one sentence shorter.
const GREETING: &str = "Greet the caller now, one sentence, then listen.";

fn params(base_url: &str, greeting: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        "greeting": greeting,
    }))
    .expect("params parse")
}

struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    #[allow(dead_code)]
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn start(mock: &MockGptLive, external_ms: u64, greeting: &str) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider =
        GptLiveDuplex::new(params(&mock.base_url(), greeting)).with_timeouts(ProviderTimeouts {
            external: Duration::from_millis(external_ms),
            idle: Duration::from_millis(30_000),
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
async fn voice_duplex_gpt_live_sends_the_greeting_once() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_greeting".to_string(),
        actions: vec![
            LiveAction::ExpectAppend {
                kind: "commentary",
                contains: "Greet the caller",
            },
            LiveAction::Delay(Duration::from_secs(60)),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 5_000, GREETING);
    assert!(matches!(
        next_event(&mut session.events).await,
        DuplexEvent::Started { .. }
    ));
    assert_eq!(
        next_event(&mut session.events).await,
        DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id: "greeting".to_string(),
            start_ms: 0,
            end_ms: 200,
        },
        "the acknowledgement carries the id this adapter chose and the model's own window"
    );

    // Keep the session busy for a while: a greeting sent twice would show up
    // as a second entry, and only time can prove it does not.
    for _ in 0..50 {
        session
            .audio_in
            .send(vec![0u8; 640])
            .await
            .expect("the session still takes audio");
    }
    eventually("the audio to arrive", || async {
        (mock.received_audio().await.len() >= 50).then_some(())
    })
    .await;

    let appends = mock.appends().await;
    assert_eq!(appends.len(), 1, "exactly one greeting, ever: {appends:?}");
    assert_eq!(appends[0]["type"], "session.commentary.append");
    assert_eq!(appends[0]["event_id"], "greeting");
    assert_eq!(
        appends[0]["delegation_id"],
        JsonValue::Null,
        "the greeting answers no delegation; the key is present and null"
    );
    assert_eq!(appends[0]["content"], GREETING);

    session.join.abort();
}

/// An empty greeting is the default, and it ships nothing: the model waits for
/// the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_greeting_sends_nothing() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_quiet".to_string(),
        actions: vec![LiveAction::Delay(Duration::from_secs(60))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 5_000, "");
    assert!(matches!(
        next_event(&mut session.events).await,
        DuplexEvent::Started { .. }
    ));
    for _ in 0..10 {
        session
            .audio_in
            .send(vec![1u8; 640])
            .await
            .expect("the session still takes audio");
    }
    eventually("the audio to arrive", || async {
        (mock.received_audio().await.len() >= 10).then_some(())
    })
    .await;

    assert!(
        mock.appends().await.is_empty(),
        "nothing is said before the caller says something"
    );
    session.join.abort();
}

/// And it waits for `session.started`: a peer that never confirms the session
/// gets no greeting, it gets a timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_never_started_gets_no_greeting() {
    let mock = MockGptLive::start(LiveScript {
        started: false,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let session = start(&mock, 250, GREETING);
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        matches!(verdict, Err(DuplexError::Timeout)),
        "an unanswered session.start is a timeout: {verdict:?}"
    );
    assert!(
        mock.appends().await.is_empty(),
        "the greeting follows `session.started` and nothing else"
    );
}
