//! Welle Live, L2a -- voice_duplex_gpt_live_decodes_output_audio.
//!
//! A `session.output_audio.delta` is base64-decoded (`voice/b64.rs`) and sent
//! on as one item; a delta that will not decode is a `Warning` and never the
//! end of a call.
//!
//! The second half is the one worth writing down: the caller is on the phone.
//! A frame the adapter cannot read is a moment of silence, and a moment of
//! silence is not a reason to hang up on someone.

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

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

struct Session {
    #[allow(dead_code)]
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    #[allow(dead_code)]
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

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

async fn next_event(events: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for a duplex event")
        .expect("event channel closed early")
}

async fn next_chunk(audio: &mut mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(FAILURE_TIMEOUT, audio.recv())
        .await
        .expect("timed out waiting for output audio")
        .expect("audio channel closed early")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_decodes_output_audio() {
    // Every byte value, so a decoder that quietly mangles the two
    // non-alphanumeric characters of the alphabet is caught here.
    let first: Vec<u8> = (0..=255u8).collect();
    let second: Vec<u8> = vec![0xFF, 0x0F, 0x80, 0x00, 0x7F, 0xFB];
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_audio".to_string(),
        actions: vec![
            LiveAction::SendAudio(first.clone()),
            // Not base64, and therefore not audio. The call goes on.
            LiveAction::Send(json!({
                "type": "session.output_audio.delta",
                "event_id": "event_broken",
                "delta": "this is not base64"
            })),
            LiveAction::SendAudio(second.clone()),
            LiveAction::Delay(Duration::from_secs(60)),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 5_000, 30_000);
    assert!(matches!(
        next_event(&mut session.events).await,
        DuplexEvent::Started { .. }
    ));

    assert_eq!(
        next_chunk(&mut session.audio_out).await,
        first,
        "a delta is decoded and handed on whole"
    );
    assert_eq!(
        next_chunk(&mut session.audio_out).await,
        second,
        "and the delta after the broken one still arrives"
    );

    match next_event(&mut session.events).await {
        DuplexEvent::Warning { detail } => {
            assert!(
                detail.contains("output_audio"),
                "the warning says which frame it was about: {detail}"
            );
        }
        other => panic!("an undecodable delta is a warning, not {other:?}"),
    }

    assert!(
        !session.join.is_finished(),
        "an undecodable delta ends nothing"
    );
    session.join.abort();
}
