//! Welle Live, L2a -- voice_duplex_gpt_live_maps_every_documented_event.
//!
//! One fixture per documented server event yields exactly one `DuplexEvent`,
//! and an event this adapter does not know is ignored with a debug line rather
//! than breaking a live call.
//!
//! The list is read off `rohbericht-steuerung.md` section D and the events
//! measured in S0, and it is a TABLE rather than a test per event on purpose:
//! what this file locks is not that each arm works but that the SET is closed
//! -- a fixture that silently produced two events, or none, would pass every
//! per-event assertion and fail here.
//!
//! `session.output_audio.delta` is the one documented event missing from the
//! table: it leaves through `audio_out`, not through the event channel, and
//! `voice_duplex_gpt_live_decodes_output_audio` is where it is read.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts, Speaker,
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
    #[allow(dead_code)]
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    #[allow(dead_code)]
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn start(mock: &MockGptLive) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_maps_every_documented_event() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_map".to_string(),
        actions: vec![
            LiveAction::Transcript {
                speaker: "user",
                delta: "good ",
                start_ms: 100,
                end_ms: 300,
            },
            // An empty fragment is not a fragment.
            LiveAction::Transcript {
                speaker: "user",
                delta: "",
                start_ms: 300,
                end_ms: 300,
            },
            LiveAction::Transcript {
                speaker: "assistant",
                delta: "day to you",
                start_ms: 400,
                end_ms: 900,
            },
            LiveAction::Delegation {
                id: "dlg_1",
                offset_ms: 950,
            },
            // A delegation aimed anywhere but at us is somebody else's work.
            LiveAction::Send(json!({
                "type": "session.delegation.created",
                "event_id": "event_other",
                "offset_ms": 960,
                "delegation": { "id": "dlg_2", "type": "delegation", "target": "server" }
            })),
            LiveAction::Send(json!({
                "type": "session.commentary.appended",
                "event_id": "event_ack_1",
                "client_event_id": "ev_a",
                "start_ms": 9_400,
                "end_ms": 9_600
            })),
            LiveAction::Send(json!({
                "type": "session.thinking.appended",
                "event_id": "event_ack_2",
                "client_event_id": "ev_b",
                "start_ms": 9_600,
                "end_ms": 9_800
            })),
            LiveAction::Send(json!({
                "type": "session.instructions.appended",
                "event_id": "event_ack_3",
                "client_event_id": "ev_c",
                "start_ms": 9_800,
                "end_ms": 10_000
            })),
            LiveAction::Usage {
                seconds: 30.0,
                ratio: Some(0.07),
            },
            // The meter without a window reading -- it is optional on the wire.
            LiveAction::Usage {
                seconds: 45.0,
                ratio: None,
            },
            LiveAction::Send(json!({
                "type": "session.input_audio.muted",
                "event_id": "event_muted"
            })),
            LiveAction::Send(json!({
                "type": "session.input_audio.unmuted",
                "event_id": "event_unmuted"
            })),
            // After `session.started` an error is about one item, not about the
            // session -- the session is demonstrably running (R-V17).
            LiveAction::Send(json!({
                "type": "error",
                "event_id": "event_err",
                "error": { "message": "one append was too long" }
            })),
            // Everything below is documented and carries nothing this adapter
            // translates. Each has to yield NOTHING.
            LiveAction::Send(json!({ "type": "session.updated", "event_id": "e1" })),
            LiveAction::Send(json!({ "type": "info", "event_id": "e2", "message": "hello" })),
            LiveAction::Send(json!({ "type": "response.event", "event_id": "e3" })),
            LiveAction::Send(json!({ "type": "transport.reconnecting", "event_id": "e4" })),
            LiveAction::Send(json!({ "type": "something.unheard.of", "event_id": "e5" })),
            LiveAction::CloseWith {
                reason: "drained",
                usage_seconds: 61.5,
            },
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock);

    // The fake replays in order and the channel keeps it, so the whole session
    // is one comparison: every event, exactly once, and nothing else.
    let mut seen = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(FAILURE_TIMEOUT, session.events.recv()).await {
        let last = matches!(event, DuplexEvent::Closed { .. });
        seen.push(event);
        if last {
            break;
        }
    }

    let expected = vec![
        DuplexEvent::Started {
            session_id: "sess_map".to_string(),
        },
        DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "good ".to_string(),
            start_ms: 100,
            end_ms: 300,
        },
        DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            delta: "day to you".to_string(),
            start_ms: 400,
            end_ms: 900,
        },
        DuplexEvent::DelegationCreated {
            delegation_id: "dlg_1".to_string(),
            offset_ms: 950,
        },
        DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id: "ev_a".to_string(),
            start_ms: 9_400,
            end_ms: 9_600,
        },
        DuplexEvent::Appended {
            kind: AppendKind::Thinking,
            event_id: "ev_b".to_string(),
            start_ms: 9_600,
            end_ms: 9_800,
        },
        DuplexEvent::Appended {
            kind: AppendKind::Instructions,
            event_id: "ev_c".to_string(),
            start_ms: 9_800,
            end_ms: 10_000,
        },
        DuplexEvent::Usage {
            seconds: 30.0,
            usage_ratio: Some(0.07),
        },
        DuplexEvent::Usage {
            seconds: 45.0,
            usage_ratio: None,
        },
        DuplexEvent::Muted,
        DuplexEvent::Unmuted,
        DuplexEvent::Warning {
            detail: "one append was too long".to_string(),
        },
        DuplexEvent::Closed {
            reason: "drained".to_string(),
            usage_seconds: 61.5,
        },
    ];
    assert_eq!(
        seen, expected,
        "one documented event is one duplex event, and an unknown one is none"
    );

    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a provider that closed in an orderly way is not a failure: {verdict:?}"
    );
}
