//! GH #717 — a turn let go right after the last word keeps that word.
//!
//! Measured on the fresh instance during the human acceptance of wave H
//! (18.09.2026). A held turn lost its ending, and neither the client nor the
//! speaker was at fault: an endpointing provider ends a turn after a stretch of
//! silence IN THE AUDIO STREAM (Deepgram Flux: `eot_threshold`, 0.7 s), and the
//! client stops sending about 120 ms after `release`. So nothing reached the
//! provider afterwards, no `EndOfTurn` ever came, and the boundary was cut at
//! `release_grace_ms` with the interim transcript — which lags the audio, so
//! the end of the sentence was missing. Two machine-timed runs against one
//! fixture: released 20 ms after the last word → cut at +1501 ms, transcript
//! EMPTY; released 1.62 s later, with silence still streaming → cut at
//! +137 ms, transcript complete. The maintainer's own turn: released
//! 08:01:31.337, closed 08:01:32.838, exactly the cap.
//!
//! The fix is in the cell, not in the client: from the `release` on, the
//! connection feeds the provider 20-ms frames of silence until the provider
//! ends the turn or the grace runs out. The two claims locked here:
//!
//! 1. A turn released right after its last word carries the WHOLE transcript
//!    and closes well before the cap.
//! 2. A provider that never ends anything still hits the cap, and the silence
//!    stops there — the cap is the backstop it always was, and the tail is not
//!    a loop that runs for the rest of the call.
//!
//! No sleep decides claim 1: the scripted provider holds its `EndOfTurn`
//! behind an audio gate the test never opens itself. Only the cell's own
//! silence can open it.

use meclaw_cells::voice::cell::{VoiceCell, VoiceReconfig};
use meclaw_cells::voice::contract::{AudioFormat, SttError, SttEvent, SttProvider};
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_cells::voice::wire::Mode;
use meclaw_colony::{DbConn, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, OriginSink, Path};
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The repo's failure-marker window: generous, never a discriminator.
const MARKER: Duration = Duration::from_secs(30);

/// The mount this file's cell registers.
const MOUNT: &str = "voice";

/// 16 kHz mono PCM16: 640 bytes is 20 ms, the frame this wire recommends and
/// the size the cell's own silence frames have to be.
const RATE: u32 = 16_000;
const FRAME_BYTES: usize = 640;

/// How much audio the scripted provider waits for before it says anything.
const FIRST_GATE: usize = FRAME_BYTES;

/// A recognition provider on a script, with an audio gate in front of the end.
///
/// It reports an interim once `FIRST_GATE` bytes have arrived and ends the turn
/// once `end_gate` bytes have — or never, when `end_gate` is `None`. It counts
/// every byte it was given, which is how the test sees the silence tail without
/// reading a clock.
struct ScriptedStt {
    interim: String,
    final_text: String,
    /// Total bytes that must have arrived before the turn ends. `None` for a
    /// provider that ends nothing, ever.
    end_gate: Option<usize>,
    received: Arc<AtomicUsize>,
}

impl SttProvider for ScriptedStt {
    /// Anything but `echo`: `echo` is the one name the connection reads as
    /// "there is no recognition session at all".
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(RATE)
    }

    fn run_session(
        &self,
        _format: AudioFormat,
        mut audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: meclaw_colony::io_liveness::IoLivenessMark,
    ) -> meclaw_cells::voice::contract::BoxFuture<Result<(), SttError>> {
        let interim = self.interim.clone();
        let final_text = self.final_text.clone();
        let end_gate = self.end_gate;
        let received = Arc::clone(&self.received);
        Box::pin(async move {
            let mut total = 0usize;
            let mut said_interim = false;
            let mut ended = false;
            while let Some(chunk) = audio.recv().await {
                total += chunk.len();
                received.store(total, Ordering::SeqCst);
                liveness.mark_success();
                if !said_interim && total >= FIRST_GATE {
                    said_interim = true;
                    let _ = events.send(SttEvent::SpeechStarted).await;
                    let _ = events
                        .send(SttEvent::Partial {
                            text: interim.clone(),
                            eager: false,
                        })
                        .await;
                }
                if let Some(gate) = end_gate
                    && !ended
                    && total >= gate
                {
                    ended = true;
                    let _ = events
                        .send(SttEvent::EndOfTurn {
                            text: final_text.clone(),
                        })
                        .await;
                }
            }
            Ok(())
        })
    }
}

/// One `voice` cell with both halves running, and nothing else: no colony, no
/// topology. The turn frame this file asserts on is the one the CLIENT is
/// answered with, which is the same boundary the `turn` lane carries.
struct Live {
    ws_base: String,
    received: Arc<AtomicUsize>,
    handler: tokio::task::JoinHandle<()>,
    io: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    /// Held: the I/O half reads this channel closing as "the handler is gone"
    /// and shuts down, so dropping the sender would take the listener with it.
    _reconfig: mpsc::Sender<VoiceReconfig>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.handler.abort();
        self.io.abort();
        self.listener.abort();
    }
}

/// Boot a cell around `stt`, with the given `release_grace_ms`.
async fn boot(stt: ScriptedStt, release_grace_ms: u64) -> Live {
    let received = Arc::clone(&stt.received);
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = mpsc::channel(256);

    // The params are the cell's own declaration; the provider object comes
    // from the I/O half. `echo` is what a params document may say without a
    // credential, and the connection's echo path keys off the PROVIDER's name
    // rather than off this.
    let raw = json!({
        "mount": MOUNT,
        "default_mode": "hold",
        "release_grace_ms": release_grace_ms,
        "emit_partials": true,
        "stt": {"provider": "echo"}
    });
    let params = VoiceParams::parse(&raw).expect("params");

    let mut io = VoiceIo::new(
        MOUNT.to_string(),
        Arc::new(stt),
        None,
        Mode::Hold,
        Duration::from_secs(5),
        Duration::from_secs(30),
        events_tx.clone(),
    );
    io.cell_path = Path::new("/voice");
    io.surfaces = Arc::clone(&surfaces);

    let mut cell = VoiceCell::new(Path::new("/voice"), io, &params, &raw);
    let io_half = cell.split_io();
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(64);
    let io_task = tokio::spawn(VoiceCell::run_io(io_half, events_tx, reconfig_rx));

    // The handler loop the substrate would run. Its emissions go nowhere this
    // file looks — the claim is about the boundary, and the client is told
    // about that on its own socket.
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(256);
    let sink = OriginSink::new(out_tx, Path::new("/voice"), 8);
    let mut db = DbConn::wrap(rusqlite::Connection::open_in_memory().expect("open"), None);
    let handler = tokio::spawn(async move {
        while let Some(event) = events_rx.recv().await {
            cell.handle_event(event, &sink, &mut db).await;
        }
    });

    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cell registers its mount within the failure marker");

    Live {
        ws_base: format!("ws://{addr}/{MOUNT}"),
        received,
        handler,
        io: io_task,
        listener,
        _reconfig: reconfig_tx,
    }
}

/// Connect, retrying while the listener is still coming up.
async fn connect(live: &Live, query: &str) -> VoiceClient {
    let url = format!("{}/ws?{query}", live.ws_base);
    let deadline = Instant::now() + MARKER;
    loop {
        match VoiceClient::connect(&url).await {
            Ok((client, _hello)) => return client,
            Err(e) if Instant::now() >= deadline => panic!("never connected to {url}: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
}

/// Wait for the interim the script sends, so the release below is a release of
/// a take the provider has already started on.
async fn wait_for_partial(client: &mut VoiceClient, needle: &str) {
    let seen = client
        .collect_until(
            |f| {
                f.is_type("partial")
                    && f.as_text()
                        .and_then(|v| v.get("text"))
                        .and_then(Value::as_str)
                        .map(|t| t.contains(needle))
                        .unwrap_or(false)
            },
            MARKER,
        )
        .await;
    assert!(
        seen.iter().any(|(_, f)| f.is_type("partial")),
        "the script must have played before the key comes up, got {seen:?}"
    );
}

/// The whole find: the key comes up right after the last word, the client sends
/// nothing more, and the turn still carries the end of the sentence.
///
/// The provider's `EndOfTurn` sits behind a gate at four frames of audio. The
/// test opens one of them. The other three can only be the cell's own silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_released_after_its_last_word_carries_the_whole_sentence() {
    let live = boot(
        ScriptedStt {
            interim: "there were sixteen people".to_string(),
            final_text: "there were sixteen people on the call".to_string(),
            end_gate: Some(FIRST_GATE + 3 * FRAME_BYTES),
            received: Arc::new(AtomicUsize::new(0)),
        },
        1500,
    )
    .await;
    let mut client = connect(&live, "session=tail-1&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; FIRST_GATE])
        .await
        .expect("audio");
    wait_for_partial(&mut client, "sixteen people").await;

    let released = Instant::now();
    client.release().await.expect("let the key go");
    // Nothing else is sent. Whatever reaches the provider from here on is the
    // cell's.
    let turn = client
        .next_frame_of_type("turn", MARKER)
        .await
        .expect("the boundary is answered");
    let waited = released.elapsed();

    assert_eq!(
        turn["text"], "there were sixteen people on the call",
        "the end of the sentence belongs to this turn; the interim alone is \
         the cap having cut a take the provider never got to finish"
    );
    assert!(
        waited < Duration::from_millis(1500),
        "the turn closed on the provider's own end, not on the grace cap: \
         {waited:?}"
    );
    assert!(
        live.received.load(Ordering::SeqCst) >= FIRST_GATE + 3 * FRAME_BYTES,
        "the provider was fed past the gate by the cell: {} of {}",
        live.received.load(Ordering::SeqCst),
        FIRST_GATE + 3 * FRAME_BYTES
    );
}

/// The backstop stands: a provider that ends nothing still has its boundary cut
/// at `release_grace_ms`, with the interim, and the silence stops with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_the_provider_never_ends_still_closes_at_the_cap() {
    let live = boot(
        ScriptedStt {
            interim: "a b".to_string(),
            final_text: String::new(),
            end_gate: None,
            received: Arc::new(AtomicUsize::new(0)),
        },
        200,
    )
    .await;
    let mut client = connect(&live, "session=tail-2&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; FIRST_GATE])
        .await
        .expect("audio");
    wait_for_partial(&mut client, "a b").await;

    let released = Instant::now();
    client.release().await.expect("let the key go");
    let turn = client
        .next_frame_of_type("turn", MARKER)
        .await
        .expect("the cap answers the boundary");
    let waited = released.elapsed();

    assert_eq!(turn["text"], "a b", "the cap cuts with what it has");
    assert!(
        waited >= Duration::from_millis(200),
        "the cap must not fire before the grace it was armed with: {waited:?}"
    );

    // And the tail is bounded by that same grace rather than by the call: the
    // provider stops being fed once the cap has passed. Asserted as a
    // standstill over a window, from above only — a loaded runner may be late
    // with the last frame, it may not keep sending them.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let settled = live.received.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live.received.load(Ordering::SeqCst),
        settled,
        "the silence tail ends with the grace; it is not a loop for the rest \
         of the call"
    );
}
