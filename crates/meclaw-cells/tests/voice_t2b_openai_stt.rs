//! Wave voice-cell (t2b): the OpenAI Realtime transcription adapter against the
//! hermetic fake. No key, no network, no assumption that is not either in the
//! documentation or a knob of the fake.

use meclaw_cells::voice::contract::{ProviderTimeouts, SttError, SttEvent, SttProvider};
use meclaw_cells::voice::params::OpenAiSttParams;
use meclaw_cells::voice::providers::openai_stt::OpenAiTranscriptionStt;
use meclaw_colony::{ColonyMsg, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::mock_openai_realtime::{MockOpenAiRealtime, OpenAiRealtimeScript, b64_decode};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

fn params(base_url: &str, turn_detection: &str) -> OpenAiSttParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "model": "gpt-live-transcribe",
        "language": "de",
        "turn_detection": turn_detection,
        "base_url": base_url,
    }))
    .expect("params parse")
}

fn timeouts(external_ms: u64, idle_ms: u64) -> ProviderTimeouts {
    ProviderTimeouts {
        external: Duration::from_millis(external_ms),
        idle: Duration::from_millis(idle_ms),
    }
}

/// A running session: its audio end, its event end and the join handle of the
/// session future.
type Session = (
    mpsc::Sender<Vec<u8>>,
    mpsc::Receiver<SttEvent>,
    tokio::task::JoinHandle<Result<(), SttError>>,
);

/// Starts a session against `mock` with server-side VAD.
fn start_session(mock: &MockOpenAiRealtime, external_ms: u64, idle_ms: u64) -> Session {
    start_session_with(mock, external_ms, idle_ms, "server_vad")
}

/// Starts a session against `mock` with an explicit turn-detection setting.
fn start_session_with(
    mock: &MockOpenAiRealtime,
    external_ms: u64,
    idle_ms: u64,
    turn_detection: &str,
) -> Session {
    let (audio_tx, audio_rx) = mpsc::channel(16);
    let (events_tx, events_rx) = mpsc::channel(64);
    let provider = OpenAiTranscriptionStt::new(params(&mock.base_url(), turn_detection))
        .with_timeouts(timeouts(external_ms, idle_ms));
    let session = provider.run_session(
        provider.input_format(),
        audio_rx,
        events_tx,
        IoLivenessMark::disabled(),
    );
    (audio_tx, events_rx, tokio::spawn(session))
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

async fn next_event(events: &mut mpsc::Receiver<SttEvent>) -> SttEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for an stt event")
        .expect("event channel closed early")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_update_carries_model_language_and_vad() {
    let mock = MockOpenAiRealtime::start(OpenAiRealtimeScript::new())
        .await
        .expect("bind fake");
    let (_audio, _events, session) = start_session(&mock, 5_000, 30_000);

    let update: JsonValue = eventually("session.update", || mock.session_update()).await;
    assert_eq!(update["session"]["type"], "transcription");
    let input = &update["session"]["audio"]["input"];
    assert_eq!(input["format"]["type"], "audio/pcm");
    assert_eq!(input["format"]["rate"], 24_000);
    assert_eq!(input["transcription"]["model"], "gpt-live-transcribe");
    assert_eq!(input["transcription"]["languages"], json!(["de"]));
    assert_eq!(input["turn_detection"]["type"], "server_vad");

    // The credential travels as a bearer header, never as a query parameter.
    assert!(mock.authorization_is_bearer().await);
    assert!(
        !mock.received_query().await.contains_key("api_key"),
        "the key must not appear in the query string"
    );
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_url_is_used_verbatim() {
    // R-V10: the adapter appends its own path to whatever base it was given, so
    // any OpenAI-compatible endpoint is reachable by pointing `base_url` at it.
    let mock = MockOpenAiRealtime::start(OpenAiRealtimeScript::new())
        .await
        .expect("bind fake");
    let (_audio, _events, session) = start_session(&mock, 5_000, 30_000);

    eventually("the upgrade", || async {
        let path = mock.received_path().await;
        (!path.is_empty()).then_some(path)
    })
    .await;
    assert_eq!(mock.received_path().await, "/v1/realtime");
    assert!(
        mock.received_query().await.is_empty(),
        "the session type travels in session.update, not in the query (R-V18)"
    );
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_is_base64_appended() {
    let script = OpenAiRealtimeScript::new()
        .require_audio_bytes(3_200)
        .completed("item_1", "hallo")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    let first: Vec<u8> = (0..1_600u32).map(|i| (i % 251) as u8).collect();
    let second: Vec<u8> = (0..1_600u32).map(|i| (i % 97) as u8).collect();
    audio.send(first.clone()).await.expect("send audio");
    audio.send(second.clone()).await.expect("send audio");

    // The script only replays once the required bytes arrived, so receiving the
    // completed event already proves the append reached the far side.
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::EndOfTurn {
            text: "hallo".to_string()
        }
    );
    assert_eq!(mock.received_audio_bytes().await, 3_200);

    let appends: Vec<Vec<u8>> = mock
        .client_events()
        .await
        .into_iter()
        .filter(|e| e["type"] == "input_audio_buffer.append")
        .filter_map(|e| e["audio"].as_str().and_then(b64_decode))
        .collect();
    assert_eq!(
        appends,
        vec![first, second],
        "each chunk must arrive base64-encoded and byte-identical"
    );
    let _ = session.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delta_is_partial_completed_is_end_of_turn() {
    let script = OpenAiRealtimeScript::new()
        .delta("item_1", "hal")
        .delta("item_1", "lo")
        .completed("item_1", "hallo")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "hal".to_string(),
            eager: false
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "hallo".to_string(),
            eager: false
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::EndOfTurn {
            text: "hallo".to_string()
        }
    );
    assert_eq!(next_event(&mut events).await, SttEvent::Closed);
    assert!(matches!(session.await, Ok(Ok(()))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speech_started_maps() {
    let script = OpenAiRealtimeScript::new()
        .delta("item_1", "stale")
        .speech_started()
        .delta("item_2", "neu")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "stale".to_string(),
            eager: false
        }
    );
    assert_eq!(next_event(&mut events).await, SttEvent::SpeechStarted);
    // The new turn is a new item, so it starts from its own empty buffer —
    // `speech_started` does not clear anything, the item scoping does it.
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "neu".to_string(),
            eager: false
        }
    );
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transcription_failure_is_a_warning_and_keeps_the_session() {
    let script = OpenAiRealtimeScript::new()
        .failed("item_1", "audio too short")
        .delta("item_2", "go on")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Warning {
            detail: "audio too short".to_string()
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "go on".to_string(),
            eager: false
        }
    );
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn top_level_error_is_a_protocol_error() {
    let script = OpenAiRealtimeScript::new().error("unknown model");
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, _events, session) = start_session(&mock, 5_000, 30_000);

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    match result {
        Err(SttError::Protocol(detail)) => assert_eq!(detail, "unknown model"),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_close_yields_closed_event() {
    let mock = MockOpenAiRealtime::start(OpenAiRealtimeScript::new().close())
        .await
        .expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(next_event(&mut events).await, SttEvent::Closed);
    assert!(matches!(session.await, Ok(Ok(()))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_audio_end_commits_the_buffer_without_endpointing() {
    let script = OpenAiRealtimeScript::new().require_audio_bytes(640).close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (audio, _events, session) = start_session_with(&mock, 5_000, 30_000, "none");

    audio.send(vec![1u8; 640]).await.expect("send audio");
    // The connection dropped its audio end: whatever is still buffered on the
    // far side has to be committed, or the last turn is never transcribed.
    drop(audio);

    eventually("input_audio_buffer.commit", || async {
        mock.client_events()
            .await
            .into_iter()
            .find(|e| e["type"] == "input_audio_buffer.commit")
    })
    .await;
    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    assert!(result.is_ok(), "expected a clean end, got {result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_timeout_is_stt_timeout() {
    let script = OpenAiRealtimeScript::new().with_accept_delay(Duration::from_secs(10));
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    // 200 ms against a handshake that is held for ten seconds: the A-timeout is
    // the only thing that can end this wait.
    let (_audio, _events, session) = start_session(&mock, 200, 30_000);

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    assert!(
        matches!(result, Err(SttError::Timeout)),
        "expected a timeout, got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_401_is_auth_error() {
    let script = OpenAiRealtimeScript::new().with_upgrade_status(401);
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, _events, session) = start_session(&mock, 5_000, 30_000);

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    match result {
        Err(SttError::Auth(detail)) => assert!(
            detail.contains("401"),
            "the verdict should name the status, got {detail}"
        ),
        other => panic!("expected an auth error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_deadline_fires_while_audio_still_flows() {
    // The regression this pins: an idle deadline built as
    // `timeout(idle, read.next())` INSIDE the `select!` is thrown away and
    // rebuilt every time the audio arm wins, so a talking client against a
    // silent provider would never trip it — the one case it exists for.
    let script = OpenAiRealtimeScript::new().delay_ms(60_000);
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (audio, _events, session) = start_session(&mock, 5_000, 200);

    let pump = tokio::spawn(async move {
        // Keep the audio arm winning for far longer than the idle deadline.
        for _ in 0..200 {
            if audio.send(vec![0u8; 640]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    pump.abort();
    match result {
        Err(SttError::Closed(detail)) => assert!(
            detail.contains("idle"),
            "the verdict should name the idle deadline, got {detail}"
        ),
        other => panic!("expected a closed-on-idle error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_provider_frame_marks_liveness() {
    let script = OpenAiRealtimeScript::new()
        .delta("item_1", "hallo")
        .completed("item_1", "hallo")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");

    let (colony_tx, mut colony_rx) = mpsc::channel::<ColonyMsg>(16);
    let (_audio_tx, audio_rx) = mpsc::channel(16);
    let (events_tx, mut events) = mpsc::channel(64);
    let provider = OpenAiTranscriptionStt::new(params(&mock.base_url(), "server_vad"))
        .with_timeouts(timeouts(5_000, 30_000));
    let session = tokio::spawn(provider.run_session(
        provider.input_format(),
        audio_rx,
        events_tx,
        IoLivenessMark::new(Path::new("/member/channels/voice"), Some(colony_tx)),
    ));

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "hallo".to_string(),
            eager: false
        }
    );
    let mark = tokio::time::timeout(FAILURE_TIMEOUT, colony_rx.recv())
        .await
        .expect("timed out waiting for a liveness mark")
        .expect("liveness channel closed");
    match mark {
        ColonyMsg::IoLiveness { path, at } => {
            assert_eq!(path.as_str(), "/member/channels/voice");
            assert!(at.is_some(), "a completed round trip must carry its time");
        }
        _ => panic!("a round trip must be reported as ColonyMsg::IoLiveness"),
    }
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_socket_ends_the_session() {
    // Nothing is scripted after the greeting: the idle deadline is the only
    // thing that can end this session.
    let script = OpenAiRealtimeScript::new().delay_ms(60_000);
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, _events, session) = start_session(&mock, 5_000, 150);

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    match result {
        Err(SttError::Closed(detail)) => assert!(
            detail.contains("idle"),
            "the verdict should name the idle deadline, got {detail}"
        ),
        other => panic!("expected a closed-on-idle error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_commit_while_a_vad_is_running() {
    // The mirror image of the test above: with the service doing the
    // endpointing, a hand-written commit would cut the turn it is measuring.
    let script = OpenAiRealtimeScript::new().require_audio_bytes(640).close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (audio, _events, session) = start_session(&mock, 5_000, 30_000);

    audio.send(vec![1u8; 640]).await.expect("send audio");
    drop(audio);

    let result = tokio::time::timeout(FAILURE_TIMEOUT, session)
        .await
        .expect("session did not end")
        .expect("session task panicked");
    assert!(result.is_ok(), "expected a clean end, got {result:?}");
    let committed = mock
        .client_events()
        .await
        .iter()
        .any(|e| e["type"] == "input_audio_buffer.commit");
    assert!(!committed, "a running VAD owns the turn boundary, not us");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interleaved_items_keep_their_own_partials() {
    // Two items overlap on the wire; one shared buffer would splice them.
    let script = OpenAiRealtimeScript::new()
        .delta("item_1", "gu")
        .delta("item_2", "ha")
        .delta("item_1", "ten")
        .completed("item_1", "guten")
        .delta("item_2", "llo")
        .delta("item_1", "spaet")
        .completed("item_2", "hallo")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    let expected = vec![
        SttEvent::Partial {
            text: "gu".to_string(),
            eager: false,
        },
        SttEvent::Partial {
            text: "ha".to_string(),
            eager: false,
        },
        SttEvent::Partial {
            text: "guten".to_string(),
            eager: false,
        },
        SttEvent::EndOfTurn {
            text: "guten".to_string(),
        },
        SttEvent::Partial {
            text: "hallo".to_string(),
            eager: false,
        },
        // The late delta for the finished item_1 is dropped, not appended.
        SttEvent::EndOfTurn {
            text: "hallo".to_string(),
        },
        SttEvent::Closed,
    ];
    for want in expected {
        assert_eq!(next_event(&mut events).await, want);
    }
    assert!(matches!(session.await, Ok(Ok(()))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speech_stopped_produces_nothing() {
    let script = OpenAiRealtimeScript::new()
        .speech_stopped()
        .delta("item_1", "afterwards")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    // The next event after the ignored one is the delta, not anything derived
    // from `speech_stopped`.
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "afterwards".to_string(),
            eager: false
        }
    );
    session.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_after_session_updated_is_a_warning_and_the_session_runs_on() {
    let script = OpenAiRealtimeScript::new()
        .session_updated()
        .error("unknown parameter")
        .delta("item_1", "go on")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Warning {
            detail: "unknown parameter".to_string()
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "go on".to_string(),
            eager: false
        }
    );
    assert_eq!(next_event(&mut events).await, SttEvent::Closed);
    assert!(matches!(session.await, Ok(Ok(()))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_without_session_updated_still_survives_a_later_error() {
    // R-V10: an OpenAI-compatible endpoint that never confirms the session but
    // does transcribe is running one; a later error must not kill the stream.
    let script = OpenAiRealtimeScript::new()
        .delta("item_1", "hallo")
        .error("rate limited")
        .delta("item_2", "go on")
        .close();
    let mock = MockOpenAiRealtime::start(script).await.expect("bind fake");
    let (_audio, mut events, session) = start_session(&mock, 5_000, 30_000);

    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "hallo".to_string(),
            eager: false
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Warning {
            detail: "rate limited".to_string()
        }
    );
    assert_eq!(
        next_event(&mut events).await,
        SttEvent::Partial {
            text: "go on".to_string(),
            eager: false
        }
    );
    assert_eq!(next_event(&mut events).await, SttEvent::Closed);
    assert!(matches!(session.await, Ok(Ok(()))));
}
