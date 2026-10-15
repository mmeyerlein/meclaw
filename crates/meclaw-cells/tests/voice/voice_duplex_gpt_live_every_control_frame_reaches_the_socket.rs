//! Welle Live, L2a -- voice_duplex_gpt_live_every_control_frame_reaches_the_socket.
//!
//! Each of the four `DuplexControl` items is one frame on the wire, in the
//! order it was handed over: an append per kind, a mute, an unmute, a close.
//!
//! The kinds are not interchangeable -- `commentary` is spoken, `thinking` is
//! not, `instructions` change who the model is -- and a mute that does not
//! arrive is a caller who is still heard while the colony believes it put the
//! line on hold. Both were readable only in the source until this file; so was
//! OR-L.L2a.4, the ruling that a mute frame carries nothing but its type
//! because the contract names no `event_id` for it.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts,
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

/// The `type` of every frame the fake saw, in arrival order.
fn kinds(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            e.get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("<untyped>")
                .to_string()
        })
        .collect()
}

/// Hands one item to the session, and says so when nobody is there to take it.
async fn send(control_tx: &mpsc::Sender<DuplexControl>, item: DuplexControl) {
    control_tx
        .send(item)
        .await
        .expect("the session was still taking guidance");
}

/// The next event, or a failure that names what was waited for.
async fn next_event(events_rx: &mut mpsc::Receiver<DuplexEvent>, what: &str) -> DuplexEvent {
    tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for `{what}`"))
        .unwrap_or_else(|| panic!("the session ended before `{what}`"))
}

/// The next event is the acknowledgement of exactly this append.
async fn expect_appended(
    events_rx: &mut mpsc::Receiver<DuplexEvent>,
    want_kind: AppendKind,
    want_event_id: &str,
) {
    let event = next_event(events_rx, "Appended").await;
    let DuplexEvent::Appended { kind, event_id, .. } = &event else {
        panic!("{want_kind:?} was acknowledged, not {event:?}");
    };
    assert_eq!(
        *kind, want_kind,
        "the channel the colony asked for is the channel that answered"
    );
    assert_eq!(
        event_id, want_event_id,
        "the acknowledgement carries the colony's own id back"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_every_control_frame_reaches_the_socket() {
    // The fake answers each guidance frame in turn and then waits out the
    // test; the close is answered by its reader wherever the script stands.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_control".to_string(),
        actions: vec![
            LiveAction::ExpectAppend {
                kind: "commentary",
                contains: "the line is quiet",
            },
            LiveAction::ExpectAppend {
                kind: "thinking",
                contains: "he asked that twice",
            },
            LiveAction::ExpectAppend {
                kind: "instructions",
                contains: "speak more slowly",
            },
            LiveAction::ExpectMute,
            LiveAction::ExpectUnmute,
            LiveAction::Delay(Duration::from_secs(60)),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: Duration::from_secs(600),
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
    let join = tokio::spawn(future);

    assert!(
        matches!(
            tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv())
                .await
                .expect("timed out waiting for `Started`"),
            Some(DuplexEvent::Started { .. })
        ),
        "the session opened before anybody steered it"
    );

    // One item at a time, each waited out: the fake answers each frame with
    // its acknowledgement, and the close is answered wherever the script
    // stands -- so sending all six at once would race the close past the
    // acknowledgements and prove nothing about the four in between.
    send(
        &control_tx,
        DuplexControl::Append {
            kind: AppendKind::Commentary,
            event_id: "ev_fact".to_string(),
            delegation_id: None,
            content: "the line is quiet".to_string(),
        },
    )
    .await;
    expect_appended(&mut events_rx, AppendKind::Commentary, "ev_fact").await;

    send(
        &control_tx,
        DuplexControl::Append {
            kind: AppendKind::Thinking,
            event_id: "ev_context".to_string(),
            delegation_id: Some("deleg_1".to_string()),
            content: "he asked that twice".to_string(),
        },
    )
    .await;
    expect_appended(&mut events_rx, AppendKind::Thinking, "ev_context").await;

    send(
        &control_tx,
        DuplexControl::Append {
            kind: AppendKind::Instructions,
            event_id: "ev_correction".to_string(),
            delegation_id: None,
            content: "speak more slowly".to_string(),
        },
    )
    .await;
    expect_appended(&mut events_rx, AppendKind::Instructions, "ev_correction").await;

    send(&control_tx, DuplexControl::Mute).await;
    assert!(
        matches!(
            next_event(&mut events_rx, "Muted").await,
            DuplexEvent::Muted
        ),
        "the far side muted the line the colony put on hold"
    );
    send(&control_tx, DuplexControl::Unmute).await;
    assert!(
        matches!(
            next_event(&mut events_rx, "Unmuted").await,
            DuplexEvent::Unmuted
        ),
        "and took it off hold again"
    );

    send(&control_tx, DuplexControl::Close).await;
    assert!(
        matches!(
            next_event(&mut events_rx, "Closed").await,
            DuplexEvent::Closed { .. }
        ),
        "a requested close is answered"
    );

    let client_events = mock.client_events().await;
    assert_eq!(
        kinds(&client_events),
        vec![
            "session.start",
            "session.commentary.append",
            "session.thinking.append",
            "session.instructions.append",
            "session.input_audio.mute",
            "session.input_audio.unmute",
            "session.close",
        ],
        "one control item is one frame, in the order it was handed over"
    );

    // OR-L.L2a.4: the contract names no `event_id` for the two audio switches,
    // and an adapter that invents one is inventing a field.
    for name in ["session.input_audio.mute", "session.input_audio.unmute"] {
        let frame = client_events
            .iter()
            .find(|e| e.get("type").and_then(|t| t.as_str()) == Some(name))
            .expect("the frame arrived");
        assert_eq!(
            frame.as_object().map(|o| o.len()),
            Some(1),
            "{name} is its type and nothing else: {frame}"
        );
    }

    // And an append carries what the colony gave it, `delegation_id` included
    // as an explicit null when there is none to answer.
    let commentary = client_events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("session.commentary.append"))
        .expect("the append arrived");
    assert_eq!(
        commentary.get("event_id").and_then(|v| v.as_str()),
        Some("ev_fact")
    );
    assert_eq!(
        commentary.get("delegation_id"),
        Some(&serde_json::Value::Null)
    );
    let thinking = client_events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("session.thinking.append"))
        .expect("the append arrived");
    assert_eq!(
        thinking.get("delegation_id").and_then(|v| v.as_str()),
        Some("deleg_1"),
        "a delegation the colony named travels with the append"
    );

    assert!(
        mock.closed_requested().await,
        "`Close` asked the far side rather than dropping the socket"
    );
    let verdict: Result<(), DuplexError> = tokio::time::timeout(FAILURE_TIMEOUT, join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a requested close is not a failure: {verdict:?}"
    );
}
