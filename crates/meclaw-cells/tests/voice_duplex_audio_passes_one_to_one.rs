//! Welle Live, L1 — one client frame is one item, and it comes back as it went
//! in.
//!
//! R-L4 is the sentence this file holds: **no buffer, no pacing, no merging**.
//! A duplex session is the one place in this tree where latency is the product,
//! and every well-meant buffer on the way is latency somebody added by hand. So
//! the loopback is measured in BYTES and in COUNT, not in totals: fifty frames
//! in, fifty items out, each one identical to the one that went in and in the
//! order it went in.
//!
//! No socket here, and that is deliberate — this measures the contract
//! (`DuplexSession`), not the wire. The wire gets the same measurement in L2b,
//! over a real connection.

use meclaw_cells::voice::contract::{
    AudioFormat, DuplexControl, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::DuplexParams;
use meclaw_cells::voice::providers::build_duplex;
use meclaw_colony::IoLivenessMark;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous, per the 30 s convention.
const MARKER: Duration = Duration::from_secs(30);

/// One loopback session, with the four channel ends a test drives it by.
struct Wired {
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    task: tokio::task::JoinHandle<()>,
}

fn loopback() -> Arc<dyn DuplexProvider> {
    let params: DuplexParams = meclaw_core::serde_json::from_value(json!({"provider": "echo"}))
        .expect("the loopback block parses");
    build_duplex(&params, ProviderTimeouts::default()).expect("the loopback adapter")
}

fn start(provider: Arc<dyn DuplexProvider>) -> Wired {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(64);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(64);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(64);
    let session = DuplexSession {
        audio_in: audio_in_rx,
        audio_out: audio_out_tx,
        events: events_tx,
        control: control_rx,
    };
    let fut = provider.run_session(
        AudioFormat::pcm16_mono(16_000),
        session,
        IoLivenessMark::disabled(),
    );
    let task = tokio::spawn(async move {
        fut.await.expect("a loopback session ends cleanly");
    });
    Wired {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
        task,
    }
}

async fn next_event(rx: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    tokio::time::timeout(MARKER, rx.recv())
        .await
        .expect("an event arrives within the failure marker")
        .expect("the session is still reporting")
}

/// Fifty frames in, fifty items out, byte for byte and in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_frames_come_back_as_fifty_frames() {
    let mut w = start(loopback());
    assert!(
        matches!(next_event(&mut w.events).await, DuplexEvent::Started { .. }),
        "the first event of a session is that it started"
    );

    // Distinguishable frames: the n-th frame is 640 bytes of n, so a merge, a
    // reorder or a dropped frame is visible as a value and not as a total.
    let sent: Vec<Vec<u8>> = (0..50u8).map(|i| vec![i; 640]).collect();
    for frame in &sent {
        w.audio_in
            .send(frame.clone())
            .await
            .expect("the session keeps taking audio");
    }

    let mut got = Vec::new();
    while got.len() < sent.len() {
        let item = tokio::time::timeout(MARKER, w.audio_out.recv())
            .await
            .expect("the loopback answers within the failure marker")
            .expect("the outbound channel is open");
        got.push(item);
    }
    assert_eq!(
        got, sent,
        "one client frame is one item, never merged and never re-cut (R-L4)"
    );

    // The meter reading that belongs to those fifty items: one second of
    // 20 ms audio.
    let usage = next_event(&mut w.events).await;
    match usage {
        DuplexEvent::Usage {
            seconds,
            usage_ratio,
        } => {
            assert!(
                (seconds - 1.0).abs() < f64::EPSILON,
                "fifty 20 ms items is one second, got {seconds}"
            );
            assert_eq!(usage_ratio, None, "a loopback has no context window");
        }
        other => panic!("expected the meter reading after fifty items, got {other:?}"),
    }

    // The client goes away: `audio_in` closing IS the end of the call.
    drop(w.audio_in);
    match next_event(&mut w.events).await {
        DuplexEvent::Closed {
            reason,
            usage_seconds,
        } => {
            assert_eq!(reason, "close_requested");
            assert!(
                (usage_seconds - 1.0).abs() < f64::EPSILON,
                "the final reading is the session's total, got {usage_seconds}"
            );
        }
        other => panic!("expected the close, got {other:?}"),
    }
    tokio::time::timeout(MARKER, w.task)
        .await
        .expect("the session ends when the audio does")
        .expect("and it does not panic");
}

/// A `Close` ends it too, and says so with the same reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_ends_the_session() {
    let mut w = start(loopback());
    assert!(matches!(
        next_event(&mut w.events).await,
        DuplexEvent::Started { .. }
    ));
    w.control
        .send(DuplexControl::Close)
        .await
        .expect("the control channel is open");
    match next_event(&mut w.events).await {
        DuplexEvent::Closed { reason, .. } => assert_eq!(reason, "close_requested"),
        other => panic!("expected the close, got {other:?}"),
    }
    tokio::time::timeout(MARKER, w.task)
        .await
        .expect("the session ends on a close")
        .expect("and it does not panic");
}

// ──────────────────────────────────────────────────────────────────────────
// Welle Live, L2b — the same measurement on the real wire
// ──────────────────────────────────────────────────────────────────────────
//
// The two tests above measure the CONTRACT: fifty items through
// `DuplexSession` and back. This one measures the WIRE the contract is there
// for — a WebSocket client, `run_duplex`, the shipped loopback adapter, the
// framer, and the socket again — because that is where a well-meant buffer
// would be added. It is the file's own promise taken up ("the wire gets the
// same measurement in L2b, over a real connection").

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

/// How many chunks the client sends. Enough that a merge, a re-cut or a drop
/// is a value and not a rounding error.
const WIRE_CHUNKS: usize = 50;

/// What one chunk carries: 40 ms at 16 kHz, deliberately TWO frames' worth.
///
/// A chunk the size of a frame would prove nothing here: the cascade's own echo
/// branch writes a binary frame straight back uncut, so a 640-byte chunk comes
/// back identical on either path and the test would pass on the wrong one (it
/// did, measured against the reverted tree). At 1 280 bytes the two answers
/// differ — the cascade returns one frame of 1 280, the duplex path returns two
/// of 640 — and the assertion below can tell them apart.
const WIRE_CHUNK_BYTES: usize = 1_280;

/// Fifty chunks out, a hundred frames back: the same bytes, in order, and none
/// of them longer than the wire allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wire_carries_the_same_bytes_in_frames_the_client_can_swallow() {
    let raw = duplex_cell::duplex_params(meclaw_core::serde_json::json!({
        "default_mode": "auto",
        "audio_out_frame_ms": 20,
    }));
    let live = duplex_cell::boot_echo(raw).await;
    let (mut client, hello) = live.connect("session=wire-1&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    assert_eq!(
        hello["audio_out_frame_ms"], 20,
        "a duplex session IS framed, whatever the inert cascade placeholder \
         beside it says (OR-L23): {hello}"
    );

    // The n-th chunk is 1 280 bytes of n, so a reorder is visible as a value.
    #[allow(clippy::cast_possible_truncation)]
    let sent: Vec<Vec<u8>> = (0..WIRE_CHUNKS)
        .map(|i| vec![(i % 251) as u8; WIRE_CHUNK_BYTES])
        .collect();
    for chunk in &sent {
        client
            .send_audio(chunk)
            .await
            .expect("the socket takes audio");
    }

    let expected: Vec<u8> = sent.concat();
    let mut got: Vec<u8> = Vec::new();
    let mut frames = 0usize;
    while got.len() < expected.len() {
        let frame = client
            .next_frame(duplex_cell::MARKER)
            .await
            .expect("the loopback answers within the failure marker");
        let audio = frame
            .as_audio()
            .unwrap_or_else(|| panic!("expected audio, got {frame:?}"));
        assert!(
            audio.len() <= duplex_cell::FRAME_BYTES,
            "no frame carries more than 20 ms — the ceiling a telephony edge \
             aborts the call above (got {} bytes)",
            audio.len()
        );
        got.extend_from_slice(audio);
        frames += 1;
    }
    assert_eq!(
        got, expected,
        "the same bytes in the same order: no buffer, no pacing, nothing \
         invented (R-L4)"
    );
    assert_eq!(
        frames,
        WIRE_CHUNKS * 2,
        "and each 40 ms chunk left as exactly two 20 ms frames"
    );
}
