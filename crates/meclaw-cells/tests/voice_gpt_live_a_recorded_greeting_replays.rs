//! Wave w1, strand liveaufnahme (GH #1610) -- a recorded `gpt-live-1.1` session
//! replays in the mock, and a recording that no longer matches what the test
//! would send today is refused as stale instead of passing or failing quietly.
//!
//! The recording (`fixtures/live/greeting_gpt_live_1_1.jsonl`) is one real
//! greeting session: the first line is its header (model, protocol, hash of
//! the opening frames the client sent), every further line one server event
//! with its gap to the one before. Output audio is kept as a duration only and
//! comes back as silence of the same length, so the replay carries what the
//! model said and when, never what it sounded like.
//!
//! The opening frames of that session live in
//! `fixtures/live/greeting_opening_gpt_live_1_1.json`, which the live recorder
//! in `gpt_live_live_frames.rs` sends and which a test below holds equal to the
//! adapter's own start frame (GH #1632): change one byte of them and the
//! recording is stale.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::voice::contract::AudioFormat;
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::{session_start_frame, session_url};
use meclaw_testing::mock_gpt_live::{
    LiveScript, MockGptLive, ProviderIds, RecordingError, RecordingHeader, parse_recording,
};
use meclaw_testing::mock_openai_realtime::b64_decode;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The frames the live recorder opens its session with: the adapter's own start
/// frame on today's default model, the greeting as the last `developer` message.
const OPENING: &str = include_str!("fixtures/live/greeting_opening_gpt_live_1_1.json");

/// One real greeting session of `gpt-live-1.1`.
const RECORDING: &str = include_str!("fixtures/live/greeting_gpt_live_1_1.jsonl");

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

fn opening() -> Vec<JsonValue> {
    serde_json::from_str(OPENING).expect("the opening fixture parses")
}

/// The header a recording made today would carry.
fn today() -> RecordingHeader {
    RecordingHeader::for_opening(&opening(), &session_url(""))
}

/// The script of `recording`, or the refusal: a stale recording is a message
/// that names what moved, never a quiet replay of an old session.
fn script_or_refuse(recording: &str) -> LiveScript {
    match LiveScript::from_recording(recording, &today()) {
        Ok(script) => script,
        Err(refusal) => panic!("{refusal}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recorded_greeting_replays_in_the_mock() {
    let (header, recorded) = parse_recording(RECORDING).expect("the recording parses");
    let expected: Vec<_> = recorded
        .iter()
        .filter(|e| e.kind != "session.started")
        .collect();
    assert!(
        expected
            .iter()
            .any(|e| e.kind == "session.output_audio.delta" && e.audio_ms.is_some()),
        "the recording holds the spoken greeting"
    );
    assert!(
        expected
            .iter()
            .any(|e| e.kind == "session.output_transcript.delta"),
        "the recording holds what the model said"
    );

    let mock = MockGptLive::start(script_or_refuse(RECORDING))
        .await
        .expect("bind fake");
    let url = format!("{}{}", mock.base_url(), session_url(""));
    let (ws, _) = tokio::time::timeout(FAILURE_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .expect("the handshake answered")
        .expect("the handshake succeeded");
    let (mut write, mut read) = ws.split();
    let frames = opening();
    write
        .send(WsMessage::Text(frames[0].to_string().into()))
        .await
        .expect("session.start goes out");

    let mut received: Vec<JsonValue> = Vec::new();
    while received.len() < expected.len() {
        let msg = tokio::time::timeout(FAILURE_TIMEOUT, read.next())
            .await
            .expect("the replay goes on")
            .expect("the socket stays open")
            .expect("the frame is readable");
        let WsMessage::Text(text) = msg else { continue };
        let event: JsonValue = serde_json::from_str(&text).expect("a server event is JSON");
        if event["type"] == "session.started" {
            for frame in &frames[1..] {
                write
                    .send(WsMessage::Text(frame.to_string().into()))
                    .await
                    .expect("the opening goes out");
            }
            continue;
        }
        received.push(event);
    }

    for (got, want) in received.iter().zip(&expected) {
        assert_eq!(
            got["type"],
            want.kind.as_str(),
            "the events come in recorded order"
        );
        let mut got_rest = got.clone();
        let delta = got_rest.as_object_mut().and_then(|o| {
            if want.audio_ms.is_some() {
                o.remove("delta")
            } else {
                None
            }
        });
        assert_eq!(got_rest, want.payload, "the payload comes back verbatim");
        if let (Some(ms), Some(delta)) = (want.audio_ms, delta) {
            let silence = b64_decode(delta.as_str().expect("audio is base64")).expect("decodes");
            assert!(
                silence.iter().all(|b| *b == 0),
                "recorded audio replays as silence"
            );
            // PCM16 mono at the rate the recording's own header names.
            let replayed_ms = silence.len() as f64 * 1000.0 / (header.audio_rate as f64 * 2.0);
            assert!(
                (replayed_ms - ms).abs() < 0.05,
                "the silence is as long as the recorded audio: {replayed_ms} vs {ms}"
            );
        }
    }
}

/// GH #1646: no recording this repository ships carries an id of the provider.
/// A live session id has the shape of another vendor's access token, and a
/// secret scanner of a public tree reads it as one.
#[test]
fn no_shipped_recording_carries_a_provider_id() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).expect("the fixtures are listed") {
        let path = entry.expect("a fixture").path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a recording is readable");
        let (_, events) = parse_recording(&text).expect("a recording parses");
        let mut ids = ProviderIds::default();
        for (n, event) in events.iter().enumerate() {
            let mut scrubbed = event.payload.clone();
            ids.scrub(&mut scrubbed);
            // Never the payload itself: the message would carry the id.
            assert!(
                scrubbed == event.payload,
                "{}: event {n} ({}) carries an id of the provider",
                path.display(),
                event.kind
            );
            read += 1;
        }
    }
    assert!(read > 0, "a recording was read");
}

/// GH #1632: what is recorded is what the adapter sends, not a frame somebody
/// wrote beside it.
#[test]
fn the_recording_opens_like_the_adapter_on_its_default_model() {
    let frames = opening();
    let [start] = frames.as_slice() else {
        panic!("the greeting is part of the start: {frames:?}");
    };
    let greeting = start["session"]["input"][0]["content"][0]["text"].clone();
    let params: GptLiveParams = serde_json::from_value(serde_json::json!({
        "api_key": "test-key",
        "instructions": start["session"]["instructions"],
        "greeting": greeting,
    }))
    .expect("params parse");
    let sent = session_start_frame(&params, AudioFormat::pcm16_mono(24_000), None, true);
    assert_eq!(*start, sent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recording_with_another_header_is_refused_as_stale() {
    let (header, _) = parse_recording(RECORDING).expect("the recording parses");
    let moved = RECORDING.replacen(
        &format!("\"model\":\"{}\"", header.model),
        "\"model\":\"gpt-live-0\"",
        1,
    );
    assert_ne!(moved, RECORDING, "the header names its model");
    let refusal = LiveScript::from_recording(&moved, &today())
        .expect_err("a recording of another model is not replayed");
    assert!(matches!(refusal, RecordingError::Stale(_)), "{refusal:?}");
    let text = refusal.to_string();
    assert!(text.starts_with("stale recording: "), "{text}");
    assert!(
        text.contains("model"),
        "the refusal names what moved: {text}"
    );

    // The same recording against an opening that changed by one word.
    let mut frames = opening();
    frames[0]["session"]["instructions"] = "Greet the caller now, two sentences.".into();
    let changed = RecordingHeader::for_opening(&frames, &session_url(""));
    let refusal = LiveScript::from_recording(RECORDING, &changed)
        .expect_err("a changed opening makes the recording stale");
    assert!(refusal.to_string().contains("config"), "{refusal}");
}
