//! Wave voice-cell, strand t2a — the Deepgram Flux adapter against the fake.
//!
//! Every test here proves one clause of the provider contract against a
//! scripted fake of the real wire (`meclaw_testing::mock_deepgram`), so the
//! suite needs no credential and no network. What the fake cannot answer, no
//! test asserts.

use meclaw_cells::voice::contract::{ProviderTimeouts, SttError, SttEvent, SttProvider};
use meclaw_cells::voice::params::DeepgramParams;
use meclaw_cells::voice::providers::deepgram::DeepgramFluxStt;
use meclaw_colony::{ColonyMsg, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};
use std::time::Duration;
use tokio::sync::mpsc;

/// Params pointed at a fake. Everything else stays at its default, so the
/// defaults themselves are what the query assertions see.
fn params(base_url: &str) -> DeepgramParams {
    serde_json::from_value(serde_json::json!({
        "api_key": "test-credential-not-a-real-key",
        "base_url": base_url,
    }))
    .expect("deepgram params parse")
}

/// The same params plus a keyterm list.
fn params_with_keyterms(base_url: &str, keyterms: serde_json::Value) -> DeepgramParams {
    serde_json::from_value(serde_json::json!({
        "api_key": "test-credential-not-a-real-key",
        "base_url": base_url,
        "keyterms": keyterms,
    }))
    .expect("deepgram params parse")
}

/// Runs one session against `mock` with `stt` and returns once it ended, so a
/// query assertion sees a completed upgrade.
async fn run_one_session(stt: DeepgramFluxStt) {
    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));
    let _ = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("the scripted turn arrives");
    drop(audio_tx);
    let _ = tokio::time::timeout(Duration::from_secs(30), session).await;
}

/// A provider with test-sized deadlines.
fn provider(base_url: &str) -> DeepgramFluxStt {
    provider_with(base_url, Duration::from_secs(5), Duration::from_secs(30))
}

/// A provider whose deadlines the test picks — the timeout cases need them
/// short enough to fire well inside the 30 s failure-marker convention.
fn provider_with(base_url: &str, external: Duration, idle: Duration) -> DeepgramFluxStt {
    DeepgramFluxStt::new(params(base_url)).with_timeouts(ProviderTimeouts { external, idle })
}

/// Collects everything the session emitted until it ended.
async fn drain(rx: &mut mpsc::Receiver<SttEvent>) -> Vec<SttEvent> {
    let mut out = Vec::new();
    while let Some(event) = rx.recv().await {
        out.push(event);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connects_with_query_and_token() {
    let mock = MockDeepgram::start(DeepgramScript::new().turn_info("EndOfTurn", "hallo"))
        .await
        .expect("mock deepgram starts");
    let stt = provider(&mock.base_url());

    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));
    // The first event proves the socket is up and the query was seen.
    let _ = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("the scripted turn arrives");
    drop(audio_tx);
    let _ = tokio::time::timeout(Duration::from_secs(30), session).await;

    let query = mock.received_query();
    assert_eq!(
        query.get("model").map(String::as_str),
        Some("flux-general-multi"),
        "the multilingual model is the default, because the default language is not English"
    );
    assert_eq!(query.get("encoding").map(String::as_str), Some("linear16"));
    assert_eq!(query.get("sample_rate").map(String::as_str), Some("16000"));
    assert_eq!(query.get("eot_threshold").map(String::as_str), Some("0.7"));
    assert_eq!(
        query.get("eager_eot_threshold").map(String::as_str),
        Some("0.3")
    );
    assert_eq!(
        query.get("eot_timeout_ms").map(String::as_str),
        Some("5000")
    );
    assert_eq!(query.get("language_hint").map(String::as_str), Some("de"));
    // The header must be there; its value is never read, asserted or logged.
    assert!(
        mock.saw_authorization(),
        "the upgrade must carry an Authorization header"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwards_audio_and_maps_events() {
    let mock = MockDeepgram::start(
        DeepgramScript::new()
            .require_audio_bytes(3200)
            .turn_info("Update", "hal")
            .turn_info("EndOfTurn", "hallo"),
    )
    .await
    .expect("mock deepgram starts");
    let stt = provider(&mock.base_url());

    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));

    for _ in 0..5 {
        audio_tx.send(vec![0u8; 640]).await.expect("audio accepted");
    }
    let first = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("a partial arrives inside the failure-marker deadline")
        .expect("a partial arrives");
    let second = tokio::time::timeout(Duration::from_secs(30), events_rx.recv())
        .await
        .expect("a turn arrives inside the failure-marker deadline")
        .expect("a turn arrives");
    drop(audio_tx);
    let _ = tokio::time::timeout(Duration::from_secs(30), session).await;

    assert_eq!(
        first,
        SttEvent::Partial {
            text: "hal".to_string(),
            eager: false
        }
    );
    assert_eq!(
        second,
        SttEvent::EndOfTurn {
            text: "hallo".to_string()
        }
    );
    assert_eq!(
        mock.received_audio_bytes(),
        3200,
        "every chunk must reach the service unchanged and uncounted-for"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_of_turn_is_speech_started() {
    let events = run_script(DeepgramScript::new().turn_info("StartOfTurn", "hi")).await;
    assert_eq!(
        events,
        vec![
            SttEvent::SpeechStarted,
            SttEvent::Partial {
                text: "hi".to_string(),
                eager: false
            }
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eager_end_of_turn_is_eager_partial() {
    let events = run_script(DeepgramScript::new().turn_info("EagerEndOfTurn", "fertig")).await;
    assert_eq!(
        events,
        vec![SttEvent::Partial {
            text: "fertig".to_string(),
            eager: true
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turn_resumed_maps() {
    let events = run_script(
        DeepgramScript::new()
            .turn_info("EagerEndOfTurn", "fertig")
            .turn_info("TurnResumed", "fertig bitte"),
    )
    .await;
    assert_eq!(
        events,
        vec![
            SttEvent::Partial {
                text: "fertig".to_string(),
                eager: true
            },
            SttEvent::TurnResumed
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_audio_sends_close_stream_and_returns_ok() {
    let mock = MockDeepgram::start(DeepgramScript::new())
        .await
        .expect("mock deepgram starts");
    let stt = provider_with(
        &mock.base_url(),
        Duration::from_millis(300),
        Duration::from_secs(30),
    );

    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, _events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));
    audio_tx.send(vec![0u8; 640]).await.expect("audio accepted");
    drop(audio_tx);

    let result = tokio::time::timeout(Duration::from_secs(30), session)
        .await
        .expect("the session ends")
        .expect("the session task does not panic");
    assert!(result.is_ok(), "a clean end is not an error: {result:?}");
    assert!(
        mock.client_messages()
            .iter()
            .any(|m| m.contains("CloseStream")),
        "the adapter must tell Flux the stream is over, got {:?}",
        mock.client_messages()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_close_yields_closed_event() {
    let mock = MockDeepgram::start(DeepgramScript::new().close())
        .await
        .expect("mock deepgram starts");
    let stt = provider(&mock.base_url());

    let (_audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));

    let events = tokio::time::timeout(Duration::from_secs(30), drain(&mut events_rx))
        .await
        .expect("the session ends");
    let result = session.await.expect("the session task does not panic");
    assert_eq!(events, vec![SttEvent::Closed]);
    assert!(result.is_ok(), "a server-side close is not an error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_timeout_is_stt_timeout() {
    // The fake accepts the TCP connection and then sits on the upgrade, which
    // is what a stalled service looks like from the client's side.
    let mock =
        MockDeepgram::start(DeepgramScript::new().with_accept_delay(Duration::from_secs(10)))
            .await
            .expect("mock deepgram starts");
    let stt = provider_with(
        &mock.base_url(),
        Duration::from_millis(200),
        Duration::from_secs(30),
    );

    let (_audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, _events_rx) = mpsc::channel(8);
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()),
    )
    .await
    .expect("the operation timeout fires long before the test deadline");
    assert!(
        matches!(result, Err(SttError::Timeout)),
        "expected a timeout, got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_401_is_auth_error() {
    let mock = MockDeepgram::start(DeepgramScript::new().rejecting_with(401))
        .await
        .expect("mock deepgram starts");
    let stt = provider(&mock.base_url());

    let (_audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, _events_rx) = mpsc::channel(8);
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()),
    )
    .await
    .expect("the handshake fails quickly");
    match result {
        Err(SttError::Auth(detail)) => assert!(
            !detail.contains("test-credential"),
            "an error must never carry the credential"
        ),
        other => panic!("expected an auth error, got {other:?}"),
    }
}

/// A Flux socket that goes quiet while the client keeps talking must end the
/// session, not sit there. The regression this pins: a deadline rebuilt inside
/// the `select!` is reset by every outgoing chunk too, so a live audio stream
/// held a mute socket open forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mute_socket_ends_the_session_although_audio_keeps_flowing() {
    // Ten seconds of nothing from the service, fifty times the idle deadline.
    let mock = MockDeepgram::start(DeepgramScript::new().delay_ms(10_000))
        .await
        .expect("mock deepgram starts");
    let stt = provider_with(
        &mock.base_url(),
        Duration::from_secs(5),
        Duration::from_millis(200),
    );

    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, _events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));
    // Keep the audio side busy for well past the idle deadline.
    let pump = tokio::spawn(async move {
        for _ in 0..200 {
            if audio_tx.send(vec![0u8; 640]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let result = tokio::time::timeout(Duration::from_secs(30), session)
        .await
        .expect("the idle deadline fires long before the failure marker")
        .expect("the session task does not panic");
    pump.abort();
    match result {
        Err(SttError::Closed(reason)) => assert_eq!(reason, "idle"),
        other => panic!("expected an idle close, got {other:?}"),
    }
}

/// The liveness mark is the cell's only evidence that a provider socket is
/// answering, so it has to be set per received frame — not per connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_received_turn_marks_liveness() {
    // No `Connected` and no server-side close: the one `TurnInfo` is then the
    // only frame the session receives, so the mark count is unambiguous. The
    // session ends on its own, because dropping the audio sender sends
    // `CloseStream` and the drain is bounded by the operation timeout.
    let mock = MockDeepgram::start(
        DeepgramScript::new()
            .without_connected()
            .turn_info("EndOfTurn", "hallo"),
    )
    .await
    .expect("mock deepgram starts");
    let stt = provider_with(
        &mock.base_url(),
        Duration::from_millis(500),
        Duration::from_secs(30),
    );

    let (marks_tx, mut marks_rx) = mpsc::channel::<ColonyMsg>(8);
    let (audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(
        audio_rx,
        events_tx,
        IoLivenessMark::new(Path::new("/voice"), Some(marks_tx)),
    ));
    drop(audio_tx);
    let events = tokio::time::timeout(Duration::from_secs(30), drain(&mut events_rx))
        .await
        .expect("the session ends");
    let _ = session.await;

    assert_eq!(
        events,
        vec![SttEvent::EndOfTurn {
            text: "hallo".to_string()
        }]
    );
    match marks_rx.try_recv().expect("the received turn is marked") {
        ColonyMsg::IoLiveness { path, at } => {
            assert_eq!(path.as_str(), "/voice");
            assert!(at.is_some(), "a successful round trip carries its time");
        }
        // `ColonyMsg` carries no `Debug`, so the variant is the whole verdict.
        _ => panic!("expected the mark to arrive as ColonyMsg::IoLiveness"),
    }
    assert!(
        marks_rx.try_recv().is_err(),
        "one received frame is one mark"
    );
}

/// Runs a script with no audio and returns every event the session emitted.
async fn run_script(script: DeepgramScript) -> Vec<SttEvent> {
    let mock = MockDeepgram::start(script.close())
        .await
        .expect("mock deepgram starts");
    let stt = provider(&mock.base_url());
    let (_audio_tx, audio_rx) = mpsc::channel(8);
    let (events_tx, mut events_rx) = mpsc::channel(8);
    let session = tokio::spawn(stt.run_session(audio_rx, events_tx, IoLivenessMark::disabled()));
    let mut events = tokio::time::timeout(Duration::from_secs(30), drain(&mut events_rx))
        .await
        .expect("the session ends");
    let _ = session.await;
    // The trailing `Closed` belongs to the script's `close()`, not to the
    // mapping under test.
    if events.last() == Some(&SttEvent::Closed) {
        events.pop();
    }
    events
}

/// Keyterm prompting reaches the wire as a repeated parameter -- the shape the
/// service reads a list in. The fake's map view cannot see a repetition, so
/// this reads the pairs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyterms_reach_the_wire_as_repeated_parameters() {
    let mock = MockDeepgram::start(DeepgramScript::new().turn_info("EndOfTurn", "hallo"))
        .await
        .expect("mock deepgram starts");
    let stt = DeepgramFluxStt::new(params_with_keyterms(
        &mock.base_url(),
        serde_json::json!(["Egon", "meclaw core"]),
    ))
    .with_timeouts(ProviderTimeouts {
        external: Duration::from_secs(5),
        idle: Duration::from_secs(30),
    });
    run_one_session(stt).await;

    let terms: Vec<String> = mock
        .received_query_pairs()
        .into_iter()
        .filter(|(k, _)| k == "keyterm")
        .map(|(_, v)| v)
        .collect();
    assert_eq!(
        terms,
        vec!["Egon".to_string(), "meclaw%20core".to_string()],
        "two parameters, in order, and the phrase stays one of them"
    );
}

/// The default is no keyterms, and that must be the query the adapter always
/// sent: not one parameter more, not an empty one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_keyterms_the_query_is_unchanged() {
    let mock = MockDeepgram::start(DeepgramScript::new().turn_info("EndOfTurn", "hallo"))
        .await
        .expect("mock deepgram starts");
    run_one_session(provider(&mock.base_url())).await;

    let pairs: Vec<(String, String)> = mock.received_query_pairs();
    let expected: Vec<(String, String)> = [
        ("model", "flux-general-multi"),
        ("encoding", "linear16"),
        ("sample_rate", "16000"),
        ("eot_threshold", "0.7"),
        ("eager_eot_threshold", "0.3"),
        ("eot_timeout_ms", "5000"),
        ("language_hint", "de"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(
        pairs, expected,
        "keys, values and order: the query is the one from before keyterms existed"
    );
}
