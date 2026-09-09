//! Wave voice-cell, t4: the `voice` cell's listener, seen from a client.
//!
//! Everything here runs against a bare [`VoiceIo`] — no colony, no handler, no
//! `cell.db`. That is the point: the I/O half is supposed to be complete on its
//! own, so the events it pushes and the commands it obeys can be checked
//! without a turn machine in the way. The providers are defined in this file
//! and do exactly what each test needs; the real ones arrive in t2/t3.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::voice::cell::{VoiceCell, VoiceEvent, VoiceReconfig};
use meclaw_cells::voice::contract::{
    AudioFormat, BoxFuture, SttError, SttEvent, SttProvider, TtsError, TtsProvider,
};
use meclaw_cells::voice::io::{VoiceIo, run_io};
use meclaw_cells::voice::params::{DEFAULT_AUDIO_OUT_FRAME_MS, VoiceParams};
use meclaw_cells::voice::wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason};
use meclaw_colony::{DbConn, IoLivenessMark, LongRunningCell};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{OriginSink, OutputSink, Path};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Failure-marker timeout: generous, per the 30 s convention.
const MARKER: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------- providers

/// A recognition provider that drains audio, counts it, and says what a test
/// told it to say.
struct ScriptedStt {
    name: &'static str,
    rate: u32,
    /// Bytes seen so far, so a test can prove audio reached the provider.
    seen: Arc<AtomicUsize>,
    /// How often `run_session` was entered — one per session, so a reconnect
    /// is visible as a second one.
    starts: Arc<AtomicUsize>,
    /// Emitted once, after `after_bytes` bytes arrived.
    script: Option<(usize, SttEvent)>,
    /// End the session with this error instead of draining.
    fail: Option<String>,
}

impl ScriptedStt {
    fn draining(seen: Arc<AtomicUsize>) -> Self {
        Self {
            name: "scripted",
            rate: 16_000,
            seen,
            starts: Arc::new(AtomicUsize::new(0)),
            script: None,
            fail: None,
        }
    }
    fn echo() -> Self {
        Self {
            name: "echo",
            rate: 16_000,
            seen: Arc::new(AtomicUsize::new(0)),
            starts: Arc::new(AtomicUsize::new(0)),
            script: None,
            fail: None,
        }
    }
    fn saying(seen: Arc<AtomicUsize>, after_bytes: usize, event: SttEvent) -> Self {
        Self {
            name: "scripted",
            rate: 16_000,
            seen,
            starts: Arc::new(AtomicUsize::new(0)),
            script: Some((after_bytes, event)),
            fail: None,
        }
    }
    fn failing(detail: &str) -> Self {
        Self {
            name: "scripted",
            rate: 16_000,
            seen: Arc::new(AtomicUsize::new(0)),
            starts: Arc::new(AtomicUsize::new(0)),
            script: None,
            fail: Some(detail.to_string()),
        }
    }

    /// Hand the test the session counter.
    fn counting(mut self, starts: Arc<AtomicUsize>) -> Self {
        self.starts = starts;
        self
    }
}

impl SttProvider for ScriptedStt {
    fn name(&self) -> &'static str {
        self.name
    }
    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.rate)
    }
    fn run_session(
        &self,
        _format: AudioFormat,
        mut audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let seen = self.seen.clone();
        let script = self.script.clone();
        let fail = self.fail.clone();
        let starts = self.starts.clone();
        Box::pin(async move {
            starts.fetch_add(1, Ordering::SeqCst);
            if let Some(detail) = fail {
                return Err(SttError::Connect(detail));
            }
            let mut fired = false;
            while let Some(chunk) = audio.recv().await {
                let total = seen.fetch_add(chunk.len(), Ordering::SeqCst) + chunk.len();
                if let Some((after, event)) = script.as_ref()
                    && !fired
                    && total >= *after
                {
                    fired = true;
                    if events.send(event.clone()).await.is_err() {
                        break;
                    }
                }
            }
            Ok(())
        })
    }
}

/// A recognition provider that takes the audio channel and never reads it.
///
/// It is how a connection is wedged on purpose: the connection task's own
/// backpressure (`audio_tx.send`) parks it, so it stops reading its command
/// channel — exactly the state a client that stopped reading its socket puts it
/// in, but reached in a bounded number of frames instead of a bounded number of
/// bytes.
struct StallingStt;

impl SttProvider for StallingStt {
    fn name(&self) -> &'static str {
        "stalling"
    }
    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(16_000)
    }
    fn run_session(
        &self,
        _format: AudioFormat,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        Box::pin(async move {
            // Held, not dropped: a closed channel would let the connection task
            // go on. The queue has to fill and stay full.
            let _held = (audio, events);
            std::future::pending::<()>().await;
            Ok(())
        })
    }
}

/// A synthesis provider that emits `chunks` chunks and honours cancellation.
struct ScriptedTts {
    chunks: usize,
    delay: Duration,
    fail: Option<String>,
    /// Go silent after the scripted chunks instead of returning — a wedged
    /// synthesis socket.
    hang: bool,
    /// Set when the provider saw the cancel flag.
    cancelled: Arc<AtomicUsize>,
}

impl ScriptedTts {
    fn new(chunks: usize, delay_ms: u64, cancelled: Arc<AtomicUsize>) -> Self {
        Self {
            chunks,
            delay: Duration::from_millis(delay_ms),
            fail: None,
            hang: false,
            cancelled,
        }
    }
    /// One chunk, then silence for ever.
    fn wedged() -> Self {
        Self {
            chunks: 1,
            delay: Duration::from_millis(0),
            fail: None,
            hang: true,
            cancelled: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn failing(detail: &str) -> Self {
        Self {
            chunks: 0,
            delay: Duration::from_millis(0),
            fail: Some(detail.to_string()),
            hang: false,
            cancelled: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl TtsProvider for ScriptedTts {
    fn name(&self) -> &'static str {
        "scripted-tts"
    }
    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }
    fn synthesize(
        &self,
        _format: AudioFormat,
        _text: String,
        audio: mpsc::Sender<Vec<u8>>,
        mut cancel: watch::Receiver<bool>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let chunks = self.chunks;
        let delay = self.delay;
        let fail = self.fail.clone();
        let hang = self.hang;
        let cancelled = self.cancelled.clone();
        Box::pin(async move {
            if let Some(detail) = fail {
                return Err(TtsError::Protocol(detail));
            }
            for i in 0..chunks {
                if *cancel.borrow_and_update() {
                    cancelled.fetch_add(1, Ordering::SeqCst);
                    return Err(TtsError::Cancelled);
                }
                tokio::select! {
                    _ = cancel.changed() => {
                        cancelled.fetch_add(1, Ordering::SeqCst);
                        return Err(TtsError::Cancelled);
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                if audio.send(vec![(i % 251) as u8; 4]).await.is_err() {
                    return Err(TtsError::Cancelled);
                }
            }
            if hang {
                // Neither a chunk nor an ending: exactly what a dead socket
                // looks like from this side.
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
}

/// A synthesis provider that emits exactly the chunks it was handed.
///
/// The framing tests need a chunk whose size is the *provider's* choice — the
/// whole point of `audio_out_frame_ms` is that the two sizes are not the same
/// number — so this one takes them literally instead of counting.
struct BulkTts {
    chunks: Vec<Vec<u8>>,
    /// Stay open after the last chunk instead of ending the synthesis, so a
    /// test can cancel while the framer still holds something.
    hang: bool,
}

impl BulkTts {
    fn new(chunks: Vec<Vec<u8>>) -> Self {
        Self {
            chunks,
            hang: false,
        }
    }
    fn hanging(chunks: Vec<Vec<u8>>) -> Self {
        Self { chunks, hang: true }
    }
}

impl TtsProvider for BulkTts {
    fn name(&self) -> &'static str {
        "bulk-tts"
    }
    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }
    fn synthesize(
        &self,
        _format: AudioFormat,
        _text: String,
        audio: mpsc::Sender<Vec<u8>>,
        mut cancel: watch::Receiver<bool>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let chunks = self.chunks.clone();
        let hang = self.hang;
        Box::pin(async move {
            for chunk in chunks {
                if *cancel.borrow_and_update() {
                    return Err(TtsError::Cancelled);
                }
                if audio.send(chunk).await.is_err() {
                    return Err(TtsError::Cancelled);
                }
            }
            if hang {
                let _ = cancel.changed().await;
                return Err(TtsError::Cancelled);
            }
            Ok(())
        })
    }
}

// ------------------------------------------------------------------ harness

/// A running listener plus the two channel ends a handler would hold.
struct Live {
    port: u16,
    events_rx: mpsc::Receiver<VoiceEvent>,
    reconfig_tx: mpsc::Sender<VoiceReconfig>,
    task: tokio::task::JoinHandle<()>,
}

impl Live {
    /// The next event the I/O half pushed at the handler.
    async fn event(&mut self) -> VoiceEvent {
        tokio::time::timeout(MARKER, self.events_rx.recv())
            .await
            .expect("the I/O half pushes an event")
            .expect("the events channel stays open")
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start(stt: Arc<dyn SttProvider>, tts: Option<Arc<dyn TtsProvider>>, mode: Mode) -> Live {
    start_with(
        stt,
        tts,
        mode,
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .await
}

async fn start_with(
    stt: Arc<dyn SttProvider>,
    tts: Option<Arc<dyn TtsProvider>>,
    mode: Mode,
    external: Duration,
    idle: Duration,
) -> Live {
    start_framed(stt, tts, mode, external, idle, DEFAULT_AUDIO_OUT_FRAME_MS).await
}

/// [`start_with`], with the outbound frame length named — `0` is the
/// passthrough this cell had before the framing.
async fn start_framed(
    stt: Arc<dyn SttProvider>,
    tts: Option<Arc<dyn TtsProvider>>,
    mode: Mode,
    external: Duration,
    idle: Duration,
    audio_out_frame_ms: u32,
) -> Live {
    let port = free_port();
    let (events_tx, events_rx) = mpsc::channel(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(64);
    let mut io = VoiceIo::new(
        "127.0.0.1".to_string(),
        port,
        stt,
        tts,
        mode,
        external,
        idle,
        events_tx,
    );
    io.audio_out_frame_ms = audio_out_frame_ms;
    let task = tokio::spawn(run_io(io, reconfig_rx));
    let mut live = Live {
        port,
        events_rx,
        reconfig_tx,
        task,
    };
    match live.event().await {
        VoiceEvent::Bound(addr) => assert!(addr.ends_with(&port.to_string()), "bound to {addr}"),
        other => panic!("expected Bound, got {}", label(&other)),
    }
    live
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(port: u16, query: &str) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws{query}"))
        .await
        .expect("the cell accepts a websocket on /ws");
    ws
}

/// The next text frame, as JSON.
async fn next_text(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("a frame arrives")
            .expect("the stream stays open")
            .expect("a readable frame");
        match msg {
            WsMessage::Text(t) => {
                return meclaw_core::serde_json::from_str(&t).expect("the frame is json");
            }
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

/// The next binary frame.
async fn next_binary(ws: &mut Ws) -> Vec<u8> {
    loop {
        let msg = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("a frame arrives")
            .expect("the stream stays open")
            .expect("a readable frame");
        match msg {
            WsMessage::Binary(b) => return b.to_vec(),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("expected a binary frame, got {other:?}"),
        }
    }
}

/// Read until the socket ends. A moved listener is the point: the connection it
/// accepted cannot follow it, and the client reconnects against the new address.
async fn drain_until_closed(ws: &mut Ws) {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => panic!("the socket never ended"),
            Ok(None) | Ok(Some(Err(_))) => return,
            Ok(Some(Ok(WsMessage::Close(_)))) => return,
            Ok(Some(Ok(_))) => continue,
        }
    }
}

/// Read until the socket closes; return the close code if there was one.
async fn close_code(ws: &mut Ws) -> Option<u16> {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        let next = tokio::time::timeout_at(deadline, ws.next()).await;
        match next {
            Err(_) => panic!("the socket never closed"),
            Ok(None) => return None,
            Ok(Some(Err(e))) => panic!("the socket ended with an error: {e}"),
            Ok(Some(Ok(WsMessage::Close(frame)))) => {
                return frame.map(|f| u16::from(f.code));
            }
            Ok(Some(Ok(_))) => continue,
        }
    }
}

async fn send_text(ws: &mut Ws, value: Value) {
    ws.send(WsMessage::Text(value.to_string().into()))
        .await
        .expect("send");
}

async fn send_binary(ws: &mut Ws, bytes: Vec<u8>) {
    ws.send(WsMessage::Binary(bytes.into()))
        .await
        .expect("send");
}

/// The variant name of an event.
///
/// `VoiceEvent` carries no `Debug` in the t1 contract, and a failing test still
/// has to say what arrived instead of what was expected.
fn label(event: &VoiceEvent) -> &'static str {
    match event {
        VoiceEvent::Bound(_) => "Bound",
        VoiceEvent::BindFailed(_) => "BindFailed",
        VoiceEvent::Connected { .. } => "Connected",
        VoiceEvent::Disconnected { .. } => "Disconnected",
        VoiceEvent::Control { .. } => "Control",
        VoiceEvent::Stt { .. } => "Stt",
        VoiceEvent::SpeakEnded { .. } => "SpeakEnded",
        VoiceEvent::BadAudioFrame { .. } => "BadAudioFrame",
        VoiceEvent::ReleaseGraceExpired { .. } => "ReleaseGraceExpired",
        VoiceEvent::ClientTooSlow { .. } => "ClientTooSlow",
    }
}

// -------------------------------------------------------------------- tests

/// Step 1: the two reads at the cell's edge, and the first frame of a session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn info_page_and_hello_declare_the_same_wiring() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;

    // R-V6: the declaration without a connection.
    let res = reqwest::get(format!("http://127.0.0.1:{}/info", live.port))
        .await
        .expect("GET /info");
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let info: Value =
        meclaw_core::serde_json::from_str(&res.text().await.expect("body")).expect("json");
    assert_eq!(info["protocol"], json!("meclaw-voice/1"));
    assert_eq!(info["mode"], json!("auto"));
    assert_eq!(
        info["audio_in"],
        json!({"encoding": "pcm_s16le", "sample_rate": 16000, "channels": 1})
    );
    assert_eq!(info["audio_out"], Value::Null, "no tts, no output format");
    assert_eq!(
        info["audio_out_frame_ms"],
        json!(0),
        "nothing is ever framed where nothing is ever spoken"
    );
    assert_eq!(info["stt"], json!("scripted"));
    assert_eq!(info["tts"], Value::Null);
    assert_eq!(
        info["speak_plain"],
        json!(true),
        "what happens to a written answer before it is spoken is declared"
    );
    assert!(
        info.get("session_id").is_none(),
        "nothing was opened, so there is no session id: {info}"
    );

    // R-V9: the built-in test page, exactly as t4b wrote it.
    let res = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("GET /");
    assert_eq!(res.status(), 200);
    assert!(
        res.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .starts_with("text/html"),
        "the test page is html"
    );
    assert_eq!(
        res.text().await.expect("body"),
        meclaw_cells::voice::testpage::html(),
        "the route serves the page verbatim"
    );

    // The session identity and the mode come from the query string.
    let mut ws = connect(live.port, "?session=abc&mode=hold").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["type"], json!("hello"));
    assert_eq!(hello["protocol"], json!("meclaw-voice/1"));
    assert_eq!(hello["session_id"], json!("abc"));
    assert_eq!(hello["mode"], json!("hold"));
    assert_eq!(hello["audio_in"]["sample_rate"], json!(16000));
    assert_eq!(
        hello["speak_plain"],
        json!(true),
        "the same declaration the read gives, on the connection itself"
    );
    match live.event().await {
        VoiceEvent::Connected { session_id, mode } => {
            assert_eq!(session_id, "abc");
            assert_eq!(mode, Mode::Hold);
        }
        other => panic!("expected Connected, got {}", label(&other)),
    }
}

/// Step 1: without `?session` the cell mints one, and the default mode applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_without_an_id_gets_a_uuid7() {
    let seen = Arc::new(AtomicUsize::new(0));
    let live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut ws = connect(live.port, "").await;
    let hello = next_text(&mut ws).await;
    let id = hello["session_id"]
        .as_str()
        .expect("a session id")
        .to_string();
    assert_eq!(id.len(), 36, "uuid shaped: {id}");
    assert_eq!(
        id.chars().nth(14),
        Some('7'),
        "the version nibble says v7: {id}"
    );
    assert_eq!(hello["mode"], json!("auto"), "the cell's default applies");
}

/// R-V15: a telephony edge names the session `session_token`, and `session`
/// wins when a client sends both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_token_is_an_alias_for_session() {
    let seen = Arc::new(AtomicUsize::new(0));
    let live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;

    let mut ws = connect(live.port, "?session_token=call-7f3a").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["session_id"], json!("call-7f3a"));

    let mut ws = connect(live.port, "?session=chosen&session_token=ignored").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(
        hello["session_id"],
        json!("chosen"),
        "session wins over its alias"
    );
}

/// Step 2: the handler addresses one session, and can end it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_handler_reaches_and_closes_one_session() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::ToClient {
            session_id: "abc".to_string(),
            frame: meclaw_cells::voice::wire::ServerFrame::Mode { mode: Mode::Hold },
        })
        .await
        .expect("send");
    let frame = next_text(&mut ws).await;
    assert_eq!(frame, json!({"type": "mode", "mode": "hold"}));

    live.reconfig_tx
        .send(VoiceReconfig::Close {
            session_id: "abc".to_string(),
            code: 4409,
        })
        .await
        .expect("send");
    assert_eq!(close_code(&mut ws).await, Some(4409));
}

/// Step 2: a second connection takes the session, and the first is told so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_connection_displaces_the_first() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut first = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut first).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "abc"),
        other => panic!("expected Connected, got {}", label(&other)),
    }

    let mut second = connect(live.port, "?session=abc").await;
    let hello = next_text(&mut second).await;
    assert_eq!(hello["session_id"], json!("abc"));

    assert_eq!(
        close_code(&mut first).await,
        Some(4409),
        "the older connection is displaced"
    );
    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "abc"),
        other => panic!("expected Disconnected first, got {}", label(&other)),
    }
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "abc"),
        other => panic!("expected Connected second, got {}", label(&other)),
    }
}

/// Step 3: audio goes to the provider, semantics go to the handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_reaches_the_provider_and_events_reach_the_handler() {
    let seen = Arc::new(AtomicUsize::new(0));
    let stt = ScriptedStt::saying(
        seen.clone(),
        3200,
        SttEvent::EndOfTurn {
            text: "hello".to_string(),
        },
    );
    let mut live = start(Arc::new(stt), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    send_binary(&mut ws, vec![0u8; 3200]).await;
    match live.event().await {
        VoiceEvent::Stt { session_id, event } => {
            assert_eq!(session_id, "abc");
            assert_eq!(
                event,
                SttEvent::EndOfTurn {
                    text: "hello".to_string()
                }
            );
        }
        other => panic!("expected Stt, got {}", label(&other)),
    }
    assert_eq!(
        seen.load(Ordering::SeqCst),
        3200,
        "every byte reached the provider"
    );

    // A control frame is the handler's business, not this half's.
    send_text(&mut ws, json!({"type": "hold"})).await;
    match live.event().await {
        VoiceEvent::Control { session_id, frame } => {
            assert_eq!(session_id, "abc");
            assert_eq!(frame, ClientFrame::Hold);
        }
        other => panic!("expected Control, got {}", label(&other)),
    }

    // Nonsense is answered, and the connection survives it.
    send_text(&mut ws, json!({"type": "bogus"})).await;
    let err = next_text(&mut ws).await;
    assert_eq!(err["type"], json!("error"));
    assert_eq!(err["code"], json!("bad_frame"));
    send_text(&mut ws, json!({"type": "release"})).await;
    match live.event().await {
        VoiceEvent::Control { frame, .. } => assert_eq!(frame, ClientFrame::Release),
        other => panic!(
            "expected Control after the bad frame, got {}",
            label(&other)
        ),
    }
}

/// The handler has no clock, so the release grace is a round trip over this
/// seam: `ArmReleaseGrace` out, `ReleaseGraceExpired` back with the token it
/// was armed with.
///
/// Two things are proved here and nowhere else. The answer is not early — a
/// turn cut before the grace it was armed with is the defect this whole knob
/// exists against — and the wait does not block the loop that owns the
/// listener: a second command sent right behind the arming is served while the
/// first is still asleep (GH #593, and here it is a `sleep` rather than a slow
/// client).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_armed_release_grace_comes_back_with_its_token() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    let armed = tokio::time::Instant::now();
    live.reconfig_tx
        .send(VoiceReconfig::ArmReleaseGrace {
            session_id: "abc".to_string(),
            ms: 200,
            token: 7,
        })
        .await
        .expect("send");

    // The loop is still serving while the grace sleeps.
    live.reconfig_tx
        .send(VoiceReconfig::ToClient {
            session_id: "abc".to_string(),
            frame: meclaw_cells::voice::wire::ServerFrame::Mode { mode: Mode::Hold },
        })
        .await
        .expect("send");
    let frame = next_text(&mut ws).await;
    assert_eq!(
        frame,
        json!({"type": "mode", "mode": "hold"}),
        "a grace that parked the command loop would starve everything behind it"
    );

    match live.event().await {
        VoiceEvent::ReleaseGraceExpired { session_id, token } => {
            assert_eq!(session_id, "abc");
            assert_eq!(token, 7, "the generation comes back untouched");
        }
        other => panic!("expected ReleaseGraceExpired, got {}", label(&other)),
    }
    assert!(
        armed.elapsed() >= Duration::from_millis(200),
        "the grace must not report before it has elapsed: {:?}",
        armed.elapsed()
    );
}

/// R-V17: a warning is a notice, not a failure — it reaches the handler and
/// the session it came from goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_warning_passes_through_without_a_reconnect() {
    let seen = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let stt = ScriptedStt::saying(
        seen.clone(),
        100,
        SttEvent::Warning {
            detail: "sample rate mismatch, continuing".to_string(),
        },
    )
    .counting(starts.clone());
    let mut live = start(Arc::new(stt), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    send_binary(&mut ws, vec![0u8; 100]).await;
    match live.event().await {
        VoiceEvent::Stt { session_id, event } => {
            assert_eq!(session_id, "abc");
            assert_eq!(
                event,
                SttEvent::Warning {
                    detail: "sample rate mismatch, continuing".to_string()
                }
            );
        }
        other => panic!("expected Stt, got {}", label(&other)),
    }

    // The session is still the same one, and still listening.
    send_binary(&mut ws, vec![1u8; 640]).await;
    let deadline = tokio::time::Instant::now() + MARKER;
    while seen.load(Ordering::SeqCst) < 740 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the provider kept receiving after the warning"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "a warning starts no second session"
    );

    // And nothing was sent to the client about it: a warning is the handler's
    // business, not a wire error.
    let quiet = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
    assert!(quiet.is_err(), "no frame follows a warning: {quiet:?}");
}

/// Step 3 (R-V6'): a mis-framed binary frame is dropped and counted, and the
/// call goes on.
///
/// The counter is the whole answer to "is this one lost buffer or a client with
/// the wrong sample format", and it is why no threshold is needed: nothing is
/// hung up on, and a systematic fault is a number that climbs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_odd_binary_frame_is_reported_and_the_connection_stays() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen.clone())),
        None,
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    for expected in 1..=2u32 {
        send_binary(&mut ws, vec![7u8; 3201]).await;
        match live.event().await {
            VoiceEvent::BadAudioFrame {
                session_id,
                len,
                count,
            } => {
                assert_eq!(session_id, "abc");
                assert_eq!(len, 3201);
                assert_eq!(count, expected, "the counter climbs with each one");
            }
            other => panic!("expected BadAudioFrame, got {}", label(&other)),
        }
    }

    // Still the same connection, still listening: a good frame gets through.
    send_binary(&mut ws, vec![0u8; 640]).await;
    let deadline = tokio::time::Instant::now() + MARKER;
    while seen.load(Ordering::SeqCst) < 640 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "a good frame still reaches the provider after two bad ones"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        seen.load(Ordering::SeqCst),
        640,
        "and the dropped frames never reached it"
    );
}

/// Step 4: one synthesis, start to finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speak_order_becomes_frames_and_a_verdict() {
    let seen = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(ScriptedTts::new(3, 1, cancelled))),
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_out"]["sample_rate"], json!(24000));
    assert_eq!(hello["tts"], json!("scripted-tts"));
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let start = next_text(&mut ws).await;
    assert_eq!(start, json!({"type": "speak_start", "speak_id": "s1"}));
    for i in 0..3 {
        assert_eq!(next_binary(&mut ws).await, vec![i as u8; 4]);
    }
    let end = next_text(&mut ws).await;
    assert_eq!(
        end,
        json!({"type": "speak_end", "speak_id": "s1", "reason": "done"})
    );
    match live.event().await {
        VoiceEvent::SpeakEnded {
            session_id,
            speak_id,
            reason,
            detail,
        } => {
            assert_eq!(session_id, "abc");
            assert_eq!(speak_id, "s1");
            assert_eq!(reason, SpeakEndReason::Done);
            assert_eq!(detail, None);
        }
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }
}

/// Step 4 (R-V11): a cancel stops the sound at once and is reported once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_stops_the_synthesis_and_discards_what_is_buffered() {
    let seen = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(ScriptedTts::new(50, 20, cancelled.clone()))),
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "a long answer".to_string(),
        })
        .await
        .expect("send");
    let _start = next_text(&mut ws).await;
    let _first = next_binary(&mut ws).await;

    live.reconfig_tx
        .send(VoiceReconfig::CancelSpeak {
            session_id: "abc".to_string(),
        })
        .await
        .expect("send");

    // Chunks produced before the cancel took effect may still be in flight; the
    // verdict is the boundary that matters.
    let mut heard = 1;
    let end = loop {
        let msg = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("a frame arrives")
            .expect("open")
            .expect("readable");
        match msg {
            WsMessage::Text(t) => {
                break meclaw_core::serde_json::from_str::<Value>(&t).expect("json");
            }
            WsMessage::Binary(_) => heard += 1,
            _ => continue,
        }
    };
    assert_eq!(
        end,
        json!({"type": "speak_end", "speak_id": "s1", "reason": "cancelled"})
    );
    assert!(
        heard < 50,
        "the synthesis was cut short, not run to its end: {heard} chunks"
    );
    match live.event().await {
        VoiceEvent::SpeakEnded { reason, .. } => assert_eq!(reason, SpeakEndReason::Cancelled),
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }

    // And nothing follows it: the buffered audio was discarded, not flushed
    // (R-V11). A semantic timing discriminator, held deliberately tight — the
    // provider would have produced 49 more chunks at 20 ms each.
    let after = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
    assert!(after.is_err(), "no frame may follow the verdict: {after:?}");
}

/// Step 4: a synthesis socket that goes quiet is a verdict too — and traffic on
/// the connection does not reset that deadline.
///
/// The defect this pins: a `timeout` rebuilt inside the connection's `select!`
/// is reset by every other arm that wins, so a client that keeps sending audio
/// would keep a wedged synthesis alive for ever. The client here sends a frame
/// every 20 ms against a 400 ms idle deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wedged_synthesis_socket_ends_even_while_the_client_talks() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start_with(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(ScriptedTts::wedged())),
        Mode::Auto,
        Duration::from_secs(5),
        Duration::from_millis(400),
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let start = next_text(&mut ws).await;
    assert_eq!(start["type"], json!("speak_start"));
    let _first = next_binary(&mut ws).await;

    // Talk over it, and keep talking.
    let end = loop {
        send_binary(&mut ws, vec![0u8; 640]).await;
        match tokio::time::timeout(Duration::from_millis(20), ws.next()).await {
            Err(_) => continue,
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                break meclaw_core::serde_json::from_str::<Value>(&t).expect("json");
            }
            Ok(Some(Ok(_))) => continue,
            other => panic!("the socket ended before the verdict: {other:?}"),
        }
    };
    assert_eq!(end["type"], json!("speak_end"));
    assert_eq!(end["reason"], json!("failed"));
    assert!(
        end["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("no audio"),
        "the reason names the silence: {end}"
    );
    match live.event().await {
        VoiceEvent::SpeakEnded { reason, .. } => assert_eq!(reason, SpeakEndReason::Failed),
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }
}

/// Step 4: a provider failure is a verdict, not a silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_synthesis_ends_with_a_reason() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(ScriptedTts::failing("no voice by that name"))),
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let _start = next_text(&mut ws).await;
    let end = next_text(&mut ws).await;
    assert_eq!(end["type"], json!("speak_end"));
    assert_eq!(end["reason"], json!("failed"));
    assert!(
        end["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("no voice by that name"),
        "the reason travels: {end}"
    );
    match live.event().await {
        VoiceEvent::SpeakEnded { reason, detail, .. } => {
            assert_eq!(reason, SpeakEndReason::Failed);
            assert!(detail.is_some());
        }
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }
}

/// Step 4: a client that hangs up mid-sentence still closes the books.
///
/// The handler counts one `SpeakEnded` per `Speak` it issued. Without one here
/// its session would be marked speaking for ever and the next answer would
/// queue behind a synthesis that no longer exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_leaves_mid_synthesis_still_ends_the_speak() {
    let seen = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(ScriptedTts::new(50, 20, cancelled))),
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "a long answer".to_string(),
        })
        .await
        .expect("send");
    let _start = next_text(&mut ws).await;
    let _first = next_binary(&mut ws).await;

    ws.send(WsMessage::Close(None)).await.expect("close");
    drop(ws);

    match live.event().await {
        VoiceEvent::SpeakEnded {
            session_id,
            speak_id,
            reason,
            ..
        } => {
            assert_eq!(session_id, "abc");
            assert_eq!(speak_id, "s1");
            assert_eq!(reason, SpeakEndReason::Cancelled);
        }
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }
    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "abc"),
        other => panic!("expected Disconnected after it, got {}", label(&other)),
    }
}

/// Step 1: what the door refuses, and how.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door_refuses_a_malformed_query_and_a_wrong_path() {
    let seen = Arc::new(AtomicUsize::new(0));
    let live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let base = format!("http://127.0.0.1:{}", live.port);

    let long = "a".repeat(129);
    for (url, why) in [
        (
            format!("{base}/ws?session={long}"),
            "a session over 128 characters",
        ),
        (
            format!("{base}/ws?session=a%20b"),
            "a session with a space in it",
        ),
        (
            format!("{base}/ws?mode=nope"),
            "a mode that is neither word",
        ),
        (format!("{base}/ws"), "a plain GET on the socket path"),
    ] {
        let res = reqwest::get(&url).await.expect("request");
        assert_eq!(res.status(), 400, "{why} is refused: {url}");
    }

    let res = reqwest::get(format!("{base}/nope")).await.expect("request");
    assert_eq!(
        res.status(),
        404,
        "the cell serves three routes and no more"
    );
}

/// Step 5: the echo path is a loopback, in order and byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_echo_provider_is_a_byte_identical_loopback() {
    let mut live = start(Arc::new(ScriptedStt::echo()), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["stt"], json!("echo"));
    assert_eq!(hello["tts"], Value::Null);
    assert_eq!(
        hello["audio_out"], hello["audio_in"],
        "echo speaks what it hears"
    );
    let _connected = live.event().await;

    let sent: Vec<Vec<u8>> = (0..10u8).map(|i| vec![i; 640]).collect();
    for frame in &sent {
        send_binary(&mut ws, frame.clone()).await;
    }
    for expected in &sent {
        assert_eq!(&next_binary(&mut ws).await, expected);
    }

    // Turn frames mean nothing without a model, and are refused rather than
    // silently ignored.
    send_text(&mut ws, json!({"type": "hold"})).await;
    let err = next_text(&mut ws).await;
    assert_eq!(err["type"], json!("error"));
    assert_eq!(err["code"], json!("wrong_mode"));
}

/// Step 3: a recognition session that will not come back ends the connection,
/// after exactly one retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recognition_session_that_keeps_failing_ends_the_connection() {
    let mut live = start(
        Arc::new(ScriptedStt::failing("the provider refused")),
        None,
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    for round in 0..2 {
        match live.event().await {
            VoiceEvent::Stt {
                event: SttEvent::Failed { detail },
                ..
            } => assert!(
                detail.contains("the provider refused"),
                "round {round}: {detail}"
            ),
            other => panic!("round {round}: expected Stt Failed, got {}", label(&other)),
        }
    }
    // Told twice: once for the failure that costs a second of deaf audio, once
    // for the one that ends the connection.
    for round in 0..2 {
        let err = next_text(&mut ws).await;
        assert_eq!(err["type"], json!("error"), "round {round}");
        assert_eq!(err["code"], json!("stt_failed"), "round {round}");
    }
    assert_eq!(close_code(&mut ws).await, Some(1011));
}

/// Review 2: two connections move together, and both are reported before the
/// new address is announced.
///
/// The defect this pins: with the table alone as the count, the second
/// connection could empty it and release the rebind while the first was still
/// between its own removal and its own `Disconnected`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_moved_connections_are_both_reported_before_the_new_address() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut first = connect(live.port, "?session=one").await;
    let _hello = next_text(&mut first).await;
    let _connected = live.event().await;
    let mut second = connect(live.port, "?session=two").await;
    let _hello = next_text(&mut second).await;
    let _connected = live.event().await;

    let next_port = free_port();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    live.reconfig_tx
        .send(VoiceReconfig::Rebind {
            bind: "127.0.0.1".to_string(),
            port: next_port,
            ack: ack_tx,
        })
        .await
        .expect("send");
    let verdict = tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the I/O half answers")
        .expect("the ack channel stays open");
    assert!(verdict.is_ok(), "the new address binds: {verdict:?}");
    drain_until_closed(&mut first).await;
    drain_until_closed(&mut second).await;

    let mut gone = Vec::new();
    for _ in 0..2 {
        match live.event().await {
            VoiceEvent::Disconnected { session_id } => gone.push(session_id),
            other => panic!(
                "both disconnects come before the new address, got {} after {gone:?}",
                label(&other)
            ),
        }
    }
    gone.sort();
    assert_eq!(gone, vec!["one".to_string(), "two".to_string()]);
    match live.event().await {
        VoiceEvent::Bound(addr) => assert!(addr.ends_with(&next_port.to_string()), "{addr}"),
        other => panic!("expected Bound after both, got {}", label(&other)),
    }
}

/// Step 1/2: the listener moves, and the connections it accepted go with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listener_moves_and_its_connections_are_dropped() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;

    let next_port = free_port();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    live.reconfig_tx
        .send(VoiceReconfig::Rebind {
            bind: "127.0.0.1".to_string(),
            port: next_port,
            ack: ack_tx,
        })
        .await
        .expect("send");
    let verdict = tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the I/O half answers")
        .expect("the ack channel stays open");
    assert!(verdict.is_ok(), "the new address binds: {verdict:?}");
    drain_until_closed(&mut ws).await;

    // The connection the old listener accepted is gone, and the handler is told
    // so — otherwise its session table would keep a row for a socket nobody can
    // reach, and `unknown_session` could never fire for that identity again.
    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "abc"),
        other => panic!(
            "expected Disconnected for the moved connection, got {}",
            label(&other)
        ),
    }
    match live.event().await {
        VoiceEvent::Bound(addr) => assert!(addr.ends_with(&next_port.to_string()), "{addr}"),
        other => panic!("expected Bound on the new address, got {}", label(&other)),
    }
    let mut ws = connect(next_port, "?session=abc").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["session_id"], json!("abc"));
}

// ------------------------------------------------- a client that stopped reading

/// One filler command, small and harmless: what matters is that there are more
/// of them than a connection's command channel holds.
fn filler() -> ServerFrame {
    ServerFrame::Mode { mode: Mode::Hold }
}

/// Park this connection's task where a client that stopped reading parks it,
/// and prove it is parked.
///
/// Positive control first — a command arrives while the task is healthy — then
/// the recognition queue is filled so the task blocks in its own backpressure,
/// then a negative control: a second command does not arrive. Without both
/// halves a test could pass on a connection that was simply slow.
async fn wedge(live: &mut Live, ws: &mut Ws, session_id: &str) {
    live.reconfig_tx
        .send(VoiceReconfig::ToClient {
            session_id: session_id.to_string(),
            frame: filler(),
        })
        .await
        .expect("send");
    let frame = next_text(ws).await;
    assert_eq!(
        frame,
        json!({"type": "mode", "mode": "hold"}),
        "positive control"
    );

    // Fill the provider queue (32) so the task parks on a send that will never
    // have room again. Round by round, because the task prefers its command
    // channel to its socket: a command sent before the queue is full is still
    // delivered, which is what the negative control below reads.
    for round in 0..10 {
        for _ in 0..50 {
            send_binary(ws, vec![0u8; 320]).await;
        }
        live.reconfig_tx
            .send(VoiceReconfig::ToClient {
                session_id: session_id.to_string(),
                frame: filler(),
            })
            .await
            .expect("send");
        // Negative control: nothing arrives any more, because the task never
        // returns to its command channel.
        if tokio::time::timeout(Duration::from_millis(300), ws.next())
            .await
            .is_err()
        {
            return;
        }
        assert!(
            round < 9,
            "the connection task never parked on its recognition queue"
        );
    }
}

/// GH #593: a rebind is answered while one client's delivery is wedged.
///
/// The defect this pins: `run_io` dispatched commands with a blocking send in
/// the same loop that reads `Rebind`. A connection whose channel was full
/// parked that send, the `Rebind` queued behind it was never read, and the
/// handler's ack timeout refused an update that was perfectly good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebind_is_answered_while_a_client_is_wedged() {
    let mut live = start_with(
        Arc::new(StallingStt),
        None,
        Mode::Auto,
        Duration::from_millis(500),
        Duration::from_millis(500),
    )
    .await;
    let mut stuck = connect(live.port, "?session=stuck").await;
    let _hello = next_text(&mut stuck).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "stuck"),
        other => panic!("expected Connected, got {}", label(&other)),
    }
    wedge(&mut live, &mut stuck, "stuck").await;

    // More commands than the connection's channel holds: the 65th of these is
    // where the old loop stopped reading anything else.
    for _ in 0..100 {
        live.reconfig_tx
            .send(VoiceReconfig::ToClient {
                session_id: "stuck".to_string(),
                frame: filler(),
            })
            .await
            .expect("send");
    }

    let next_port = free_port();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    live.reconfig_tx
        .send(VoiceReconfig::Rebind {
            bind: "127.0.0.1".to_string(),
            port: next_port,
            ack: ack_tx,
        })
        .await
        .expect("send");
    let verdict = tokio::time::timeout(Duration::from_secs(2), ack_rx)
        .await
        .expect("the rebind is answered even though a client is wedged")
        .expect("the ack channel stays open");
    assert!(verdict.is_ok(), "the new address binds: {verdict:?}");

    // The wedged connection is dropped like every other one the old listener
    // accepted, and it is reported — a row nobody can reach must not outlive
    // the address it was accepted on.
    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "stuck"),
        other => panic!(
            "expected Disconnected for the wedged connection, got {}",
            label(&other)
        ),
    }
    match live.event().await {
        VoiceEvent::Bound(addr) => assert!(addr.ends_with(&next_port.to_string()), "{addr}"),
        other => panic!("expected Bound on the new address, got {}", label(&other)),
    }
    let mut fresh = connect(next_port, "?session=fresh").await;
    let hello = next_text(&mut fresh).await;
    assert_eq!(hello["session_id"], json!("fresh"));
}

/// **A connection that dies mid-sentence still reports the sentence.**
///
/// The hang-up-after-`speak_end` mechanism of the telephony hive rests on one
/// promise: exactly one `SpeakEnded` per `Speak` the handler issued, whatever
/// happens to the socket. Three arms of the connection task can be the last one
/// standing when the client goes away — the post-loop block, the end of the
/// synthesis with a dead sink, and a `Speak` whose very first frame cannot be
/// written — and two of them used to drop the verdict on the floor, which left
/// a hive waiting for ever.
///
/// So this **counts**: one `Speak`, the client torn away in the middle of it,
/// exactly one `SpeakEnded` and no second one. Which arm carries it depends on
/// where the write happens to fail, and that is precisely what must not matter;
/// the two repaired arms are only reachable when a socket errors at an instant
/// no test can choose, so what is pinned here is the promise and not the arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_speak_ends_even_when_the_client_disappears() {
    let cancelled = Arc::new(AtomicUsize::new(0));
    // Long enough that the tear-down lands INSIDE the synthesis rather than
    // after it.
    let mut live = start(
        Arc::new(StallingStt),
        Some(Arc::new(ScriptedTts::new(50, 20, cancelled))),
        Mode::Auto,
    )
    .await;
    let mut client = connect(live.port, "?session=torn").await;
    let _hello = next_text(&mut client).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "torn"),
        other => panic!("expected Connected, got {}", label(&other)),
    }

    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "torn".to_string(),
            speak_id: "s1".to_string(),
            text: "A sentence nobody hears to the end.".to_string(),
        })
        .await
        .expect("send");
    // Wait until the synthesis is really running, then tear the socket away
    // without a close frame — the client vanishing, not saying goodbye.
    let _ = tokio::time::timeout(MARKER, client.next())
        .await
        .expect("the first frame of the synthesis arrives");
    drop(client);

    let mut ends = 0usize;
    let mut disconnected = false;
    while !disconnected {
        match event_within(&mut live, Duration::from_secs(10)).await {
            VoiceEvent::SpeakEnded { speak_id, .. } => {
                assert_eq!(speak_id, "s1");
                ends += 1;
            }
            VoiceEvent::Disconnected { session_id } => {
                assert_eq!(session_id, "torn");
                disconnected = true;
            }
            _ => {}
        }
    }
    assert_eq!(
        ends, 1,
        "the `Speak` the handler issued comes back exactly once, however the \
         connection ended — never nought, which would leave a waiter hanging, \
         and never twice"
    );
}

/// GH #593: a `Speak` that cannot be delivered is answered, not swallowed.
///
/// The handler counts on exactly one `SpeakEnded` per `Speak` it issued. A
/// command that never reaches its client therefore has to come back as a
/// failure — otherwise the turn machine waits for a synthesis nobody started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speak_to_a_wedged_client_ends_as_a_reported_failure() {
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut live = start_with(
        Arc::new(StallingStt),
        Some(Arc::new(ScriptedTts::new(1, 0, cancelled))),
        Mode::Auto,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .await;
    let mut stuck = connect(live.port, "?session=stuck").await;
    let _hello = next_text(&mut stuck).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "stuck"),
        other => panic!("expected Connected, got {}", label(&other)),
    }
    wedge(&mut live, &mut stuck, "stuck").await;

    // Fill the connection's channel, so the synthesis order cannot slip
    // through ahead of the backlog it is queued behind.
    for _ in 0..100 {
        live.reconfig_tx
            .send(VoiceReconfig::ToClient {
                session_id: "stuck".to_string(),
                frame: filler(),
            })
            .await
            .expect("send");
    }
    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "stuck".to_string(),
            speak_id: "s1".to_string(),
            text: "nobody will hear this".to_string(),
        })
        .await
        .expect("send");

    match live.event().await {
        VoiceEvent::SpeakEnded {
            session_id,
            speak_id,
            reason,
            detail,
        } => {
            assert_eq!(session_id, "stuck");
            assert_eq!(speak_id, "s1");
            assert_eq!(reason, SpeakEndReason::Failed);
            let detail = detail.expect("a failed speak says why");
            assert!(detail.contains("nothing was said"), "{detail}");
        }
        other => panic!(
            "expected a failed SpeakEnded for the undelivered speak, got {}",
            label(&other)
        ),
    }
    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "stuck"),
        other => panic!(
            "expected Disconnected after the connection was given up on, got {}",
            label(&other)
        ),
    }
}

/// The next event, under a deadline the test chooses.
///
/// [`Live::event`] waits the 30 s failure marker, which cannot tell "reported
/// at once" apart from "reported when a timer eventually fired". The two tests
/// below are about exactly that difference.
async fn event_within(live: &mut Live, limit: Duration) -> VoiceEvent {
    tokio::time::timeout(limit, live.events_rx.recv())
        .await
        .expect("the I/O half reports without waiting for a deadline")
        .expect("the events channel stays open")
}

/// GH #593, the count rather than the clock: a burst past both buffers is given
/// up on at once, and said out loud.
///
/// `external_timeout` is half a minute here on purpose. If the only way out of
/// a wedged connection were the delivery deadline, nothing below would arrive
/// in the five seconds this test allows — the report has to come from the
/// second trigger, 64 commands queued behind an already full connection
/// channel.
///
/// Since GH #601 the first thing out of that trigger is the reason:
/// `ClientTooSlow`, with the number of queued commands the session took with
/// it. Then the `Speak` that did not fit, then the disconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_past_both_buffers_is_given_up_on_without_waiting_for_the_deadline() {
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut live = start_with(
        Arc::new(StallingStt),
        Some(Arc::new(ScriptedTts::new(1, 0, cancelled))),
        Mode::Auto,
        Duration::from_secs(30),
        Duration::from_secs(30),
    )
    .await;
    let mut stuck = connect(live.port, "?session=stuck").await;
    let _hello = next_text(&mut stuck).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "stuck"),
        other => panic!("expected Connected, got {}", label(&other)),
    }
    wedge(&mut live, &mut stuck, "stuck").await;

    // More than the two buffers together hold (64 + 64).
    for i in 0..400 {
        live.reconfig_tx
            .send(VoiceReconfig::Speak {
                session_id: "stuck".to_string(),
                speak_id: format!("s{i}"),
                text: "nobody will hear this".to_string(),
            })
            .await
            .expect("send");
    }

    let limit = Duration::from_secs(5);
    // GH #601: the count says so before anything else does, and it names how
    // many queued commands went with the session — the 64 the dispatch queue
    // held, plus the one that no longer fitted.
    match event_within(&mut live, limit).await {
        VoiceEvent::ClientTooSlow {
            session_id,
            dropped,
        } => {
            assert_eq!(session_id, "stuck");
            assert_eq!(
                dropped, 65,
                "the queue that decided, plus the command that bounced"
            );
        }
        other => panic!(
            "the give-up reports its reason first, got {}",
            label(&other)
        ),
    }
    match event_within(&mut live, limit).await {
        VoiceEvent::SpeakEnded { reason, detail, .. } => {
            assert_eq!(reason, SpeakEndReason::Failed);
            let detail = detail.expect("a failed speak says why");
            assert!(detail.contains("the client is not reading"), "{detail}");
        }
        other => panic!(
            "the command that did not fit is reported, got {}",
            label(&other)
        ),
    }
    // Exactly one report, not one per queued command: the rest of the backlog
    // belongs to a session this connection no longer holds, so it is a log line
    // and not an event addressed at whoever holds that identity next.
    match event_within(&mut live, limit).await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "stuck"),
        other => panic!("expected Disconnected next, got {}", label(&other)),
    }
}

/// GH #593: a rebind leaves no row behind, even for a connection with nothing
/// queued for it.
///
/// The gap this pins is the one the delivery deadline cannot close. A wedged
/// connection with an *empty* queue never makes its `deliver` task wait for
/// anything — the rebind's own `Close` fits — so nothing there ever times out,
/// nothing gives up, and the connection never reports itself gone. Without the
/// eviction after the drain, `Bound` would be announced with that row still in
/// the table and the handler's session map would carry it for the rest of the
/// cell's life.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebind_drops_a_wedged_connection_that_has_nothing_queued() {
    let mut live = start_with(
        Arc::new(StallingStt),
        None,
        Mode::Auto,
        Duration::from_millis(400),
        Duration::from_millis(400),
    )
    .await;
    let mut stuck = connect(live.port, "?session=stuck").await;
    let _hello = next_text(&mut stuck).await;
    match live.event().await {
        VoiceEvent::Connected { session_id, .. } => assert_eq!(session_id, "stuck"),
        other => panic!("expected Connected, got {}", label(&other)),
    }
    wedge(&mut live, &mut stuck, "stuck").await;

    let next_port = free_port();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    live.reconfig_tx
        .send(VoiceReconfig::Rebind {
            bind: "127.0.0.1".to_string(),
            port: next_port,
            ack: ack_tx,
        })
        .await
        .expect("send");
    let verdict = tokio::time::timeout(Duration::from_secs(2), ack_rx)
        .await
        .expect("the rebind is answered even though a client is wedged")
        .expect("the ack channel stays open");
    assert!(verdict.is_ok(), "the new address binds: {verdict:?}");

    match live.event().await {
        VoiceEvent::Disconnected { session_id } => assert_eq!(session_id, "stuck"),
        other => panic!(
            "the rebind reports the row it removed, got {} — a connection of the old address \
             must not outlive it in the table",
            label(&other)
        ),
    }
    match live.event().await {
        VoiceEvent::Bound(addr) => assert!(addr.ends_with(&next_port.to_string()), "{addr}"),
        other => panic!("expected Bound after the disconnect, got {}", label(&other)),
    }
}

// ------------------------------------------------- outbound framing (i-framing)

/// Every binary frame up to the next text frame, and that text frame.
async fn audio_until_text(ws: &mut Ws) -> (Vec<Vec<u8>>, Value) {
    let mut frames = Vec::new();
    loop {
        let msg = tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("a frame arrives")
            .expect("the stream stays open")
            .expect("a readable frame");
        match msg {
            WsMessage::Binary(b) => frames.push(b.to_vec()),
            WsMessage::Text(t) => {
                return (
                    frames,
                    meclaw_core::serde_json::from_str(&t).expect("the frame is json"),
                );
            }
            _ => continue,
        }
    }
}

/// Start one synthesis on a fresh connection and take everything it produced.
async fn speak_and_collect(live: &mut Live) -> (Vec<Vec<u8>>, Value) {
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;
    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let start = next_text(&mut ws).await;
    assert_eq!(start["type"], json!("speak_start"));
    audio_until_text(&mut ws).await
}

/// The finding this knob exists for: a provider chunk of 40 KB reaches the
/// client as 20 ms frames, because a phone edge (FreeSWITCH `mod_audio_stream`
/// 1.0.3) aborts the call on a binary frame longer than about 100 ms.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_big_synthesis_chunk_leaves_in_twenty_millisecond_frames() {
    let seen = Arc::new(AtomicUsize::new(0));
    let chunk: Vec<u8> = (0..40_960u32).map(|i| (i % 251) as u8).collect();
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(BulkTts::new(vec![chunk.clone()]))),
        Mode::Auto,
    )
    .await;

    let (frames, end) = speak_and_collect(&mut live).await;
    assert_eq!(
        end,
        json!({"type": "speak_end", "speak_id": "s1", "reason": "done"})
    );
    assert_eq!(
        frames.len(),
        43,
        "42 full frames of 960 B plus a 640 B tail"
    );
    for (i, frame) in frames.iter().enumerate() {
        assert!(
            frame.len() <= 960,
            "frame {i} is {} bytes — a phone edge dies on this",
            frame.len()
        );
        assert_eq!(frame.len() % 2, 0, "frame {i} was cut through a sample");
        if i + 1 < frames.len() {
            assert_eq!(frame.len(), 960, "only the last frame may be short");
        }
    }
    assert_eq!(
        frames.concat(),
        chunk,
        "the same bytes in the same order — framing loses nothing"
    );
}

/// A chunk that does not end on a sample boundary: the odd byte travels with
/// the next chunk rather than being cut through or dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_odd_chunk_hands_its_last_byte_to_the_next_one() {
    let seen = Arc::new(AtomicUsize::new(0));
    let first = vec![1u8; 961];
    let second = vec![2u8; 959];
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(BulkTts::new(vec![first.clone(), second.clone()]))),
        Mode::Auto,
    )
    .await;

    let (frames, end) = speak_and_collect(&mut live).await;
    assert_eq!(end["reason"], json!("done"));
    assert_eq!(frames.len(), 2, "961 + 959 is exactly two full frames");
    assert!(frames.iter().all(|f| f.len() == 960), "got {frames:?}");
    assert_eq!(
        frames[1][0], 1,
        "the odd byte of the first chunk leads the second frame"
    );
    assert!(frames[1][1..].iter().all(|b| *b == 2));
    assert_eq!(frames.concat(), [first, second].concat());
}

/// `audio_out_frame_ms: 0` is the passthrough: one provider chunk, one frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn framing_zero_sends_the_provider_chunk_unchanged() {
    let seen = Arc::new(AtomicUsize::new(0));
    let chunk: Vec<u8> = (0..40_960u32).map(|i| (i % 251) as u8).collect();
    let mut live = start_framed(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(BulkTts::new(vec![chunk.clone()]))),
        Mode::Auto,
        Duration::from_secs(5),
        Duration::from_secs(5),
        0,
    )
    .await;

    let mut ws = connect(live.port, "?session=abc").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_out_frame_ms"], json!(0));
    let _connected = live.event().await;
    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let _start = next_text(&mut ws).await;
    let (frames, end) = audio_until_text(&mut ws).await;
    assert_eq!(end["reason"], json!("done"));
    assert_eq!(frames, vec![chunk], "one chunk, one frame, nothing cut");
}

/// A cancel discards the held part-sample with the rest of the synthesis
/// (R-V11): nothing of it is flushed after the verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_discards_what_the_framer_still_holds() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        // 961 bytes: one whole frame goes out, one byte stays behind.
        Some(Arc::new(BulkTts::hanging(vec![vec![5u8; 961]]))),
        Mode::Auto,
    )
    .await;
    let mut ws = connect(live.port, "?session=abc").await;
    let _hello = next_text(&mut ws).await;
    let _connected = live.event().await;
    live.reconfig_tx
        .send(VoiceReconfig::Speak {
            session_id: "abc".to_string(),
            speak_id: "s1".to_string(),
            text: "hallo".to_string(),
        })
        .await
        .expect("send");
    let _start = next_text(&mut ws).await;
    assert_eq!(next_binary(&mut ws).await.len(), 960);

    live.reconfig_tx
        .send(VoiceReconfig::CancelSpeak {
            session_id: "abc".to_string(),
        })
        .await
        .expect("send");
    let end = next_text(&mut ws).await;
    assert_eq!(
        end,
        json!({"type": "speak_end", "speak_id": "s1", "reason": "cancelled"})
    );
    match live.event().await {
        VoiceEvent::SpeakEnded { reason, .. } => assert_eq!(reason, SpeakEndReason::Cancelled),
        other => panic!("expected SpeakEnded, got {}", label(&other)),
    }
    let after = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
    assert!(
        after.is_err(),
        "the held byte was discarded with the synthesis, not flushed: {after:?}"
    );
}

/// Both declarations carry the frame length, so a client reads it instead of
/// measuring it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hello_and_info_declare_the_outbound_frame_length() {
    let seen = Arc::new(AtomicUsize::new(0));
    let mut live = start(
        Arc::new(ScriptedStt::draining(seen)),
        Some(Arc::new(BulkTts::new(vec![vec![0u8; 4]]))),
        Mode::Auto,
    )
    .await;

    let info: Value = meclaw_core::serde_json::from_str(
        &reqwest::get(format!("http://127.0.0.1:{}/info", live.port))
            .await
            .expect("GET /info")
            .text()
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(info["audio_out_frame_ms"], json!(20), "the shipped default");
    assert_eq!(info["audio_out"]["sample_rate"], json!(24000));

    let mut ws = connect(live.port, "?session=abc").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_out_frame_ms"], json!(20));
    let _connected = live.event().await;
}

// ------------------------------------------------- speech text (`speak_plain`)

/// A synthesis provider that keeps the text it was asked to speak and makes no
/// sound at all — what reaches it is the whole question here.
struct RecordingTts {
    spoken: mpsc::Sender<String>,
}

impl TtsProvider for RecordingTts {
    fn name(&self) -> &'static str {
        "recording-tts"
    }
    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }
    fn synthesize(
        &self,
        _format: AudioFormat,
        text: String,
        _audio: mpsc::Sender<Vec<u8>>,
        _cancel: watch::Receiver<bool>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let spoken = self.spoken.clone();
        Box::pin(async move {
            let _ = spoken.send(text).await;
            Ok(())
        })
    }
}

/// An in-memory `cell.db` with the overlay table a params update writes to.
fn cell_db() -> DbConn {
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS params (
             key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
    )
    .expect("the overlay table");
    DbConn::wrap(conn, None)
}

/// The whole cell — handler half and I/O half — around a recording provider,
/// driven the way a colony drives it: a client on the socket, one `in_speak`
/// message into `handle` per entry of `written`. Answers with the FIRST text
/// the provider was handed, which is what makes "this answer was not spoken at
/// all" a positive receipt: send the silent one first and a real one behind it,
/// and the provider's first text is the second answer.
///
/// The handler is what rewrites markdown, so nothing shorter than this proves
/// it: a command put on the I/O half's seam by hand would never pass the place
/// where the rewriting happens.
async fn text_a_provider_is_handed(speak_plain: Option<bool>, written: &[&str]) -> String {
    let path = Path::new("/main/members/tester/channels/voice");
    let port = free_port();
    let mut raw = json!({
        "port": port,
        "stt": {"provider": "deepgram", "api_key": "k"},
        "tts": {"provider": "cartesia", "api_key": "k", "voice": "v"},
    });
    if let Some(flag) = speak_plain {
        raw["speak_plain"] = json!(flag);
    }
    let params = VoiceParams::parse(&raw).expect("the config parses");

    let (spoken_tx, mut spoken_rx) = mpsc::channel::<String>(4);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    // The two providers are this file's own: the recogniser drains, and the
    // synthesiser answers the one question the test asks.
    let io = VoiceIo::new(
        params.bind.clone(),
        params.port,
        Arc::new(ScriptedStt::draining(Arc::new(AtomicUsize::new(0)))),
        Some(Arc::new(RecordingTts { spoken: spoken_tx })),
        params.default_mode,
        Duration::from_millis(params.external_timeout_ms),
        Duration::from_millis(params.provider_idle_timeout_ms),
        events_tx.clone(),
    );
    let mut cell = VoiceCell::new(path.clone(), io, &params, &raw);
    let io = LongRunningCell::split_io(&mut cell);
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(8);
    let task = tokio::spawn(<VoiceCell as LongRunningCell>::run_io(
        io,
        events_tx,
        reconfig_rx,
    ));

    let mut next_event = async || {
        tokio::time::timeout(MARKER, events_rx.recv())
            .await
            .expect("the I/O half pushes an event")
            .expect("the events channel stays open")
    };
    match next_event().await {
        VoiceEvent::Bound(_) => {}
        other => panic!("expected Bound, got {}", label(&other)),
    }

    let mut ws = connect(port, "?session=call-1").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["session_id"], json!("call-1"));
    assert_eq!(
        hello["speak_plain"],
        json!(speak_plain.unwrap_or(true)),
        "the declaration says what will happen to the text: {hello}"
    );

    let mut db = cell_db();
    let (out_tx, mut out_rx) = mpsc::channel::<meclaw_core::CellEmission>(16);
    let origin = OriginSink::new(out_tx.clone(), path.clone(), 64);
    let out = OutputSink::new(
        out_tx,
        path.clone(),
        meclaw_core::Uuid::now_v7(),
        meclaw_core::Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let (reconfig_tx, _reconfig_rx) = mpsc::channel(8);
    let connected = next_event().await;
    assert!(
        matches!(connected, VoiceEvent::Connected { .. }),
        "expected Connected, got {}",
        label(&connected)
    );
    cell.handle_event(connected, &origin, &mut db).await;

    for answer in written {
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("session_id".to_string(), json!("call-1"));
        let speak = meclaw_core::MessageBuilder::new(path.clone())
            .context(ctx)
            .body(meclaw_core::Body::Inline(json!({
                "messages": [{"origin": "assistant", "type": "text", "text": answer}]
            })))
            .build();
        cell.handle(speak, &out, &mut db, &reconfig_tx).await;
        if let Ok(em) = out_rx.try_recv() {
            panic!("the speak order was refused: {:?}", em.content);
        }
    }

    let spoken = tokio::time::timeout(MARKER, spoken_rx.recv())
        .await
        .expect("the synthesis provider is asked within 30s")
        .expect("the provider channel stays open");
    task.abort();
    spoken
}

/// The knob's whole point: an assistant writes markdown, and a provider is
/// handed speech.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speak_plain_hands_the_provider_speech_and_not_markdown() {
    assert_eq!(
        text_a_provider_is_handed(None, &["**Hallo** Welt"]).await,
        "Hallo Welt",
        "the shipped default rewrites, and the stars never reach the provider"
    );
}

/// An answer that is nothing but markup has nothing to say, and is not queued.
///
/// The receipt is positive: the answer BEHIND it is the first text the provider
/// sees, which proves both halves at once — the silent one never reached a
/// synthesis, and the queue it did not enter still works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_with_no_words_left_is_not_spoken_at_all() {
    assert_eq!(
        text_a_provider_is_handed(None, &["---", "Guten Tag"]).await,
        "Guten Tag",
        "a horizontal rule is not a sentence, and the answer after it is"
    );
}

/// And off is off: the answer arrives at the provider exactly as it was
/// written, which is the behaviour this cell had before the knob existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speak_plain_false_hands_the_text_over_untouched() {
    assert_eq!(
        text_a_provider_is_handed(Some(false), &["**Hallo** Welt"]).await,
        "**Hallo** Welt"
    );
}

// ------------------------------------------------ GH #619: the negotiated rate

/// A recognition provider that serves a list of rates and remembers the one it
/// was actually run at.
struct MultiRateStt {
    rates: Vec<u32>,
    ran_at: Arc<AtomicUsize>,
}

impl MultiRateStt {
    fn new(rates: Vec<u32>, ran_at: Arc<AtomicUsize>) -> Self {
        Self { rates, ran_at }
    }
}

impl SttProvider for MultiRateStt {
    fn name(&self) -> &'static str {
        "multirate"
    }
    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.rates[0])
    }
    fn input_rates(&self) -> Vec<u32> {
        self.rates.clone()
    }
    fn negotiate_input(&self, sample_rate: u32) -> Option<AudioFormat> {
        self.rates
            .contains(&sample_rate)
            .then(|| AudioFormat::pcm16_mono(sample_rate))
    }
    fn run_session(
        &self,
        format: AudioFormat,
        mut audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        _liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let ran_at = self.ran_at.clone();
        Box::pin(async move {
            ran_at.store(format.sample_rate as usize, Ordering::SeqCst);
            let _events = events;
            while audio.recv().await.is_some() {}
            Ok(())
        })
    }
}

/// A synthesis provider fixed at one rate: what a client asks for beyond it is
/// a wish the cell cannot grant.
struct FixedRateTts {
    rate: u32,
}

impl TtsProvider for FixedRateTts {
    fn name(&self) -> &'static str {
        "fixed-tts"
    }
    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.rate)
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

/// GH #619: a telephony edge says what it sends, and the recognition session
/// runs at that rate instead of at the one the template happened to name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_negotiates_the_rate_it_sends() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at.clone())),
        Some(Arc::new(FixedRateTts { rate: 8_000 })),
        Mode::Auto,
    )
    .await;

    let mut ws = connect(live.port, "?session=phone&sample_rate=8000").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(
        hello["audio_in"],
        json!({"encoding": "pcm_s16le", "sample_rate": 8000, "channels": 1}),
        "the declaration is the negotiated pair, not the provider default: {hello}"
    );
    assert_eq!(
        hello["audio_out"]["sample_rate"],
        json!(8000),
        "a call answered at 8 kHz is spoken back to at 8 kHz: {hello}"
    );

    // One frame is enough to prove the session was started at all.
    send_binary(&mut ws, vec![0u8; 320]).await;
    for _ in 0..100 {
        if ran_at.load(Ordering::SeqCst) != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        ran_at.load(Ordering::SeqCst),
        8_000,
        "the recognition session runs at the negotiated rate, not at the declared one"
    );
}

/// The cell never resamples (R-V2), so a rate the recogniser does not serve is
/// a refused connection — answered before the upgrade, like a bad `session`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_the_recogniser_cannot_serve_is_refused_before_the_upgrade() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at)),
        None,
        Mode::Auto,
    )
    .await;

    let res = reqwest::get(format!(
        "http://127.0.0.1:{}/ws?sample_rate=11025",
        live.port
    ))
    .await
    .expect("GET /ws");
    assert_eq!(res.status(), 400);
    let body = res.text().await.expect("body");
    assert!(
        body.contains("11025") && body.contains("8000") && body.contains("16000"),
        "the refusal names what was asked for and what is on offer: {body}"
    );

    assert!(
        tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/ws?sample_rate=11025",
            live.port
        ))
        .await
        .is_err(),
        "and the websocket handshake does not come up either"
    );
}

/// The two directions are negotiated separately: what a client sends has to be
/// understood, what it is sent it can merely dislike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outbound_rate_falls_back_to_what_the_synthesiser_serves() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at)),
        Some(Arc::new(FixedRateTts { rate: 24_000 })),
        Mode::Auto,
    )
    .await;

    let mut ws = connect(live.port, "?sample_rate=8000").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(
        hello["audio_in"]["sample_rate"],
        json!(8000),
        "the inbound wish is binding: {hello}"
    );
    assert_eq!(
        hello["audio_out"]["sample_rate"],
        json!(24000),
        "the outbound wish is not: the provider's own rate stands, and is declared: {hello}"
    );
}

/// R-V6: what a client MAY ask for is a read, not a refusal it has to provoke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn info_lists_every_rate_that_can_be_negotiated() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at)),
        Some(Arc::new(FixedRateTts { rate: 24_000 })),
        Mode::Auto,
    )
    .await;

    let res = reqwest::get(format!("http://127.0.0.1:{}/info", live.port))
        .await
        .expect("GET /info");
    let info: Value =
        meclaw_core::serde_json::from_str(&res.text().await.expect("body")).expect("json");
    assert_eq!(info["audio_in_rates"], json!([16000, 8000]));
    assert_eq!(info["audio_out_rates"], json!([24000]));
    assert_eq!(
        info["audio_in"]["sample_rate"],
        json!(16000),
        "and `audio_in` stays what a client that asks for nothing gets: {info}"
    );
}

/// An encoding this version does not speak is refused rather than assumed: a
/// telephony edge configured for companded audio would otherwise send bytes
/// that are perfectly valid noise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_encoding_this_version_does_not_speak_is_refused() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at)),
        None,
        Mode::Auto,
    )
    .await;

    let res = reqwest::get(format!(
        "http://127.0.0.1:{}/ws?sample_rate=8000&encoding=mulaw",
        live.port
    ))
    .await
    .expect("GET /ws");
    assert_eq!(res.status(), 400);
    assert!(
        res.text().await.expect("body").contains("pcm_s16le"),
        "the refusal names the one encoding this version has"
    );

    // And the one it does speak passes, spelled the way `hello` spells it.
    let mut ws = connect(live.port, "?sample_rate=8000&encoding=pcm_s16le").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_in"]["encoding"], json!("pcm_s16le"));
}

/// Nothing moves for a client that asks for nothing — the parameter is
/// additive, and every existing colony keeps the rates it had.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_rate_the_providers_own_declaration_stands() {
    let ran_at = Arc::new(AtomicUsize::new(0));
    let live = start(
        Arc::new(MultiRateStt::new(vec![16_000, 8_000], ran_at.clone())),
        Some(Arc::new(FixedRateTts { rate: 24_000 })),
        Mode::Auto,
    )
    .await;

    let mut ws = connect(live.port, "?session=browser").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_in"]["sample_rate"], json!(16000), "{hello}");
    assert_eq!(hello["audio_out"]["sample_rate"], json!(24000), "{hello}");

    send_binary(&mut ws, vec![0u8; 320]).await;
    for _ in 0..100 {
        if ran_at.load(Ordering::SeqCst) != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(ran_at.load(Ordering::SeqCst), 16_000);
}

/// The loopback does not answer for a synthesis provider that exists.
///
/// `stt: "echo"` used to declare `audio_out = audio_in` unconditionally, and a
/// cell configured with an echo recogniser AND a synthesis block then announced
/// the input rate while `start_speak` — which reads `shared.tts` and never asks
/// which recogniser is in front of it — put the provider's own rate on the
/// socket. The declaration has to be the one a client can act on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_echo_cell_with_a_synthesis_provider_declares_the_providers_rate() {
    let live = start(
        Arc::new(ScriptedStt::echo()),
        Some(Arc::new(FixedRateTts { rate: 24_000 })),
        Mode::Auto,
    )
    .await;

    let mut ws = connect(live.port, "?session=loop").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_in"]["sample_rate"], json!(16000), "{hello}");
    assert_eq!(
        hello["audio_out"]["sample_rate"],
        json!(24000),
        "the synthesis provider is asked even behind an echo recogniser: {hello}"
    );

    // And with nothing to synthesise with, the loopback is its own answer again.
    let bare = start(Arc::new(ScriptedStt::echo()), None, Mode::Auto).await;
    let mut ws = connect(bare.port, "?session=bare").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(
        hello["audio_out"], hello["audio_in"],
        "what goes in comes back: {hello}"
    );

    let res = reqwest::get(format!("http://127.0.0.1:{}/info", bare.port))
        .await
        .expect("GET /info");
    let info: Value =
        meclaw_core::serde_json::from_str(&res.text().await.expect("body")).expect("json");
    assert_eq!(
        info["audio_out_rates"], info["audio_in_rates"],
        "a `null` rate list beside a set `audio_out` is a contradiction: {info}"
    );
}

/// The trait's DEFAULT refusal, which every service test above steps around by
/// using a provider that serves several rates. A provider that declares one
/// rate and overrides nothing serves that rate and refuses the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_overrides_nothing_serves_exactly_its_own_rate() {
    let seen = Arc::new(AtomicUsize::new(0));
    let live = start(Arc::new(ScriptedStt::draining(seen)), None, Mode::Auto).await;

    let res = reqwest::get(format!(
        "http://127.0.0.1:{}/ws?sample_rate=8000",
        live.port
    ))
    .await
    .expect("GET /ws");
    assert_eq!(res.status(), 400);
    let body = res.text().await.expect("body");
    assert!(
        body.contains("8000") && body.contains("16000"),
        "the refusal names what was asked for and the one rate on offer: {body}"
    );

    // Its own rate passes, and `/info` says so without being asked twice.
    let mut ws = connect(live.port, "?sample_rate=16000").await;
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["audio_in"]["sample_rate"], json!(16000), "{hello}");

    let res = reqwest::get(format!("http://127.0.0.1:{}/info", live.port))
        .await
        .expect("GET /info");
    let info: Value =
        meclaw_core::serde_json::from_str(&res.text().await.expect("body")).expect("json");
    assert_eq!(info["audio_in_rates"], json!([16000]));
}
