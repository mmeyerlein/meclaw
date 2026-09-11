//! Wave voice-cell, strand t5 — the echo wire, measured end to end.
//!
//! The real `EchoStt` behind the real listener, driven by the real
//! [`VoiceClient`] a smoke run uses. Nothing here is scripted or stood in for:
//! t4's own tests prove the I/O half against providers written for each test,
//! and this one proves the shipped echo provider and the shipped test client
//! against each other.
//!
//! **Why a timing number nobody asserts on.** The point of the echo wire is
//! calibration: it measures the socket, the frame path and the round trip with
//! no model in the way, so a slow first turn later can be blamed on the right
//! half. An assertion on that number would measure the test runner's load
//! instead, and the July telephony work is a long record of what happens when a
//! table is built out of numbers that were never n=10 with a p90 beside them.
//! So the number is printed with its median, p90 and max, and the receipt is
//! written by a person.
//!
//! The second half runs a real colony. `turns.rs`'s unit tests prove the
//! automaton on paper; these prove the composition — that a boundary the
//! automaton drew becomes exactly one emission on the `turn` lane, seen by a
//! listener, while the client sees the same shape mirrored on its socket.
//!
//! Two rules the assertions are shaped by:
//!
//! * **Positive receipts.** Every check names something that ARRIVED — a
//!   frame, an emission, a close code. "Nothing bad happened" is not a proof.
//! * **The turn boundary is a count, not a vibe.** One `turn` per `EndOfTurn`
//!   in `auto` and per `release` in `hold`; a `partial` never wears a
//!   `turn_id`, a `turn` never wears `eager`.

use meclaw_cells::voice::VoiceCellFactory;
use meclaw_cells::voice::cell::VoiceReconfig;
use meclaw_cells::voice::contract::{SttProvider, TtsProvider};
use meclaw_cells::voice::io::{VoiceIo, run_io};
use meclaw_cells::voice::providers::echo::EchoStt;
use meclaw_cells::voice::wire::Mode;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RespawnFn, SpawnedCellKind,
    bootstrap_from_filesystem, cell_task,
};
use meclaw_core::serde_json::Value;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_core::{Cell, CellEmission, JsonValue, OutputSink};
use meclaw_testing::ColonyHandle;
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The repo's failure-marker convention — generous, so a loaded runner cannot
/// turn a slow answer into a red test.
const MARKER: Duration = Duration::from_secs(30);
/// 640 bytes = 20 ms of 16 kHz mono PCM16, the chunk size the wire recommends.
const FRAME_BYTES: usize = 640;
/// Enough round trips for a median and a p90 to mean something. Ten was the
/// number at which the July measurements stopped contradicting themselves.
const CLICKS: usize = 50;

/// Context key that opens the colony's egress door for this file's listener.
/// The root hive routes the cell's emissions to itself and stamps this on the
/// way, so what dies there leaves the colony onto a channel a test holds — the
/// `MARK` pattern of `gh163`.
const MARK: &str = "voice_out";

/// The failure-marker deadline for colony work, same 30 s convention.
const DEADLINE: Duration = Duration::from_secs(30);

/// The amount of audio the scripted providers wait for before they say
/// anything: a transcript that appears before the audio proves nothing, and
/// waiting for it with a sleep is how a suite gets flaky.
const AUDIO_GATE: usize = FRAME_BYTES;

/// The lane the flood of [`backpressure_loses_nothing`] travels on.
const PARTIAL_LANE: &str = "partial";

/// The mount every cell in this file registers. Since `voice@2.0.0` it is the
/// only door, and each fixture puts one listener in front of it
/// ([`meclaw_testing::surface_listener`]).
const MOUNT: &str = "voice";

/// Mailbox of the paced listener — small on purpose. The colony default is
/// 1000 (`docs/config.md`, `mailbox_size`), which a flood of 500 would fit into
/// without ever making the router wait; four makes the block a structural
/// certainty instead of a hope.
const LISTENER_MAILBOX: usize = 4;

/// A mounted cell behind a listener of its own, with nothing else in the way
/// but the provider under test.
struct Live {
    /// `ws://<listener>/<mount>` — the prefix the socket route hangs off.
    ws_base: String,
    task: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    _reconfig_tx: mpsc::Sender<VoiceReconfig>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.task.abort();
        self.listener.abort();
    }
}

/// Start `run_io`, put a listener in front of it and wait for the mount.
async fn start(stt: Arc<dyn SttProvider>, tts: Option<Arc<dyn TtsProvider>>) -> Live {
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(64);
    let mut io = VoiceIo::new(
        MOUNT.to_string(),
        stt,
        tts,
        Mode::Auto,
        Duration::from_secs(5),
        Duration::from_secs(30),
        events_tx,
    );
    io.cell_path = Path::new("/voice");
    io.surfaces = Arc::clone(&surfaces);
    let task = tokio::spawn(run_io(io, reconfig_rx));
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    await_mount(&surfaces).await;
    // Held: dropping the sender would close the reconfig channel under the
    // I/O half, which is a shutdown signal, not an idle one.
    Live {
        ws_base: format!("ws://{addr}/{MOUNT}"),
        task,
        listener,
        _reconfig_tx: reconfig_tx,
    }
}

/// Wait until a cell has put its mount on `surfaces`.
async fn await_mount(surfaces: &Arc<meclaw_colony::SurfaceRegistry>) {
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cell registers its mount within the failure marker");
}

/// Nearest-rank percentile — every printed number is one that was measured,
/// not one interpolated between two that were.
fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (((ordered.len() as f64) * p / 100.0).ceil() as usize).clamp(1, ordered.len());
    ordered[rank - 1]
}

/// The echo wire is byte-neutral, in order, and answers every frame.
///
/// Three claims, and each one is a different defect if it stops holding: a
/// changed byte is a broken audio path, a missing answer is a dropped frame,
/// and an answer out of order is a reordering the client cannot repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn echo_roundtrip_is_byte_identical() {
    let live = start(Arc::new(EchoStt::new()), None).await;
    let (mut client, hello) =
        VoiceClient::connect(&format!("{}/ws?session=echo-calibration", live.ws_base))
            .await
            .expect("the echo wire accepts a connection");

    assert_eq!(hello["protocol"], "meclaw-voice/1");
    assert_eq!(hello["stt"], "echo");
    assert_eq!(
        hello["tts"],
        Value::Null,
        "the echo wire synthesises nothing"
    );
    assert_eq!(hello["audio_in"]["encoding"], "pcm_s16le");
    assert_eq!(hello["audio_in"]["sample_rate"], 16000);
    assert_eq!(hello["audio_in"]["channels"], 1);
    assert_eq!(
        hello["audio_out"], hello["audio_in"],
        "a loopback hands back exactly what it was given (spec § 3)"
    );

    let mut round_trips = Vec::with_capacity(CLICKS);
    for i in 0..CLICKS {
        // A different payload per click, so an answer from the previous frame
        // cannot pass for this one's.
        let payload: Vec<u8> = (0..FRAME_BYTES).map(|b| (b ^ i) as u8).collect();
        let sent = Instant::now();
        client.send_audio(&payload).await.expect("send audio");
        let (at, frame) = client
            .next_frame_at(MARKER)
            .await
            .unwrap_or_else(|e| panic!("click {i} was never answered: {e}"));
        assert_eq!(
            frame.as_audio(),
            Some(payload.as_slice()),
            "click {i} came back changed — the wire is not neutral"
        );
        round_trips.push(at.duration_since(sent).as_secs_f64() * 1000.0);
    }

    assert_eq!(round_trips.len(), CLICKS, "every frame was answered once");
    println!(
        "echo roundtrip, n={CLICKS} frames of {FRAME_BYTES} B: median {:.2} ms, \
         p90 {:.2} ms, max {:.2} ms, min {:.2} ms",
        percentile(&round_trips, 50.0),
        percentile(&round_trips, 90.0),
        percentile(&round_trips, 100.0),
        percentile(&round_trips, 0.1),
    );
}

/// The client's real-time pacing is the other half of a measurement that means
/// anything: audio pushed as fast as the socket takes it measures the socket.
///
/// 300 ms of audio in 20 ms chunks must take about 300 ms and produce fifteen
/// answers — and the echo wire hands every one of them back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_in_real_time_keeps_the_clock_of_the_audio() {
    let live = start(Arc::new(EchoStt::new()), None).await;
    let (mut client, _) =
        VoiceClient::connect(&format!("{}/ws?session=echo-realtime", live.ws_base))
            .await
            .expect("connect");

    let pcm = vec![0u8; 16_000 * 2 * 300 / 1000];
    let started = Instant::now();
    let last = client
        .stream_pcm_realtime(&pcm, 16_000, 20)
        .await
        .expect("stream 300 ms of audio");
    let elapsed = last.duration_since(started);
    assert!(
        elapsed >= Duration::from_millis(260),
        "real time means the stream does not race ahead of its own audio: {elapsed:?}"
    );

    // Collect up to the fifteenth answer rather than for half a second: the
    // count is the claim, so it must not depend on how busy the machine is.
    let answered = AtomicUsize::new(0);
    let frames = client
        .collect_until(
            |f| f.as_audio().is_some() && answered.fetch_add(1, Ordering::SeqCst) + 1 == 15,
            MARKER,
        )
        .await;
    let audio: Vec<&[u8]> = frames.iter().filter_map(|(_, f)| f.as_audio()).collect();
    assert_eq!(
        audio.len(),
        15,
        "300 ms in 20 ms chunks is 15 frames, and the wire loses none"
    );
    assert!(
        audio.iter().all(|c| c.len() == FRAME_BYTES),
        "and hands each one back whole"
    );
    let tail = client.drain_for(Duration::from_millis(200)).await;
    assert!(
        tail.iter().all(|(_, f)| f.as_audio().is_none()),
        "and invents no sixteenth: {tail:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// the paced listener
// ──────────────────────────────────────────────────────────────────────────

/// A listener that counts what it got and then waits for the test to take the
/// receipt — it is exactly as slow as the test is, and not one clock tick
/// slower.
///
/// This is the instrument `backpressure_loses_nothing` needs, and the marked
/// egress door is not: that door hands the message over with a `try_send` and
/// drops it when the channel is full, which is exactly the loss the test is
/// supposed to rule out. A cell has a mailbox instead, and a full mailbox
/// blocks the router — which is the mechanism under test.
///
/// **Why the receipt channel and not a `sleep` (GH #600).** The first version
/// slept a few milliseconds per message, so the flood took as long as 500
/// sequential timer waits — a number the host's load owns, not the substrate.
/// Under a loaded runner that turned a correct pipeline into a red test
/// (147 of 500 in the 60 s the test allowed itself). Now the pace comes from
/// the test's own `recv`: the listener blocks on a one-deep channel, so
/// "delayed" means "not yet pulled" instead of "the runner was busy", and the
/// only clock left is a generous failure marker per step.
struct PacedCounter {
    seen: Arc<AtomicUsize>,
    handled_tx: mpsc::Sender<()>,
}

impl Cell for PacedCounter {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let seen = self.seen.clone();
        let handled_tx = self.handled_tx.clone();
        // Only the lane under test is counted. Anything else the cell emits
        // later — a turn, an error, the idle notice a quiet provider draws
        // after 30 s — must not be able to fill the hole a lost interim left;
        // an off-lane message that counted made the flood look complete when
        // one interim was missing (measured while verifying GH #600).
        let counts = hop_route(&msg) == Some(PARTIAL_LANE);
        async move {
            if !counts {
                return;
            }
            seen.fetch_add(1, Ordering::SeqCst);
            // The receipt goes out AFTER the count, so a test that has taken
            // n receipts knows `seen >= n`. A closed channel (the fixture is
            // gone, i.e. shutdown) is not an error here.
            let _ = handled_tx.send(()).await;
        }
    }
}

/// Spawns [`PacedCounter`]s that share one counter and one receipt channel.
struct PacedCounterFactory {
    seen: Arc<AtomicUsize>,
    handled_tx: mpsc::Sender<()>,
}

impl PacedCounterFactory {
    fn spawn_once(
        &self,
        path: Path,
        outputs_tx: mpsc::Sender<CellEmission>,
        mailbox_capacity: usize,
        consumes: Option<Arc<meclaw_core::CompiledConsumes>>,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
    ) -> (
        mpsc::Sender<Message>,
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
        let cell = PacedCounter {
            seen: self.seen.clone(),
            handled_tx: self.handled_tx.clone(),
        };
        let join = tokio::spawn(async move {
            let _peace_keep = peace_tx;
            cell_task(
                path,
                rx,
                outputs_tx,
                cell,
                None,
                consumes,
                Some(colony_inbox_tx),
            )
            .await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for PacedCounterFactory {
    fn validate_params(&self, _raw: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn type_name(&self) -> &'static str {
        "paced_counter"
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _raw: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, join, peace_rx, backstop_rx) = self.spawn_once(
            path.clone(),
            outputs_tx.clone(),
            mailbox_capacity,
            contract.consumes.clone(),
            colony_inbox_tx.clone(),
        );
        let factory = self.clone();
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            factory.spawn_once(
                path.clone(),
                outputs_tx.clone(),
                mailbox_capacity,
                respawn_consumes.clone(),
                colony_inbox_tx.clone(),
            )
        });
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// harness
// ──────────────────────────────────────────────────────────────────────────

/// A booted colony holding exactly one `voice` cell, plus the door its
/// emissions leave through.
struct Fixture {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    egress: mpsc::Receiver<Message>,
    /// `ws://<listener>/<mount>`.
    ws_base: String,
    /// The listener the fixture's mounts are reached on.
    listener: tokio::task::JoinHandle<()>,
    /// How many messages the paced listener has finished handling.
    seen: Arc<AtomicUsize>,
    /// One receipt per message the paced listener has handled. The listener
    /// blocks on this channel, so the test — and nothing else — decides how
    /// fast it drains.
    handled: Option<mpsc::Receiver<()>>,
}

impl Fixture {
    /// Boot a colony whose only cell is a `voice` cell with these `params`.
    ///
    /// The root hive routes `./voice -> .` and stamps [`MARK`] on the way, so
    /// every emission of the cell dies at the root and lands on `egress`.
    async fn boot(params: Value) -> Self {
        Self::boot_with(params, true).await
    }

    /// The same colony, but the cell's contract may declare
    /// `consumes.context.session_id` as optional.
    ///
    /// Required is what the shipped template declares and what every real
    /// colony wants. Optional exists for exactly one test: with `required`,
    /// the substrate refuses a message that lacks the context at the delivery
    /// boundary (`ConsumesViolation`), so the cell's own `missing_session`
    /// refusal is never reached — see `speak_missing_session_is_error_lane`.
    async fn boot_with(params: Value, session_required: bool) -> Self {
        Self::boot_full(params, session_required, false).await
    }

    /// The colony, optionally with a counting listener on the cell's out-edge
    /// whose pace the test holds ([`PacedCounter`]). Its mailbox is deliberately
    /// tiny ([`LISTENER_MAILBOX`]), so a listener that does not drain really
    /// does fill it and really does block the router — the mechanism
    /// `backpressure_loses_nothing` is about.
    async fn boot_full(params: Value, session_required: bool, paced_listener: bool) -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path().join("main");
        std::fs::create_dir_all(root.join("voice")).expect("create the cell dir");
        let mut set_context = meclaw_core::serde_json::Map::new();
        set_context.insert(MARK.to_string(), json!("'1'"));
        let mut edges = vec![json!({
            "from": "./voice",
            "to": ".",
            "modifier": {"set_context": set_context}
        })];
        let seen = Arc::new(AtomicUsize::new(0));
        // One deep: the listener counts, hands over the receipt and is stuck
        // until the test takes it.
        let (handled_tx, handled_rx) = mpsc::channel::<()>(1);
        if paced_listener {
            std::fs::create_dir_all(root.join("listener")).expect("create the listener dir");
            std::fs::write(
                root.join("listener/config.json"),
                meclaw_core::serde_json::to_string_pretty(&json!({
                    "cell": {"type": "paced_counter", "mailbox_size": LISTENER_MAILBOX},
                    "params": {},
                    "contract": {
                        "version": "1.0.0",
                        "settings": {},
                        "consumes": {"body": {"messages": {"type": "array", "required": true}}}
                    }
                }))
                .expect("serialise the listener"),
            )
            .expect("write the listener");
            edges.push(json!({"from": "./voice", "to": "./listener"}));
        }
        std::fs::write(
            root.join("config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "hive"},
                "params": {"graph": {"edges": edges}}
            }))
            .expect("serialise the hive"),
        )
        .expect("write the root hive");

        let mount = params
            .get("mount")
            .and_then(Value::as_str)
            .expect("the fixture params must name a mount")
            .to_string();
        std::fs::write(
            root.join("voice/config.json"),
            meclaw_core::serde_json::to_string_pretty(&json!({
                "cell": {"type": "voice", "timeout": -1},
                "params": params,
                "contract": {
                    "version": "1.0.0",
                    "settings": {},
                    // The cell mints `session_id` itself — a connection is
                    // where a session begins — so it is the setter the header
                    // check looks for. Without this the boot refuses the node
                    // for requiring a context nothing upstream can promote.
                    "ingress": {"context": ["session_id"]},
                    "emits": {"body": {"messages": {"type": "array", "required": true}}},
                    "consumes": {
                        "body": {"messages": {"type": "array", "required": true}},
                        "context": {
                            "session_id": {"type": "string", "required": session_required}
                        }
                    }
                }
            }))
            .expect("serialise the cell"),
        )
        .expect("write the voice cell");

        // One table for the cell and for the listener in front of it: the
        // colony spawns the cell, the cell registers its mount here, and the
        // helper's listener is what a client reaches it through.
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let voice: Arc<dyn CellFactory> = Arc::new(VoiceCellFactory::new(Arc::clone(&surfaces)));
        let mut factories: Vec<(String, Arc<dyn CellFactory>)> =
            vec![("voice".to_string(), Arc::clone(&voice))];
        let mut registry = CellFactoryRegistry::new();
        registry.insert("voice".into(), voice);
        if paced_listener {
            let listener: Arc<dyn CellFactory> = Arc::new(PacedCounterFactory {
                seen: seen.clone(),
                handled_tx,
            });
            factories.push(("paced_counter".to_string(), listener.clone()));
            registry.insert("paced_counter".into(), listener);
        }
        let (h, egress) = ColonyHandle::new_with_marked_egress_at(&td, factories, MARK);
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .expect("the colony boots with a voice cell");

        let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
        Self {
            _td: td,
            h,
            egress,
            ws_base: format!("ws://{addr}/{mount}"),
            listener,
            seen,
            handled: paced_listener.then_some(handled_rx),
        }
    }

    /// The registry entry of the `voice` node — the receipt that says whether
    /// it was ever restarted or failed.
    async fn voice_entry(&self) -> RegistryEntryDto {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
        self.h
            .inbox_tx
            .send(ColonyMsg::ReadRegistry {
                path: None,
                path_prefix: None,
                cell_type: None,
                active: None,
                limit: 1000,
                ack: ack_tx,
            })
            .await
            .expect("registry read sent");
        ack_rx
            .await
            .expect("registry read acked")
            .entries
            .into_iter()
            .find(|e| e.path == "/voice")
            .expect("the voice node is registered")
    }

    fn ws_url(&self, query: &str) -> String {
        if query.is_empty() {
            format!("{}/ws", self.ws_base)
        } else {
            format!("{}/ws?{query}", self.ws_base)
        }
    }

    /// Connect, retrying while the cell is still registering.
    ///
    /// The I/O half puts its mount on the table when the cell's task starts, so
    /// the first attempt can lose that race and read the helper's `404` — the
    /// absence of an answer, not an answer. A lasting failure still reports at
    /// the deadline with its real error.
    async fn connect(&self, query: &str) -> (VoiceClient, Value) {
        let url = self.ws_url(query);
        let deadline = Instant::now() + DEADLINE;
        loop {
            match VoiceClient::connect(&url).await {
                Ok(pair) => return pair,
                Err(e) if Instant::now() >= deadline => {
                    panic!("the voice cell never accepted a connection on {url}: {e}")
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    /// Every emission that reached the door within `window`.
    async fn emissions_for(&mut self, window: Duration) -> Vec<Message> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return out;
            }
            match tokio::time::timeout(left, self.egress.recv()).await {
                Ok(Some(m)) => out.push(m),
                Ok(None) | Err(_) => return out,
            }
        }
    }

    /// Wait until `n` emissions carry `hop.route == route`, then return them
    /// together with everything else that arrived.
    async fn wait_for_route(&mut self, route: &str, n: usize) -> Vec<Message> {
        let deadline = Instant::now() + DEADLINE;
        let mut out = Vec::new();
        loop {
            if out.iter().filter(|m| hop_route(m) == Some(route)).count() >= n {
                return out;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!(
                    "only {} of {n} `{route}` emission(s) arrived within {DEADLINE:?}; \
                     saw routes {:?}",
                    out.iter().filter(|m| hop_route(m) == Some(route)).count(),
                    out.iter().map(hop_route_owned).collect::<Vec<_>>()
                );
            }
            match tokio::time::timeout(left, self.egress.recv()).await {
                Ok(Some(m)) => out.push(m),
                Ok(None) => panic!("the egress channel closed while waiting for `{route}`"),
                Err(_) => {}
            }
        }
    }

    /// Hand the cell an assistant turn to speak, the way a topology would.
    async fn speak(&self, session_id: &str, text: &str) {
        let mut context = meclaw_core::serde_json::Map::new();
        context.insert("session_id".into(), json!(session_id));
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".into(), json!("in_speak"));
        self.h
            .send(
                MessageBuilder::new(Path::new("/voice"))
                    .context(context)
                    .hop(hop)
                    .body(Body::Inline(json!({
                        "messages": [{"origin": "assistant", "type": "text", "text": text}]
                    })))
                    .build(),
            )
            .await;
    }

    async fn shutdown(self) {
        self.listener.abort();
        self.h.shutdown().await;
    }
}

/// Wait until the client has seen a `partial` whose text contains `needle`.
///
/// Every provider message produces a partial of its own, so "a partial arrived"
/// would close a boundary somewhere in the middle of a script and make a test a
/// race. Waiting for the one that carries the whole take is what makes the next
/// step of these tests a fact.
async fn wait_for_partial(client: &mut VoiceClient, needle: &str) {
    let seen = client
        .collect_until(
            |f| {
                f.as_text()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
                    .map(|t| t.contains(needle))
                    .unwrap_or(false)
            },
            DEADLINE,
        )
        .await;
    assert!(
        seen.iter().any(|(_, f)| f.is_type("partial")),
        "the script must have played before the boundary closes, got {seen:?}"
    );
}

fn hop_route(m: &Message) -> Option<&str> {
    m.headers.hop.get("route").and_then(Value::as_str)
}

fn hop_route_owned(m: &Message) -> Option<String> {
    hop_route(m).map(str::to_string)
}

fn hop_str<'a>(m: &'a Message, key: &str) -> Option<&'a str> {
    m.headers.hop.get(key).and_then(Value::as_str)
}

/// The `text` of the first turn in an emission's body.
fn body_text(m: &Message) -> String {
    let Body::Inline(v) = &m.body else {
        return String::new();
    };
    v.get("messages")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Params for a cell whose STT is the echo provider and that has no TTS.
fn echo_params(mount: &str) -> Value {
    json!({
        "mount": mount,
        "stt": {"provider": "echo"}
    })
}

// ──────────────────────────────────────────────────────────────────────────
// 1. the wire itself
// ──────────────────────────────────────────────────────────────────────────

/// R-V6': an odd binary frame is reported, twice over, and the call goes on.
///
/// The earlier ruling closed the connection with `4400`. It was the wrong
/// trade: a client whose capture graph emits one odd chunk — a resampler with
/// a rounding bug, a worklet cut short at the end of a buffer — loses the
/// whole call for a fault that costs half a sample. Now the frame is refused
/// and counted, the caller keeps talking, and both the client and the topology
/// learn how often it happened, so the bug is visible without being fatal.
///
/// Never silent, though: a silently dropped odd byte desynchronises every
/// later frame and comes out looking like a model failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_audio_frame_is_reported_and_the_connection_stays() {
    let stt = fakes::deepgram(fakes::flux_quiet()).await;
    let tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &stt, &tts)).await;
    let (mut client, _) = fx.connect("session=odd").await;

    for expected in 1..=2u64 {
        client
            .send_audio(&vec![0u8; AUDIO_GATE + 1])
            .await
            .expect("send an odd frame");
        let err = client
            .next_frame_of_type("error", DEADLINE)
            .await
            .expect("an odd frame is answered, not swallowed");
        assert_eq!(err["code"], "bad_audio_frame");
        assert_eq!(
            err["bad_frames"], expected,
            "the count is what makes a recurring fault visible: {err}"
        );
    }

    let emissions = fx.wait_for_route("error", 2).await;
    let errors: Vec<&Message> = emissions
        .iter()
        .filter(|m| hop_route(m) == Some("error"))
        .collect();
    assert_eq!(errors.len(), 2, "one lane emission per refused frame");
    for (i, err) in errors.iter().enumerate() {
        assert_eq!(hop_str(err, "error_code"), Some("bad_audio_frame"));
        assert_eq!(
            hop_str(err, "session_id"),
            Some("odd"),
            "and it names the connection it happened on — an error lane without \
             an address is a report nobody can act on"
        );
        assert_eq!(
            err.headers.hop.get("bad_frames").and_then(Value::as_u64),
            Some(i as u64 + 1),
            "the topology sees the same count the client does"
        );
    }

    // The connection is still a connection: a whole frame after the bad ones
    // reaches the provider. Without this the test would pass on a cell that
    // stopped closing but also stopped listening.
    let before = stt.received_audio_bytes();
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("send a whole frame");
    let deadline = Instant::now() + DEADLINE;
    while stt.received_audio_bytes() < before + AUDIO_GATE && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        stt.received_audio_bytes(),
        before + AUDIO_GATE,
        "the audio after a refused frame must still reach the provider"
    );

    let tail = client.drain_for(Duration::from_millis(300)).await;
    assert!(
        tail.iter().all(|(_, f)| f.as_close().is_none()),
        "and nothing closed the call over it: {tail:?}"
    );

    fx.shutdown().await;
}

/// A second connection claiming a live session displaces the first with `4409`.
/// The addressable identity is the session, not the socket — otherwise a
/// reconnecting client would find two halves of itself answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_replaced_4409() {
    let stt = fakes::deepgram(fakes::flux_quiet()).await;
    let tts = fakes::cartesia(fakes::cartesia_chunks(4, 20)).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &stt, &tts)).await;
    let (mut first, hello_a) = fx.connect("session=twin").await;
    assert_eq!(hello_a["session_id"], "twin");

    let (mut second, hello_b) = fx.connect("session=twin").await;
    assert_eq!(hello_b["session_id"], "twin");

    let frames = first
        .collect_until(|f| f.as_close().is_some(), DEADLINE)
        .await;
    assert_eq!(
        frames.iter().find_map(|(_, f)| f.as_close()),
        Some(4409),
        "the first connection must be told it was replaced, with the documented code"
    );

    // Displacement is a handover, not a hang-up: the address now belongs to
    // the newcomer. If the cell had merely closed the old socket and kept its
    // registration, this speak would come back as an `unknown_session` error —
    // a reconnecting client would find itself unreachable under its own name.
    fx.speak("twin", "Are you still there?").await;
    let start = second
        .next_frame_of_type("speak_start", DEADLINE)
        .await
        .expect("the surviving connection is the one the session addresses");
    assert!(start["speak_id"].is_string());
    assert!(
        fx.emissions_for(Duration::from_millis(300))
            .await
            .iter()
            .all(|m| hop_route(m) != Some("error")),
        "and nothing was reported as unroutable"
    );

    fx.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────
// 2. turn boundaries — the load-bearing claims
// ──────────────────────────────────────────────────────────────────────────

/// Exactly one `turn` emission per `EndOfTurn`, and the interim transcripts
/// stay on the `partial` lane. This is the invariant of § 4 and the one defect
/// class the assistant would feel immediately: a duplicated turn is a
/// duplicated answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_turn_per_end_of_turn() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::update(script, "guten");
    let script = fakes::update(script, "guten tag");
    let script = fakes::update(script, "guten tag zusammen");
    let fake = fakes::deepgram(fakes::end_of_turn(script, "guten tag zusammen")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &fake, &quiet_tts)).await;
    let (mut client, hello) = fx.connect("session=turn-1").await;
    assert_eq!(hello["stt"], "deepgram");

    client
        .send_audio(&vec![0u8; 640])
        .await
        .expect("audio starts the session");

    let seen = fx.wait_for_route("turn", 1).await;
    let turns: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .collect();
    let partials: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("partial"))
        .collect();
    assert_eq!(turns.len(), 1, "one EndOfTurn is one turn, never two");
    assert_eq!(body_text(turns[0]), "guten tag zusammen");
    assert_eq!(partials.len(), 3, "and the three interims stayed interim");

    // Give a duplicate every chance to show up before declaring there is none.
    let more = fx.emissions_for(Duration::from_millis(500)).await;
    assert!(
        !more.iter().any(|m| hop_route(m) == Some("turn")),
        "a second turn arrived after the first: {:?}",
        more.iter().map(hop_route_owned).collect::<Vec<_>>()
    );

    // Counted by waiting for the frame that ends the turn, not by holding a
    // stopwatch open: a positive count read out of a time window is a count
    // that goes wrong on a loaded machine, in the direction of passing.
    let client_frames = client.collect_until(|f| f.is_type("turn"), DEADLINE).await;
    let client_turns = client_frames
        .iter()
        .filter(|(_, f)| f.is_type("turn"))
        .count();
    let client_partials = client_frames
        .iter()
        .filter(|(_, f)| f.is_type("partial"))
        .count();
    assert_eq!(
        (client_partials, client_turns),
        (3, 1),
        "the client sees the same shape as the lanes — it is a mirror, not a second truth"
    );
    // The window is only ever used for the negative half.
    let mirror_tail = client.drain_for(Duration::from_millis(200)).await;
    assert!(
        !mirror_tail.iter().any(|(_, f)| f.is_type("turn")),
        "and no second turn frame followed it: {mirror_tail:?}"
    );

    fx.shutdown().await;
}

/// The two lanes never wear each other's clothes: an emission on `turn` has a
/// `turn_id` and no `eager`, one on `partial` has `eager` and no `turn_id`.
/// A downstream cell keys on exactly this.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_partial_on_turn_lane() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::update(script, "halb");
    let script = fakes::eager(script, "halb fertig");
    let fake = fakes::deepgram(fakes::end_of_turn(script, "halb fertig")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &fake, &quiet_tts)).await;
    let (mut client, _) = fx.connect("session=lanes").await;
    client.send_audio(&vec![0u8; 640]).await.expect("audio");

    let seen = fx.wait_for_route("turn", 1).await;
    for m in seen.iter().filter(|m| hop_route(m) == Some("turn")) {
        assert!(
            m.headers.hop.get("turn_id").is_some(),
            "a turn is identified: {:?}",
            m.headers.hop
        );
        assert!(
            m.headers.hop.get("eager").is_none(),
            "`eager` is a property of an interim, never of a turn: {:?}",
            m.headers.hop
        );
    }
    let partials: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("partial"))
        .collect();
    assert!(!partials.is_empty(), "the script sent interims");
    for m in partials {
        assert!(
            m.headers.hop.get("eager").is_some(),
            "an interim declares whether it was a preflight: {:?}",
            m.headers.hop
        );
        assert!(
            m.headers.hop.get("turn_id").is_none(),
            "an interim is not a turn and must not carry its id: {:?}",
            m.headers.hop
        );
    }

    fx.shutdown().await;
}

/// In `hold` the client draws the boundary. Two provider end-of-turns and a
/// dangling interim collapse into exactly one turn on `release` — the whole
/// point of push-to-talk is that the provider's endpointing does not decide.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hold_release_boundary() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::end_of_turn(script, "a");
    let script = fakes::end_of_turn(script, "b");
    let fake = fakes::deepgram(fakes::update(script, "c")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    // This test is about the BUFFER, not about the drain: `0` says "cut on the
    // release frame", so the turn it asserts on is the one the frame produced
    // and not one a cap produced a second and a half later.
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    params["release_grace_ms"] = json!(0);
    let mut fx = Fixture::boot(params).await;
    let (mut client, hello) = fx.connect("session=hold-1&mode=hold").await;
    assert_eq!(hello["mode"], "hold");

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio");
    // Wait for the interim that carries the WHOLE script — every provider
    // end-of-turn produces a partial of its own, so "a partial arrived" would
    // close the boundary somewhere in the middle and make this test a race.
    let seen = client
        .collect_until(
            |f| {
                f.as_text()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
                    .map(|t| t.contains('c'))
                    .unwrap_or(false)
            },
            DEADLINE,
        )
        .await;
    assert!(
        seen.iter().any(|(_, f)| f.is_type("partial")),
        "the script must have played before the boundary closes, got {seen:?}"
    );
    client.release().await.expect("close the boundary");

    let seen = fx.wait_for_route("turn", 1).await;
    let turns: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .collect();
    assert_eq!(turns.len(), 1, "a hold ends in exactly one turn");
    assert_eq!(
        body_text(turns[0]),
        "a b c",
        "the buffer keeps every fragment the provider closed, plus the open interim"
    );

    fx.shutdown().await;
}

/// The defect found on the built-in test page (06.09.2026), end to end:
/// the last real line of a take was missing.
///
/// `release` used to cut the turn where the frame arrived, and Deepgram Flux
/// reports the end of a turn 400-700 ms after the words that caused it — so the
/// final transcript was always a little too late and was thrown away. Now the
/// boundary drains and the provider's `EndOfTurn` closes it, with everything in
/// it.
///
/// **No sleep decides this test.** The fake holds its final transcript behind a
/// second audio gate, and the test opens that gate itself, after the `release`.
/// Audio after a release is exactly what a real client does: it keeps the
/// recognition session warm, and in `hold` mode it belongs to no turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_final_after_release_lands_in_the_same_turn() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::update(script, "a");
    let script = fakes::update(script, "a b");
    // Held until the audio the test sends AFTER the release arrives.
    let script = fakes::gate(script, AUDIO_GATE * 2);
    let fake = fakes::deepgram(fakes::end_of_turn(script, "a b c")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &fake, &quiet_tts)).await;
    let (mut client, _) = fx.connect("session=grace-1&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio");
    wait_for_partial(&mut client, "a b").await;

    client.release().await.expect("let the key go");
    // The provider's answer to the audio it already had.
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio keeps the session warm after a release");

    let seen = fx.wait_for_route("turn", 1).await;
    let turns: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .collect();
    assert_eq!(turns.len(), 1, "a boundary ends in exactly one turn");
    assert_eq!(
        body_text(turns[0]),
        "a b c",
        "the words that were still in flight when the key went up belong to \
         this turn; `a b` would be the cap having cut too early"
    );

    fx.shutdown().await;
}

/// The other half of the same defect: the previous take turned up at the front
/// of the next one.
///
/// A late `EndOfTurn` used to arrive with no boundary open and, if the speaker
/// was quick, with the NEXT one open — so it was buffered there and came out as
/// a prefix. Two takes, and the second carries nothing of the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_hold_starts_empty_after_the_previous_take() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::update(script, "erster take");
    let script = fakes::gate(script, AUDIO_GATE * 2);
    let script = fakes::end_of_turn(script, "first take whole");
    let script = fakes::gate(script, AUDIO_GATE * 3);
    let script = fakes::update(script, "zweiter take");
    let script = fakes::gate(script, AUDIO_GATE * 4);
    let fake = fakes::deepgram(fakes::end_of_turn(script, "second take whole")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &fake, &quiet_tts)).await;
    let (mut client, _) = fx.connect("session=grace-2&mode=hold").await;

    for (open_partial, expected) in [
        ("erster take", "first take whole"),
        ("zweiter take", "second take whole"),
    ] {
        client.hold().await.expect("open the boundary");
        client
            .send_audio(&vec![0u8; AUDIO_GATE])
            .await
            .expect("audio");
        wait_for_partial(&mut client, open_partial).await;
        client.release().await.expect("let the key go");
        client
            .send_audio(&vec![0u8; AUDIO_GATE])
            .await
            .expect("audio after the release");

        let seen = fx.wait_for_route("turn", 1).await;
        let turns: Vec<&Message> = seen
            .iter()
            .filter(|m| hop_route(m) == Some("turn"))
            .collect();
        assert_eq!(turns.len(), 1, "one take, one turn");
        assert_eq!(
            body_text(turns[0]),
            expected,
            "each take carries its own words and only its own"
        );
    }

    fx.shutdown().await;
}

/// A provider that goes quiet must not hold a turn open for the rest of the
/// call. With `release_grace_ms: 200` and no `EndOfTurn` ever coming, the cap
/// closes the boundary with the last interim.
///
/// The clock is the point of this test rather than an accident of it, so it is
/// asserted from below only: `tokio` never wakes a sleep early, and how late a
/// loaded runner is has nothing to prove.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_release_grace_cap_ends_a_turn_the_provider_never_ends() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let fake = fakes::deepgram(fakes::update(script, "a b")).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    params["release_grace_ms"] = json!(200);
    let mut fx = Fixture::boot(params).await;
    let (mut client, _) = fx.connect("session=grace-cap&mode=hold").await;

    client.hold().await.expect("open the boundary");
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio");
    wait_for_partial(&mut client, "a b").await;

    let released = Instant::now();
    client.release().await.expect("let the key go");
    let seen = fx.wait_for_route("turn", 1).await;
    let waited = released.elapsed();

    let turns: Vec<&Message> = seen
        .iter()
        .filter(|m| hop_route(m) == Some("turn"))
        .collect();
    assert_eq!(turns.len(), 1, "the cap ends the boundary once");
    assert_eq!(
        body_text(turns[0]),
        "a b",
        "the cap cuts with the interim that was never ended"
    );
    assert!(
        waited >= Duration::from_millis(200),
        "the turn must not be cut before the grace it was armed with: {waited:?}"
    );

    fx.shutdown().await;
}

/// A release with nothing in the buffer is not an empty turn on the lane. The
/// client still gets its `turn` frame — it asked, it gets an answer — but no
/// assistant is woken for silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_without_speech_emits_nothing() {
    let fake = fakes::deepgram(fakes::flux_quiet()).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    // Nothing was said, so there is nothing for a provider to finish: `0` cuts
    // on the frame and this test does not pay for a grace it is not about.
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    params["release_grace_ms"] = json!(0);
    let mut fx = Fixture::boot(params).await;
    let (mut client, _) = fx.connect("session=hold-empty&mode=hold").await;

    client.hold().await.expect("open");
    client.release().await.expect("close on silence");

    let turn = client
        .next_frame_of_type("turn", DEADLINE)
        .await
        .expect("the client is answered even when there was nothing to say");
    assert_eq!(turn["text"], "");

    let emissions = fx.emissions_for(Duration::from_millis(500)).await;
    assert!(
        !emissions.iter().any(|m| hop_route(m) == Some("turn")),
        "silence must not reach the topology as a turn: {:?}",
        emissions.iter().map(hop_route_owned).collect::<Vec<_>>()
    );

    fx.shutdown().await;
}

/// A flood of interims from a provider that does not pause, against a listener
/// that moves only when this test lets it. Every one of them arrives: the
/// listener's mailbox fills, the router blocks on it, the emitter stalls behind
/// it. Dropping would be the easy fix and the wrong one — a lost interim is a
/// transcript with a hole in it.
///
/// **No clock owns this proof (GH #600).** The test itself is the slow
/// consumer: it holds the listener still while the flood runs, checks that the
/// flood really was held (a handful handled, not five hundred), and then pulls
/// the interims one by one. Every wait is a [`MARKER`] per step, never a budget
/// for the whole run — a loaded host makes the pull slower, it does not make
/// the pipeline lose anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backpressure_loses_nothing() {
    /// Bracketed from both sides, and the brackets are counts, not clocks.
    ///
    /// **Above** the listener's mailbox ([`LISTENER_MAILBOX`] = 4) many times
    /// over — the flood has to fill it and keep the router waiting, or there is
    /// no backpressure to lose anything through.
    ///
    /// **Below** the wire's own give-up, which is where GH #600's 500 went
    /// wrong. `VoiceIoShared::send_to` hands a client frame over with a
    /// `try_send` into a `DISPATCH_QUEUE`-deep (64) channel and, when that is
    /// full, calls the client gone and drops the session — deliberately, and
    /// without a clock ("two full buffers is not backpressure any more, it is a
    /// connection nobody is on"). The same interims go down the wire, and the
    /// emitter runs a full colony inbox ahead of the listener, so a flood
    /// larger than that queue is a race against the client task getting
    /// scheduled: won on an idle host, lost on a loaded one, and lost for good
    /// — the session is gone and the rest of the flood is never sent. Fifty
    /// fits in the queue even if the client task never runs at all, and still
    /// overruns the listener twelve times over.
    const INTERIMS: usize = 50;
    let mut script = fakes::flux_after_audio(AUDIO_GATE);
    for i in 0..INTERIMS {
        script = fakes::update(script, &format!("word {i}"));
    }
    let fake = fakes::deepgram(script).await;
    let quiet_tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut params = fakes::both_params(MOUNT, &fake, &quiet_tts);
    // The wire's own operation timeout, raised to this file's failure-marker
    // convention. At its 5 s default it is a semantic discriminator about
    // *clients* — a socket that takes nothing for five seconds has a dead
    // reader, and the I/O half drops the session (`deliver` in `voice/io.rs`).
    // Under a loaded runner the reader here is not dead, only descheduled, and
    // the timeout fired anyway: that, not slowness, is what made GH #600 red
    // (147 of 500, and the rest never sent). This test is about the topology
    // lane, so the wire gets a marker, not a stopwatch.
    params["external_timeout_ms"] = json!(30_000);
    let mut fx = Fixture::boot_full(params, true, true).await;
    let before = fx.voice_entry().await;
    let mut handled = fx.handled.take().expect("the paced listener is wired");

    let (mut client, _) = fx.connect("session=flood").await;
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio");
    // The client reads its socket for the whole flood, and that is not a
    // convenience: the same interims go down the wire, and a client that stops
    // reading is a *different* experiment — after `external_timeout_ms` the
    // I/O half calls that socket dead, reports the commands it still holds and
    // drops the session (`deliver` in `voice/io.rs`), which ends the flood.
    // GH #600's red run was exactly that: the un-drained socket stalled under
    // host load, the session went with it, and the flood stopped at 147 of 500
    // — a hole no amount of waiting fills. The experiment here is the topology
    // lane, so the wire must not be the thing that gives way.
    let reader = tokio::spawn(async move { while client.next_frame(MARKER).await.is_ok() {} });

    // 1. The stall. Until this test takes a receipt, the listener is parked in
    //    `handle()`, and everything behind it is bounded and blocking.
    tokio::time::timeout(MARKER, handled.recv())
        .await
        .expect("the first interim reaches the listener")
        .expect("the listener stays alive");
    // Semantic discriminator, and a tight one on purpose: the receipt channel
    // is one deep, so between the count and the block at most the buffered one,
    // the one handing over and the one just counted can have passed — never a
    // quarter of the flood. A pipeline that answered `handle()` and dropped the
    // rest would show the whole flood counted here, or a hole at the end.
    let while_stalled = fx.seen.load(Ordering::SeqCst);
    assert!(
        while_stalled <= 8,
        "the flood must wait for the listener, not race past it: {while_stalled} of \
         {INTERIMS} were handled while the test held the listener still"
    );

    // 2. The drain. One pull per interim, each with its own generous marker —
    //    a slow host stretches the pulls, it cannot invent a loss.
    for i in 1..INTERIMS {
        tokio::time::timeout(MARKER, handled.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "interim {i} of {INTERIMS} never reached the listener; backpressure \
                     delays, it does not drop (handled so far: {})",
                    fx.seen.load(Ordering::SeqCst)
                )
            })
            .expect("the listener stays alive for the whole flood");
    }
    assert_eq!(
        fx.seen.load(Ordering::SeqCst),
        INTERIMS,
        "every interim must reach the listener, and none twice"
    );
    reader.abort();

    let after = fx.voice_entry().await;
    assert_eq!(
        (
            before.cell_id.as_str(),
            before.lifecycle_status.as_str(),
            before.active,
            before.failed
        ),
        (
            after.cell_id.as_str(),
            after.lifecycle_status.as_str(),
            after.active,
            after.failed
        ),
        "and the cell rode it out — a respawn or a failure here would mean the \
         stall was mistaken for a wedge"
    );

    fx.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────
// 3. speaking, and stopping
// ──────────────────────────────────────────────────────────────────────────

/// `cancel` in the middle of a synthesis stops it on both sides: the client
/// gets `speak_end cancelled`, and the provider is told, so the wave of audio
/// already commissioned is not paid for twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_mid_speak() {
    let stt = fakes::deepgram(fakes::flux_quiet()).await;
    let tts = fakes::cartesia(fakes::cartesia_chunks(20, 50)).await;
    let mut fx = Fixture::boot(fakes::both_params(MOUNT, &stt, &tts)).await;
    let (mut client, hello) = fx.connect("session=speak-1").await;
    assert_eq!(hello["tts"], "cartesia");

    let before = fx.emissions_for(Duration::from_millis(100)).await.len();
    fx.speak(
        "speak-1",
        "A long sentence nobody wants to hear to the end.",
    )
    .await;

    let mut audio = 0usize;
    let deadline = Instant::now() + DEADLINE;
    while audio < 3 && Instant::now() < deadline {
        let frame = client
            .next_frame(DEADLINE)
            .await
            .expect("the synthesis must start");
        if frame.as_audio().is_some() {
            audio += 1;
        }
    }
    assert_eq!(audio, 3, "three chunks arrived before the cancel");

    client.cancel().await.expect("cancel");
    let end = client
        .next_frame_of_type("speak_end", DEADLINE)
        .await
        .expect("a cancelled synthesis still ends, and says why");
    assert_eq!(end["reason"], "cancelled");

    // The client's `speak_end` and the provider's cancel are two different
    // events on two different sockets; the second lags the first, so this
    // waits for it instead of reading once and calling the gap a defect.
    assert!(
        fakes::wait_for_cancel(&tts).await,
        "the provider must learn about the cancel — an abandoned socket keeps generating"
    );
    // A cancel that only stops the *reporting* leaves the buffered chunks
    // coming, which a listener hears as the assistant talking over them.
    let tail = client.drain_for(Duration::from_millis(300)).await;
    assert_eq!(
        tail.iter().filter(|(_, f)| f.as_audio().is_some()).count(),
        0,
        "no audio may follow the cancel: {tail:?}"
    );
    assert_eq!(
        tail.iter().filter(|(_, f)| f.is_type("speak_end")).count(),
        0,
        "and the verdict is given once, not once per buffered chunk: {tail:?}"
    );

    let after = fx.emissions_for(Duration::from_millis(300)).await.len();
    assert_eq!(
        after, 0,
        "speaking and cancelling produce no lane traffic at all (before: {before})"
    );

    fx.shutdown().await;
}

/// Barge-in: the caller starts talking while the assistant is talking, and the
/// assistant stops. The trigger is the provider's `SpeechStarted`, not a
/// client frame, because in `auto` mode the client has no button to press.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn barge_in_cancels_speak() {
    // The provider only reports speech once the caller's audio really arrived,
    // so the barge-in below is triggered by the test, not by a race.
    let stt = fakes::deepgram(fakes::speech_started(fakes::flux_after_audio(AUDIO_GATE))).await;
    let tts = fakes::cartesia(fakes::cartesia_chunks(20, 50)).await;
    let fx = Fixture::boot(fakes::both_params(MOUNT, &stt, &tts)).await;
    let (mut client, _) = fx.connect("session=barge-1").await;

    fx.speak("barge-1", "I am now telling something at great length.")
        .await;
    let _ = client
        .next_frame_of_type("speak_start", DEADLINE)
        .await
        .expect("the synthesis starts");

    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("the caller starts talking");

    let end = client
        .next_frame_of_type("speak_end", DEADLINE)
        .await
        .expect("speech from the caller must end the synthesis");
    assert_eq!(
        end["reason"], "cancelled",
        "barge-in cancels; it does not let the sentence finish"
    );
    assert!(
        fakes::wait_for_cancel(&tts).await,
        "and the provider is told, the same as on an explicit cancel"
    );

    fx.shutdown().await;
}

/// **The `speak_end` lane, ordered and not ordered.**
///
/// One emission per synthesis the cell accepted, on the cell's own path, with
/// the identity of the synthesis and the reason it ended — the receipt a
/// telephony hive waits for before it kills a leg. Both halves stand here,
/// because the "off" half is a SILENCE and a silence proves nothing without
/// its control: the same round with `emit_speak_end: true` has to produce the
/// emission, or the negative below would pass on a cell that emits nothing at
/// all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speak_end_is_a_lane_whoever_waits_orders() {
    // ── ordered: one `speak_end`, reason `done`, and it names the synthesis.
    let stt = fakes::deepgram(fakes::flux_quiet()).await;
    let tts = fakes::cartesia(fakes::cartesia_chunks(3, 5)).await;
    let mut params = fakes::both_params_default(MOUNT, &stt, &tts);
    params["emit_speak_end"] = json!(true);
    let mut fx = Fixture::boot(params).await;
    let (mut client, _) = fx.connect("session=end-1").await;

    fx.speak("end-1", "Ein kurzer Satz.").await;
    let end = client
        .next_frame_of_type("speak_end", DEADLINE)
        .await
        .expect("the synthesis ends on the socket too");
    assert_eq!(end["reason"], "done");

    let got = fx.wait_for_route("speak_end", 1).await;
    let ends: Vec<&Message> = got
        .iter()
        .filter(|m| hop_route(m) == Some("speak_end"))
        .collect();
    assert_eq!(
        ends.len(),
        1,
        "one synthesis, one receipt: {:?}",
        got.iter().map(hop_route_owned).collect::<Vec<_>>()
    );
    let e = ends[0];
    assert_eq!(hop_str(e, "session_id"), Some("end-1"));
    assert_eq!(hop_str(e, "reason"), Some("done"));
    assert_eq!(hop_str(e, "platform"), Some("voice"));
    assert_eq!(
        hop_str(e, "speak_id"),
        end["speak_id"].as_str(),
        "the lane and the client's own frame name the SAME synthesis"
    );
    assert_eq!(
        body_text(e),
        "",
        "the receipt carries no words — whoever waited for the sentence had it"
    );
    fx.shutdown().await;

    // ── not ordered: the shipped default emits nothing on the lane at all.
    let stt = fakes::deepgram(fakes::flux_quiet()).await;
    let tts = fakes::cartesia(fakes::cartesia_chunks(3, 5)).await;
    let mut fx = Fixture::boot(fakes::both_params_default(MOUNT, &stt, &tts)).await;
    let (mut client, _) = fx.connect("session=end-2").await;
    fx.speak("end-2", "Ein kurzer Satz.").await;
    let end = client
        .next_frame_of_type("speak_end", DEADLINE)
        .await
        .expect("the client is told either way — its frame is a different path");
    assert_eq!(end["reason"], "done");
    let quiet = fx.emissions_for(Duration::from_millis(300)).await;
    assert!(
        quiet.iter().all(|m| hop_route(m) != Some("speak_end")),
        "R-V8': a lane nobody ordered emits nothing: {:?}",
        quiet.iter().map(hop_route_owned).collect::<Vec<_>>()
    );
    fx.shutdown().await;
}

/// A speak addressed at a session that is not connected is an error on the
/// lane, not a swallowed message. The sender is a cell, and a cell that is
/// never told its answer went nowhere cannot retry or complain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speak_unknown_session_is_error_lane() {
    let mut fx = Fixture::boot(echo_params(MOUNT)).await;
    let (_client, _) = fx.connect("session=real").await;

    fx.speak("nope", "Who is hearing this?").await;

    let seen = fx.wait_for_route("error", 1).await;
    let err = seen
        .iter()
        .find(|m| hop_route(m) == Some("error"))
        .expect("an unroutable speak is reported");
    assert_eq!(hop_str(err, "error_code"), Some("unknown_session"));
    assert_eq!(
        hop_str(err, "session_id"),
        Some("nope"),
        "and it names the session that was asked for, not the one that exists — \
         an error lane without an address is a report nobody can act on"
    );

    fx.shutdown().await;
}

/// And a speak with no `context.session_id` at all is a different error with a
/// different name, because the two are different bugs upstream: a missing
/// context is a wiring fault, an unknown session is a race with a hang-up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speak_missing_session_is_error_lane() {
    // Optional on purpose. With the shipped `required: true` the substrate
    // refuses this message at the delivery boundary (`ConsumesViolation`) and
    // the cell never sees it — so the refusal below is unreachable in any
    // colony that declares the context required, which the shipped template
    // does. The other half of that fact is pinned right after.
    let mut fx = Fixture::boot_with(echo_params(MOUNT), false).await;

    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_speak"));
    fx.h.send(
        MessageBuilder::new(Path::new("/voice"))
            .hop(hop)
            .body(Body::Inline(json!({
                "messages": [{"origin": "assistant", "type": "text", "text": "An niemanden."}]
            })))
            .build(),
    )
    .await;

    let seen = fx.wait_for_route("error", 1).await;
    let err = seen
        .iter()
        .find(|m| hop_route(m) == Some("error"))
        .expect("a speak without an address is reported");
    assert_eq!(hop_str(err, "error_code"), Some("missing_session"));
    assert!(
        fx.h.drain_dead_letters().await.is_empty(),
        "the cell answered it, so nothing dead-lettered"
    );

    fx.shutdown().await;
}

/// R-V8': with no listener ordered, the interims stay on the socket.
///
/// The client's mirror is not the lane. A colony that emits `partial` without
/// anyone consuming it dead-letters every single interim, visibly, at the
/// channel container — which is noise the operator has to read past for the
/// rest of the call. So the lane is opt-in, and the frames are not: the person
/// watching the transcript still sees it move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partials_stay_on_the_socket_without_a_listener() {
    let script = fakes::flux_after_audio(AUDIO_GATE);
    let script = fakes::update(script, "eins");
    let script = fakes::update(script, "eins zwei");
    let stt = fakes::deepgram(fakes::update(script, "eins zwei drei")).await;
    let tts = fakes::cartesia(fakes::cartesia_silent()).await;
    let mut fx = Fixture::boot(fakes::both_params_default(MOUNT, &stt, &tts)).await;
    let (mut client, _) = fx.connect("session=quiet-lane").await;
    client
        .send_audio(&vec![0u8; AUDIO_GATE])
        .await
        .expect("audio");

    let seen = client
        .collect_until(
            |f| {
                f.as_text()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
                    .map(|t| t.contains("drei"))
                    .unwrap_or(false)
            },
            DEADLINE,
        )
        .await;
    assert_eq!(
        seen.iter().filter(|(_, f)| f.is_type("partial")).count(),
        3,
        "the client still sees every interim: {seen:?}"
    );

    let emissions = fx.emissions_for(Duration::from_millis(500)).await;
    assert!(
        !emissions.iter().any(|m| hop_route(m) == Some("partial")),
        "and nobody ordered the lane, so nothing was put on it: {:?}",
        emissions.iter().map(hop_route_owned).collect::<Vec<_>>()
    );
    assert!(
        fx.h.drain_dead_letters().await.is_empty(),
        "and nothing dead-lettered — which is the whole point of the default"
    );

    fx.shutdown().await;
}

/// The other half of the sibling test: with the contract the shipped template
/// declares (`session_id` required), a speak without one never reaches the
/// cell at all — the delivery boundary refuses it and it dead-letters.
///
/// Both facts belong in this file because together they say something the
/// error-code list alone does not: `missing_session` is the cell's answer for
/// a colony that made the context optional, not the answer a normal colony
/// gets. A reader of the closed code list would otherwise expect the emission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_session_context_is_refused_before_the_cell() {
    let mut fx = Fixture::boot(echo_params(MOUNT)).await;

    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_speak"));
    fx.h.send(
        MessageBuilder::new(Path::new("/voice"))
            .hop(hop)
            .body(Body::Inline(json!({
                "messages": [{"origin": "assistant", "type": "text", "text": "An niemanden."}]
            })))
            .build(),
    )
    .await;

    let deadline = Instant::now() + DEADLINE;
    let dlq = loop {
        let dlq = fx.h.drain_dead_letters().await;
        if !dlq.is_empty() || Instant::now() >= deadline {
            break dlq;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(dlq.len(), 1, "the substrate refuses it, once");
    assert!(
        matches!(
            dlq[0].reason,
            meclaw_colony::DeadLetterReason::ConsumesViolation
        ),
        "and for the declared reason, got {:?}",
        dlq[0].reason
    );
    assert!(
        fx.emissions_for(Duration::from_millis(300))
            .await
            .is_empty(),
        "and the cell, never having seen it, says nothing"
    );

    fx.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────
// the fakes
// ──────────────────────────────────────────────────────────────────────────

/// Thin names over the scripted provider fakes of t2a and t3a.
///
/// They live in one place so that a change to either fake's surface is one edit
/// here rather than one per test — these tests are about the cell, and the
/// shape of somebody else's builder is not what they should be spelling out
/// eleven times.
mod fakes {
    use super::*;
    use meclaw_testing::mock_cartesia::{CartesiaScript, MockCartesia};
    use meclaw_testing::mock_deepgram::{DeepgramScript, MockDeepgram};

    /// A Flux script that only starts talking once the client's audio really
    /// arrived. Everything in this file wants that gate: a transcript that
    /// appears before the audio proves nothing about the adapter, and waiting
    /// for it with a sleep is how a suite gets flaky.
    pub fn flux_after_audio(bytes: usize) -> DeepgramScript {
        DeepgramScript::new().require_audio_bytes(bytes)
    }

    /// A Flux script that greets and then says nothing at all — for the tests
    /// where the STT half only has to exist.
    pub fn flux_quiet() -> DeepgramScript {
        DeepgramScript::new()
    }

    /// An interim transcript.
    pub fn update(script: DeepgramScript, text: &str) -> DeepgramScript {
        script.turn_info("Update", text)
    }

    /// A preflight transcript — Flux's `EagerEndOfTurn`, the one that carries
    /// `eager: true` down the partial lane.
    pub fn eager(script: DeepgramScript, text: &str) -> DeepgramScript {
        script.turn_info("EagerEndOfTurn", text)
    }

    /// The provider's turn boundary.
    pub fn end_of_turn(script: DeepgramScript, text: &str) -> DeepgramScript {
        script.turn_info("EndOfTurn", text)
    }

    /// Hold the rest of the script back until `bytes` of audio have arrived in
    /// total. Mid-script this is a **rendezvous**: the test decides when the
    /// next provider message happens by sending the audio that releases it, so
    /// "the provider answered after the client let the key go" is a fact rather
    /// than a sleep somebody tuned.
    pub fn gate(script: DeepgramScript, bytes: usize) -> DeepgramScript {
        script.require_audio_bytes(bytes)
    }

    /// Flux's `StartOfTurn` — what barge-in keys on.
    pub fn speech_started(script: DeepgramScript) -> DeepgramScript {
        script.turn_info("StartOfTurn", "")
    }

    pub async fn deepgram(script: DeepgramScript) -> MockDeepgram {
        MockDeepgram::start(script)
            .await
            .expect("the fake deepgram binds")
    }

    /// A synthesis long enough to be cancelled in the middle of it: `n` chunks
    /// of 24 kHz PCM16, each `ms` apart, so a `cancel` after the third one has
    /// something left to cut off.
    pub fn cartesia_chunks(n: usize, ms: u64) -> CartesiaScript {
        let chunk = vec![0u8; 24_000 * 2 * ms as usize / 1000];
        CartesiaScript::new()
            .with_chunks(vec![chunk; n])
            .with_chunk_delay(std::time::Duration::from_millis(ms))
    }

    pub async fn cartesia(script: CartesiaScript) -> MockCartesia {
        MockCartesia::start(script)
            .await
            .expect("the fake cartesia binds")
    }

    /// Did the provider learn about the cancel? An abandoned socket keeps
    /// generating, so "the client stopped hearing it" is not the same fact —
    /// and it is a fact that arrives a little later, on the provider's socket.
    pub async fn wait_for_cancel(fake: &MockCartesia) -> bool {
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            if fake.cancelled().await {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// A synthesis provider that is configured and never asked to speak. The
    /// params contract refuses a cell that transcribes but cannot speak, so
    /// even a test about turns has to name one.
    pub fn cartesia_silent() -> CartesiaScript {
        CartesiaScript::new()
    }

    fn stt_params(fake: &MockDeepgram) -> Value {
        json!({
            "provider": "deepgram",
            "api_key": "fake-key",
            "language": "de",
            "sample_rate": 16000,
            "base_url": fake.base_url()
        })
    }

    /// The shipped defaults, with one thing said out loud: `emit_partials`.
    ///
    /// R-V8' makes it `false` by default — an interim that nobody listens for
    /// dead-letters visibly at the channel container, once per interim. So a
    /// colony that wants the lane orders it, and a test that asserts on the
    /// lane orders it here rather than leaning on a default that moved.
    pub fn both_params(mount: &str, stt: &MockDeepgram, tts: &MockCartesia) -> Value {
        let mut params = both_params_default(mount, stt, tts);
        params["emit_partials"] = json!(true);
        params
    }

    /// The same, with `emit_partials` left at whatever the default is.
    pub fn both_params_default(mount: &str, stt: &MockDeepgram, tts: &MockCartesia) -> Value {
        json!({
            "mount": mount,
            "stt": stt_params(stt),
            "tts": {
                "provider": "cartesia",
                "api_key": "fake-key",
                "voice": "fake-voice",
                "language": "de",
                "sample_rate": 24000,
                "base_url": tts.base_url()
            }
        })
    }
}
