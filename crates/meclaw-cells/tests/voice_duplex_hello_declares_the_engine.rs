//! Welle Live, L1 — the first frame says which engine is behind the connection.
//!
//! A client cannot tell a cascade from a live session by listening: both send
//! audio back, both send transcripts. What differs is what ELSE arrives —
//! `spoken` frames, turns cut on the model's clock — and a client that has to
//! discover that by waiting is a client with a race in it. So `hello` declares
//! it, and `GET /info` answers the same question without opening a session
//! (R-V6).
//!
//! The field is ADDITIVE to `meclaw-voice/1`: a client that does not know it
//! reads the frame it always read. That is why the protocol version does not
//! move (OR-L28).
//!
//! What is asserted here is the CASCADE side of the declaration — `false` and
//! `null` — because that is the side every existing colony is on, and a wrong
//! answer there is a wrong answer for all of them. The duplex side is
//! `voice_duplex_never_reaches_the_echo_path`.

use futures_util::StreamExt;
use meclaw_cells::voice::contract::{
    AudioFormat, BoxFuture, SttError, SttEvent, SttProvider, TtsError, TtsProvider,
};
use meclaw_cells::voice::io::{VoiceIo, run_io};
use meclaw_cells::voice::wire::Mode;
use meclaw_colony::IoLivenessMark;
use meclaw_core::Path;
use meclaw_core::serde_json::Value;
use meclaw_testing::surface_listener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Failure-marker timeout: generous, per the 30 s convention.
const MARKER: Duration = Duration::from_secs(30);

/// The mount this fixture registers.
const MOUNT: &str = "voice";

/// A recognition provider that drains and says nothing — the shape of the
/// cascade, with none of its behaviour in the way.
struct QuietStt;

impl SttProvider for QuietStt {
    fn name(&self) -> &'static str {
        "quiet"
    }
    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(16_000)
    }
    fn run_session(
        &self,
        _format: AudioFormat,
        mut audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        Box::pin(async move {
            let _events = events;
            while audio.recv().await.is_some() {}
            Ok(())
        })
    }
}

/// A synthesis provider that never gets asked for anything.
struct QuietTts;

impl TtsProvider for QuietTts {
    fn name(&self) -> &'static str {
        "silent"
    }
    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }
    fn synthesize(
        &self,
        _format: AudioFormat,
        _text: String,
        _audio: mpsc::Sender<Vec<u8>>,
        _cancel: watch::Receiver<bool>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        Box::pin(async move { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cascade_says_it_is_not_a_duplex_session() {
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    // The test stands in for the handler, and the one thing a handler owes a
    // connection before its `hello` is to take the session (GH #836).
    let (events_tx, mut events_rx) = mpsc::channel::<meclaw_cells::voice::cell::VoiceEvent>(64);
    tokio::spawn(async move {
        while let Some(mut event) = events_rx.recv().await {
            event.acknowledge();
        }
    });
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(64);
    let mut io = VoiceIo::new(
        MOUNT.to_string(),
        Arc::new(QuietStt),
        Some(Arc::new(QuietTts)),
        Mode::Auto,
        Duration::from_secs(5),
        Duration::from_secs(5),
        events_tx,
    );
    io.cell_path = Path::new("/main/members/tester/channels/voice");
    io.surfaces = Arc::clone(&surfaces);
    assert!(
        io.duplex.is_none(),
        "a half built by hand is a cascade: `VoiceIo::new` is unchanged (OR-L23)"
    );
    let _task = tokio::spawn(run_io(io, reconfig_rx));
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cell registers its mount within the failure marker");

    let info: Value = reqwest::get(format!("http://{addr}/{MOUNT}/info"))
        .await
        .expect("GET /info")
        .json()
        .await
        .expect("the declaration is JSON");
    assert_eq!(
        info["duplex"],
        Value::Null,
        "a cascade has no duplex provider, and says so by name: {info}"
    );
    assert_eq!(
        info["stt"], "quiet",
        "and the two cascade names are its own"
    );
    assert_eq!(info["tts"], "silent");

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/{MOUNT}/ws"))
        .await
        .expect("the cell accepts a websocket");
    let frame = tokio::time::timeout(MARKER, ws.next())
        .await
        .expect("the hello arrives within the failure marker")
        .expect("the socket is open")
        .expect("a frame");
    let hello: Value =
        meclaw_core::serde_json::from_str(frame.to_text().expect("the first frame is text"))
            .expect("the hello is JSON");
    assert_eq!(hello["type"], "hello");
    assert_eq!(
        hello["protocol"], "meclaw-voice/1",
        "an additive field does not move the version (OR-L28): {hello}"
    );
    assert_eq!(
        hello["duplex"], false,
        "the first frame declares the engine, so a client never has to guess: {hello}"
    );
}
