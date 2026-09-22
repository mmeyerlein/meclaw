//! Welle Live, L1 — an append is answered, and the loopback mirrors it as
//! assistant text.
//!
//! Two contracts in one file, because the second one only exists for the first.
//!
//! **Answered**: every `Append` comes back as an `Appended` carrying the same
//! `event_id` and where it landed on the model's clock. That is what the colony
//! waits for before it considers guidance delivered — a live model takes an
//! append up in its own time, and the `appended` is the only word anyone gets
//! about when.
//!
//! **Mirrored**: the loopback also emits the content back as an assistant
//! transcript. It does that so the connection's `speak_end` heuristic (OR-L19)
//! can be measured with no model in the way: the heuristic waits for assistant
//! fragments to stop, and without a mirror there would be nothing to stop.
//!
//! And a mute is a mute: frames sent while the model's ear is closed do not
//! come back, and the ones after the unmute do.

use meclaw_cells::voice::contract::{
    AppendKind, AudioFormat, DuplexControl, DuplexEvent, DuplexSession, ProviderTimeouts, Speaker,
};
use meclaw_cells::voice::params::DuplexParams;
use meclaw_cells::voice::providers::build_duplex;
use meclaw_colony::IoLivenessMark;
use meclaw_core::serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous, per the 30 s convention.
const MARKER: Duration = Duration::from_secs(30);

/// A semantic discriminator, not a failure marker: long enough that a frame
/// the loopback DID pass on has arrived, short enough that the test is quick.
/// Only ever used to prove an absence, where there is nothing else to wait for.
const SILENCE: Duration = Duration::from_millis(200);

struct Wired {
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
}

fn start() -> Wired {
    let params: DuplexParams = meclaw_core::serde_json::from_value(json!({"provider": "echo"}))
        .expect("the loopback block parses");
    let provider = build_duplex(&params, ProviderTimeouts::default()).expect("the adapter");
    let (audio_in_tx, audio_in_rx) = mpsc::channel(64);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(64);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(64);
    let fut = provider.run_session(
        AudioFormat::pcm16_mono(16_000),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    );
    tokio::spawn(async move {
        let _ = fut.await;
    });
    Wired {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
    }
}

async fn next_event(rx: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(MARKER, rx.recv())
        .await
        .expect("an event arrives within the failure marker")
        .expect("the session is still reporting")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_append_comes_back_answered_and_spoken() {
    let mut w = start();
    assert!(matches!(
        next_event(&mut w.events).await,
        DuplexEvent::Started { .. }
    ));

    w.control
        .send(DuplexControl::Append {
            kind: AppendKind::Commentary,
            event_id: "e1".to_string(),
            delegation_id: None,
            content: "hallo".to_string(),
        })
        .await
        .expect("the control channel is open");

    match next_event(&mut w.events).await {
        DuplexEvent::Appended {
            kind,
            event_id,
            start_ms,
            end_ms,
        } => {
            assert_eq!(kind, AppendKind::Commentary);
            assert_eq!(event_id, "e1", "the answer carries the id it was given");
            assert!(
                end_ms >= start_ms,
                "and where it landed: {start_ms}..{end_ms}"
            );
        }
        other => panic!("an append is answered, got {other:?}"),
    }
    match next_event(&mut w.events).await {
        DuplexEvent::Transcript {
            speaker,
            delta,
            start_ms,
            end_ms,
        } => {
            assert_eq!(
                speaker,
                Speaker::Assistant,
                "the mirror is the MODEL speaking, not the caller"
            );
            assert_eq!(delta, "hallo");
            assert!(end_ms >= start_ms);
        }
        other => panic!("the loopback mirrors the content as assistant text, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mute_closes_the_ear_and_an_unmute_opens_it() {
    let mut w = start();
    assert!(matches!(
        next_event(&mut w.events).await,
        DuplexEvent::Started { .. }
    ));

    w.control
        .send(DuplexControl::Mute)
        .await
        .expect("the control channel is open");
    assert_eq!(
        next_event(&mut w.events).await,
        DuplexEvent::Muted,
        "a closed ear is announced, not guessed"
    );

    w.audio_in
        .send(vec![7u8; 640])
        .await
        .expect("the session keeps taking audio while muted");
    assert!(
        tokio::time::timeout(SILENCE, w.audio_out.recv())
            .await
            .is_err(),
        "audio sent into a closed ear is not heard, so nothing comes back"
    );

    w.control
        .send(DuplexControl::Unmute)
        .await
        .expect("the control channel is open");
    assert_eq!(next_event(&mut w.events).await, DuplexEvent::Unmuted);

    w.audio_in
        .send(vec![9u8; 640])
        .await
        .expect("the session takes audio again");
    let back = tokio::time::timeout(MARKER, w.audio_out.recv())
        .await
        .expect("the loopback answers within the failure marker")
        .expect("the outbound channel is open");
    assert_eq!(
        back,
        vec![9u8; 640],
        "and what comes back is the frame sent AFTER the unmute, not the one before"
    );
}
