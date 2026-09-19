//! GH #743 — the silence tail is only worth as long as the grace pays for.
//!
//! #717 gave a released boundary its own silence, and this file is the number
//! that silence has to be fed for. Measured on the owner's colony 18.09.2026,
//! three machine-timed holds against one fixture: the provider's `EndOfTurn`
//! arrives about **1795 ms after the last spoken word**, not 400-700 ms after
//! it — that shorter figure is the model's latency AFTER its endpointing
//! threshold has already fired, and the threshold itself is measured in the
//! audio stream. A hold let go 1.24 s after the last word closed on the
//! provider at +555 ms and carried all 58 characters; a hold let go 0.12 s
//! after the last word closed on the cap at +1515 ms and carried 37. Every
//! human hold of that day ended at +1501/+1502 ms — on the cap, never on the
//! provider's end.
//!
//! So the tail was long enough to reach the provider and too short to reach
//! its answer. The claim locked here:
//!
//! > A turn released 20 ms after its last word carries the whole sentence even
//! > when the provider needs 1.8 s to say the turn is over — with the grace
//! > THIS TREE SHIPS, not with a number a test invented.
//!
//! Hence `DEFAULT_RELEASE_GRACE_MS` rather than a literal: the lock grades the
//! delivered default, which is the thing that was wrong. No sleep decides it
//! either — the scripted provider holds its `EndOfTurn` behind an audio gate
//! 90 frames (1.8 s) wide that the test itself never opens. Only the cell's
//! own silence can, and only if it is paid for long enough.
//!
//! The cap-as-backstop claim of #717 is unchanged and stays in that file.
//!
//! Where this lock gives way first: the silence tail re-arms from the moment it
//! wakes (`connection.rs`), so drift accumulates over its ticks. Nominally it
//! needs 1.78 s and the cap stands at 2.5 s, which is 720 ms of room, about
//! 28 ms a tick. That is comfortable on a loaded runner and it is also the
//! point — a default that ever went back below about 2.0 s would turn red here
//! before it was felt on a phone.

use meclaw_cells::voice::cell::{VoiceCell, VoiceReconfig};
use meclaw_cells::voice::contract::{AudioFormat, SttError, SttEvent, SttProvider};
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::{DEFAULT_RELEASE_GRACE_MS, VoiceParams};
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

/// How much silence the provider waits for before it ends the turn: 90 frames
/// of 20 ms, which is the 1.8 s measured between the last word and the
/// `EndOfTurn` on the owner's colony. The old default paid for 75 of them.
const END_GATE_FRAMES: usize = 90;

/// The grace the tree shipped before this issue, and the one the runs above
/// kept landing on. Named so the assertion can say what it means rather than
/// carry a bare number.
const OLD_GRACE: Duration = Duration::from_millis(1500);

/// A recognition provider on a script, with an audio gate in front of the end.
///
/// It reports an interim once `FIRST_GATE` bytes have arrived and ends the turn
/// once `end_gate` bytes have. It counts every byte it was given, which is how
/// the test sees the silence tail without reading a clock.
struct ScriptedStt {
    interim: String,
    final_text: String,
    /// Total bytes that must have arrived before the turn ends.
    end_gate: usize,
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
                if !ended && total >= end_gate {
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

/// The whole find: the provider takes 1.8 s to end the turn, the key comes up
/// 20 ms after the last word, and the shipped default still carries the end of
/// the sentence.
///
/// The `EndOfTurn` sits behind a gate of 91 frames. The client opens two of
/// them — the word, and the 20 ms after it. The other 89 can only be the cell's
/// own silence, and only a grace longer than 1.78 s pays for that many.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_takes_its_time_keeps_the_sentence() {
    let gate = FIRST_GATE + END_GATE_FRAMES * FRAME_BYTES;
    let live = boot(
        ScriptedStt {
            interim: "there were sixteen people on the call and".to_string(),
            final_text: "there were sixteen people on the call and none of them agreed".to_string(),
            end_gate: gate,
            received: Arc::new(AtomicUsize::new(0)),
        },
        DEFAULT_RELEASE_GRACE_MS,
    )
    .await;
    let mut client = connect(&live, "session=grace-1&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; FIRST_GATE])
        .await
        .expect("the last word");
    wait_for_partial(&mut client, "sixteen people").await;
    // The 20 ms a person's hand is still on the key after the last word. This
    // is the take that was cut short on the owner's colony; the run that
    // survived held on for 1.24 s, which is the workaround this lock removes.
    client
        .send_audio(&vec![0u8; FRAME_BYTES])
        .await
        .expect("the tail of the hold");

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
        turn["text"], "there were sixteen people on the call and none of them agreed",
        "the end of the sentence belongs to this turn; the interim alone is \
         the cap having cut a take the provider never got to finish"
    );
    assert!(
        waited >= OLD_GRACE,
        "this turn closed on the provider's own end, and that end is LATER \
         than the grace this tree used to ship — a run that closed inside \
         {OLD_GRACE:?} would not be measuring what #743 is about: {waited:?}"
    );
    assert!(
        live.received.load(Ordering::SeqCst) >= gate,
        "the provider was fed past the gate by the cell: {} of {gate}",
        live.received.load(Ordering::SeqCst),
    );
}
