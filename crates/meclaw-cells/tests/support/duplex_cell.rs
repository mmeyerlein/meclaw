//! One `voice` cell in duplex mode, both halves running, with a model a test
//! writes the script of.
//!
//! Welle Live, strand L2b. Eight locks need the same three things — a live
//! socket, a provider whose every `Append` and `Mute` the test can read, and
//! the handler's own emissions — so they are built ONCE here instead of eight
//! times (the form of `tests/support/mod.rs`).
//!
//! No colony: the claims of this strand are about the connection and the
//! handler, and a topology in between would only add a router to what is
//! already a pair of channels. The handler loop below is the one the substrate
//! runs (`cell_task_long_running`), written out — events on one side, the
//! mailbox on the other, one cell owning both.

#![allow(dead_code)]

use meclaw_cells::voice::cell::{VoiceCell, VoiceReconfig};
use meclaw_cells::voice::contract::{
    AudioFormat, BoxFuture, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts,
};
use meclaw_cells::voice::io::VoiceIo;
use meclaw_cells::voice::params::VoiceParams;
use meclaw_cells::voice::providers::build_duplex;
use meclaw_cells::voice::providers::echo::EchoStt;
use meclaw_colony::io_liveness::IoLivenessMark;
use meclaw_colony::{DbConn, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, Headers, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::surface_listener;
use meclaw_testing::voice_client::VoiceClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The repo's failure-marker window: generous, never a discriminator.
pub const MARKER: Duration = Duration::from_secs(30);

/// The mount every cell in this strand registers.
pub const MOUNT: &str = "voice";

/// 16 kHz mono PCM16: 640 bytes is 20 ms, the frame this wire recommends.
pub const RATE: u32 = 16_000;
/// Bytes in one 20 ms frame at [`RATE`].
pub const FRAME_BYTES: usize = 640;

/// What the test holds of one running fake session: the two channels it can
/// play the model's part on.
pub struct FakeSession {
    /// Everything the model says about itself.
    pub events: mpsc::Sender<DuplexEvent>,
    /// Audio the model produced.
    pub audio_out: mpsc::Sender<Vec<u8>>,
    /// Which session of the provider this is, counting from 1 (GH #896).
    pub index: usize,
    /// Whether the connection opened it as a renewal of a running call
    /// (`run_renewed_session`, GH #896).
    pub renewed: bool,
    /// Where the fake renews: lets the session finish once its caller's audio
    /// has ended ([`Self::finish`]). `None` elsewhere.
    finish: Option<oneshot::Sender<()>>,
    /// Where the fake renews: drops the session's socket ([`Self::hang_up`]).
    /// `None` elsewhere.
    hang_up: Option<oneshot::Sender<()>>,
    /// Where the fake renews: ends the provider's task at once, while this
    /// handle keeps its event channel open ([`Self::stop`]). `None` elsewhere.
    stop: Option<oneshot::Sender<oneshot::Sender<()>>>,
}

impl FakeSession {
    /// Push one event and fail loudly if the connection is gone.
    pub async fn say(&self, event: DuplexEvent) {
        self.events
            .send(event)
            .await
            .expect("the connection is still reading the model's events");
    }

    /// Let this session of a renewing fake finish (GH #896).
    ///
    /// A session whose caller's audio has ended holds its verdict until this
    /// is called or the handle is dropped. That hold is what makes the
    /// retiring session measurable: while it holds, the connection is still
    /// reading what the old model says, and a test can play its last words.
    pub fn finish(&mut self) {
        if let Some(tx) = self.finish.take() {
            let _ = tx.send(());
        }
    }

    /// The socket of this session of a renewing fake drops (GH #896).
    ///
    /// Played the way the real adapter reports it (OR-L.L2a.3): a warning, a
    /// close with `connection_lost`, and an ordinary verdict. The handle goes
    /// with it, because the connection learns that a provider is gone from its
    /// event channel closing, and this handle holds a sender of it.
    pub fn hang_up(mut self) {
        if let Some(tx) = self.hang_up.take() {
            let _ = tx.send(());
        }
    }

    /// The provider task of this session of a renewing fake ends at once and
    /// without a word, and this handle stays (GH #896, review I-3).
    ///
    /// Its way in and its control channel are gone when this returns; its
    /// event channel is NOT, because this handle holds a sender of it. That is
    /// the moment between a provider ending and the connection reading the
    /// end of its events, held open for as long as the test likes: the
    /// connection can only tell by the senders it holds.
    pub async fn stop(&mut self) {
        let Some(tx) = self.stop.take() else {
            panic!("stop() is a knob of the renewing fake")
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        tx.send(ack_tx)
            .expect("the session's provider task is still running");
        ack_rx
            .await
            .expect("the provider task acknowledges before it returns");
    }
}

/// One thing a fake session was told, in the order it was told (GH #896).
///
/// The control frames and the audio frames arrive on two channels, and two
/// channels have no order between them. The journal is the one place where
/// "the handover reached the new session before the caller did" is a
/// measurement rather than a guess.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// A control frame.
    Control(DuplexControl),
    /// An audio frame.
    Audio(Vec<u8>),
}

/// A duplex provider that does nothing on its own.
///
/// It is the counterpart of `voice_t5_behaviour.rs`'s inline providers: the
/// test decides what the model says and when, and everything the CELL told the
/// model comes back out on a channel. Two knobs beyond that: a session counter,
/// which is how "there is no reconnect" (OR-L20) is measured, and a verdict,
/// which is how a provider that gives up is played.
pub struct FakeDuplex {
    controls: mpsc::Sender<DuplexControl>,
    sessions: mpsc::Sender<FakeSession>,
    audio: mpsc::Sender<Vec<u8>>,
    let_go: mpsc::Sender<Duration>,
    starts: Arc<AtomicUsize>,
    fail: bool,
    /// A model that does NOT finalise when the caller's audio stops: it holds
    /// its verdict for ever, which is what makes the connection's own wait
    /// measurable instead of academic.
    linger: bool,
    rate: u32,
    /// Every control frame, with the index of the session it reached (GH #896).
    tagged_controls: mpsc::UnboundedSender<(usize, DuplexControl)>,
    /// Every audio frame, with the index of the session it reached.
    tagged_audio: mpsc::UnboundedSender<(usize, Vec<u8>)>,
    /// Both, in the order each session was told them.
    journal: mpsc::UnboundedSender<(usize, Seen)>,
    /// The index of every session whose caller's audio ended.
    ended: mpsc::UnboundedSender<usize>,
    /// How many sessions were opened as renewals.
    renewed_starts: Arc<AtomicUsize>,
    /// The fixture of GH #896. A closed control channel is what it is on the
    /// real adapter — the colony stopped guiding, the audio is the session and
    /// still flows, so every frame already queued is still read — and a
    /// session holds its verdict until [`FakeSession::finish`], and can drop
    /// its socket with [`FakeSession::hang_up`]. Off everywhere else, where
    /// the fake behaves exactly as it always has.
    renewing: bool,
}

impl DuplexProvider for FakeDuplex {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.rate)
    }

    fn rates(&self) -> Vec<u32> {
        vec![8_000, 16_000, 24_000]
    }

    fn run_session(
        &self,
        _format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        self.run(session, liveness, false)
    }

    /// The same session, counted as a renewal (GH #896).
    fn run_renewed_session(
        &self,
        _format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        self.run(session, liveness, true)
    }
}

impl FakeDuplex {
    /// One session, whether the connection opened it as the first of a call
    /// or as a renewal of one.
    fn run(
        &self,
        session: DuplexSession,
        liveness: IoLivenessMark,
        renewed: bool,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let DuplexSession {
            mut audio_in,
            audio_out,
            events,
            mut control,
        } = session;
        let controls = self.controls.clone();
        let sessions = self.sessions.clone();
        let audio = self.audio.clone();
        let let_go = self.let_go.clone();
        let starts = Arc::clone(&self.starts);
        let fail = self.fail;
        let linger = self.linger;
        let tagged_controls = self.tagged_controls.clone();
        let tagged_audio = self.tagged_audio.clone();
        let journal = self.journal.clone();
        let ended = self.ended.clone();
        let renewed_starts = Arc::clone(&self.renewed_starts);
        let renewing = self.renewing;
        Box::pin(async move {
            let index = starts.fetch_add(1, Ordering::SeqCst) + 1;
            if renewed {
                renewed_starts.fetch_add(1, Ordering::SeqCst);
            }
            let _ = events
                .send(DuplexEvent::Started {
                    session_id: "fake-session".to_string(),
                })
                .await;
            if fail {
                // A disturbance, not the ordinary end: the return value is the
                // verdict, exactly as with `SttProvider` (contract § 1.1).
                //
                // The handle is deliberately NOT published here. It holds a
                // clone of the event sender, and the connection learns that a
                // provider is gone from that channel closing — a handle left
                // lying in a test's channel would keep the session looking
                // alive for ever. Measured: without this the connection never
                // left its loop and `duplex_failed` never reached the lane.
                return Err(DuplexError::Closed("the model hung up".to_string()));
            }
            let (finish_tx, finish_rx) = oneshot::channel::<()>();
            let (hang_up_tx, mut hang_up_rx) = oneshot::channel::<()>();
            let (stop_tx, mut stop_rx) = oneshot::channel::<oneshot::Sender<()>>();
            let (finish_tx, hang_up_tx, stop_tx) = if renewing {
                (Some(finish_tx), Some(hang_up_tx), Some(stop_tx))
            } else {
                (None, None, None)
            };
            let _ = sessions
                .send(FakeSession {
                    events: events.clone(),
                    audio_out: audio_out.clone(),
                    index,
                    renewed,
                    finish: finish_tx,
                    hang_up: hang_up_tx,
                    stop: stop_tx,
                })
                .await;
            // Always open where the fake does not renew, so the loop below is
            // the loop it always was.
            let mut control_open = true;
            let mut hang_up_armed = renewing;
            let mut stop_armed = renewing;
            loop {
                tokio::select! {
                    biased;

                    cmd = control.recv(), if control_open => match cmd {
                        None if renewing => control_open = false,
                        None => break,
                        Some(DuplexControl::Close) => {
                            let _ = tagged_controls.send((index, DuplexControl::Close));
                            let _ = journal.send((index, Seen::Control(DuplexControl::Close)));
                            let _ = controls.send(DuplexControl::Close).await;
                            break;
                        }
                        Some(cmd) => {
                            let _ = tagged_controls.send((index, cmd.clone()));
                            let _ = journal.send((index, Seen::Control(cmd.clone())));
                            let _ = controls.send(cmd).await;
                        }
                    },

                    frame = audio_in.recv() => match frame {
                        None => break,
                        Some(bytes) => {
                            liveness.mark_success();
                            let _ = tagged_audio.send((index, bytes.clone()));
                            let _ = journal.send((index, Seen::Audio(bytes.clone())));
                            let _ = audio.send(bytes).await;
                        }
                    },

                    signal = &mut hang_up_rx, if hang_up_armed => match signal {
                        // The socket dropped, and the real adapter says so the
                        // way this does: a warning, a close, an ordinary
                        // verdict (OR-L.L2a.3).
                        Ok(()) => {
                            let _ = events
                                .send(DuplexEvent::Warning {
                                    detail: "socket ended mid-session".to_string(),
                                })
                                .await;
                            let _ = events
                                .send(DuplexEvent::Closed {
                                    reason: "connection_lost".to_string(),
                                    usage_seconds: 0.0,
                                })
                                .await;
                            return Ok(());
                        }
                        // The handle was dropped without a word: nothing
                        // happened to the socket.
                        Err(_) => hang_up_armed = false,
                    },

                    signal = &mut stop_rx, if stop_armed => match signal {
                        // Gone: the way in and the control channel drop here,
                        // the event channel stays with the handle.
                        Ok(ack) => {
                            drop(audio_in);
                            drop(control);
                            let _ = ack.send(());
                            return Ok(());
                        }
                        Err(_) => stop_armed = false,
                    },
                }
            }
            let _ = ended.send(index);
            if renewing {
                // Held until the test lets go, or drops the handle: while it
                // holds, the connection still reads what this model says.
                let _ = finish_rx.await;
            }
            if linger {
                // The caller's audio has stopped and this model does not
                // finalise. The connection is now inside its close grace; when
                // that runs out it drops the run, and dropping the run closes
                // the control channel — so `control.recv()` returning `None` is
                // the moment the connection let go, timed from here. The event
                // sender stays held on purpose: a closed event channel would end
                // the wait for a reason other than the clock.
                let waited = Instant::now();
                while control.recv().await.is_some() {}
                let _ = let_go.send(waited.elapsed()).await;
                return Ok(());
            }
            let _ = events
                .send(DuplexEvent::Closed {
                    reason: "close_requested".to_string(),
                    usage_seconds: 0.0,
                })
                .await;
            Ok(())
        })
    }
}

/// One `voice` cell with both halves running, and the four seams a test drives
/// it by.
pub struct Live {
    /// `ws://<listener>/<mount>` — the prefix the socket route hangs off.
    pub ws_base: String,
    /// Every control frame the model was told, in order.
    pub controls: mpsc::Receiver<DuplexControl>,
    /// One handle per session the cell opened.
    pub sessions: mpsc::Receiver<FakeSession>,
    /// Every audio frame the model was fed.
    pub audio: mpsc::Receiver<Vec<u8>>,
    /// How long a lingering model waited before the connection let go of it.
    pub let_go: mpsc::Receiver<Duration>,
    /// Everything the handler emitted — lanes and refusals alike.
    pub emissions: mpsc::Receiver<CellEmission>,
    /// The cell's mailbox: `in_speak`, `in_advise`, a params update.
    pub mailbox: mpsc::Sender<Message>,
    /// How many sessions the provider was asked to run.
    pub starts: Arc<AtomicUsize>,
    /// Every control frame the model was told, with the index of the session
    /// it reached (GH #896).
    pub tagged_controls: mpsc::UnboundedReceiver<(usize, DuplexControl)>,
    /// Every audio frame the model was fed, with the index of the session it
    /// reached.
    pub tagged_audio: mpsc::UnboundedReceiver<(usize, Vec<u8>)>,
    /// Both, in the order each session was told them — see [`Seen`].
    pub journal: mpsc::UnboundedReceiver<(usize, Seen)>,
    /// The index of every session whose caller's audio ended.
    pub ended: mpsc::UnboundedReceiver<usize>,
    /// How many of [`Self::starts`] were renewals.
    pub renewed_starts: Arc<AtomicUsize>,
    /// Every message the cell handed to the colony as a dead letter (GH #1016).
    pub dead_letters: mpsc::Receiver<meclaw_colony::ColonyMsg>,
    handler: tokio::task::JoinHandle<()>,
    io: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    /// Held: the I/O half reads this channel closing as "the handler is gone".
    _reconfig: mpsc::Sender<VoiceReconfig>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.handler.abort();
        self.io.abort();
        self.listener.abort();
    }
}

/// The parts of the fake a caller keeps: the provider and the channels behind
/// it.
struct Wired {
    provider: Arc<dyn DuplexProvider>,
    controls: mpsc::Receiver<DuplexControl>,
    sessions: mpsc::Receiver<FakeSession>,
    audio: mpsc::Receiver<Vec<u8>>,
    let_go: mpsc::Receiver<Duration>,
    starts: Arc<AtomicUsize>,
    tagged_controls: mpsc::UnboundedReceiver<(usize, DuplexControl)>,
    tagged_audio: mpsc::UnboundedReceiver<(usize, Vec<u8>)>,
    journal: mpsc::UnboundedReceiver<(usize, Seen)>,
    ended: mpsc::UnboundedReceiver<usize>,
    renewed_starts: Arc<AtomicUsize>,
}

fn wire_fake(fail: bool, linger: bool, rate: u32) -> Wired {
    wire_fake_narrow(fail, linger, rate, 256)
}

/// The same, with the fake's own audio queue held to `audio_cap`.
///
/// A short queue is how a test PARKS the connection task: the fake blocks on a
/// full queue, the connection's `audio_tx` fills behind it, and the connection
/// suspends inside the arm that feeds the model. Nothing else in this fixture
/// can hold that task still, and holding it still is the only way to have two
/// select arms become ready while nobody is polling them.
fn wire_fake_narrow(fail: bool, linger: bool, rate: u32, audio_cap: usize) -> Wired {
    wire_fake_full(fail, linger, rate, audio_cap, false, 8)
}

/// The fake with every knob, the GH #896 ones included: `renewing` (see
/// [`FakeDuplex`]) and room for `sessions_cap` session handles nobody has
/// picked up yet — a call that renews a few times publishes a handle per
/// session, and a full queue would park the provider before its loop.
fn wire_fake_full(
    fail: bool,
    linger: bool,
    rate: u32,
    audio_cap: usize,
    renewing: bool,
    sessions_cap: usize,
) -> Wired {
    let (controls_tx, controls) = mpsc::channel(256);
    let (sessions_tx, sessions) = mpsc::channel(sessions_cap);
    let (audio_tx, audio) = mpsc::channel(audio_cap);
    let (let_go_tx, let_go) = mpsc::channel(8);
    let (tagged_controls_tx, tagged_controls) = mpsc::unbounded_channel();
    let (tagged_audio_tx, tagged_audio) = mpsc::unbounded_channel();
    let (journal_tx, journal) = mpsc::unbounded_channel();
    let (ended_tx, ended) = mpsc::unbounded_channel();
    let starts = Arc::new(AtomicUsize::new(0));
    let renewed_starts = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(FakeDuplex {
        controls: controls_tx,
        sessions: sessions_tx,
        audio: audio_tx,
        let_go: let_go_tx,
        starts: Arc::clone(&starts),
        fail,
        linger,
        rate,
        tagged_controls: tagged_controls_tx,
        tagged_audio: tagged_audio_tx,
        journal: journal_tx,
        ended: ended_tx,
        renewed_starts: Arc::clone(&renewed_starts),
        renewing,
    });
    Wired {
        provider,
        controls,
        sessions,
        audio,
        let_go,
        starts,
        tagged_controls,
        tagged_audio,
        journal,
        ended,
        renewed_starts,
    }
}

/// The params document of a duplex cell whose provider object the test
/// supplies.
///
/// `provider: echo` is what a params document may say without a credential —
/// the parse is what makes the CELL a duplex cell (`params.duplex`), and the
/// object beside it is what actually runs, exactly as `717` does it with a
/// scripted recogniser behind `stt: {"provider": "echo"}`.
pub fn duplex_params(extra: Value) -> Value {
    let mut raw = json!({
        "mount": MOUNT,
        "emit_partials": true,
        "emit_speak_end": true,
        "duplex": {"provider": "echo", "sample_rate": RATE},
    });
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    raw
}

/// The params document of a CASCADE cell — one recogniser, no append channel.
pub fn cascade_params() -> Value {
    json!({
        "mount": MOUNT,
        "stt": {"provider": "echo"},
    })
}

/// The two deadlines the connection's `speak_end` heuristic runs on (OR-L19).
///
/// They reach the I/O half from `params.duplex` through the factory, which a
/// hand-built fixture does not run — so a test that measures the heuristic sets
/// them here. The shipped numbers are 1 500 ms and 8 000 ms; a lock about the
/// MECHANISM uses short ones, and says so where it does.
pub const QUIET_MS: u64 = 1_500;
/// The ceiling of the same heuristic.
pub const CAP_MS: u64 = 8_000;
/// How long the connection waits for the provider's verdict once the client is
/// gone — the third number the factory reads out of `params.duplex` and a
/// hand-built fixture therefore sets itself. The shipped one.
pub const CLOSE_GRACE_MS: u64 = 15_000;

/// The session clock the shipped template carries (`params.duplex.tick_ms`).
///
/// It reaches the I/O half from `params.duplex` through the factory, and the
/// fixture below does not run the factory — the duplex block of these fixtures
/// is `echo`, which has no knobs at all. So a lock about the CLOCK sets the
/// number itself, exactly as the three `speak_end` deadlines above do.
pub const TICK_MS: u64 = 1_000;

/// Boot a duplex cell around a fake model, on the shipped deadlines.
pub async fn boot_fake(raw: Value) -> Live {
    boot_fake_timed(raw, QUIET_MS, CAP_MS).await
}

/// Boot a duplex cell whose session clock ticks every `tick_ms`, with the
/// fake's audio queue held to `audio_cap`.
///
/// Two knobs, both of them about time: a finer tick makes a deadline
/// measurable inside a test's patience, and a short audio queue parks the
/// connection task (see [`wire_fake_narrow`]). `audio_cap` of 256 is the
/// ordinary one and parks nothing.
pub async fn boot_fake_clocked(raw: Value, tick_ms: u64, audio_cap: usize) -> Live {
    let wired = wire_fake_narrow(false, false, RATE, audio_cap);
    boot_with(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        tick_ms,
    )
    .await
}

/// The same, with the `speak_end` heuristic held to deadlines of the test's
/// own choosing.
pub async fn boot_fake_timed(raw: Value, quiet_ms: u64, cap_ms: u64) -> Live {
    let wired = wire_fake(false, false, RATE);
    boot(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        quiet_ms,
        cap_ms,
        CLOSE_GRACE_MS,
    )
    .await
}

/// Boot a duplex cell around a model that never finalises, with a close grace
/// of the test's own choosing.
///
/// The pair that makes the wait measurable: the model holds its verdict for
/// ever, so what ends the wait is the number and nothing else.
pub async fn boot_lingering(raw: Value, close_grace_ms: u64) -> Live {
    let wired = wire_fake(false, true, RATE);
    boot(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        close_grace_ms,
    )
    .await
}

/// A model that gives up as soon as the session is open.
pub async fn boot_failing(raw: Value) -> Live {
    let wired = wire_fake(true, false, RATE);
    boot(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
    )
    .await
}

/// Boot a duplex cell around the SHIPPED loopback (`providers/duplex_echo.rs`),
/// built from `params` exactly as the factory builds it.
///
/// The fake above is for scripts; this is for the wire — what comes out of the
/// socket is what a real adapter put in, with no test provider in between.
pub async fn boot_echo(raw: Value) -> Live {
    let params = VoiceParams::parse(&raw).expect("the fixture params parse");
    let duplex = params
        .duplex
        .as_ref()
        .map(|d| build_duplex(d, ProviderTimeouts::default()).expect("the loopback adapter"));
    boot(raw, duplex, None, QUIET_MS, CAP_MS, CLOSE_GRACE_MS).await
}

/// Boot a cascade cell: no duplex provider at all.
pub async fn boot_cascade(raw: Value) -> Live {
    boot(raw, None, None, QUIET_MS, CAP_MS, CLOSE_GRACE_MS).await
}

async fn boot(
    raw: Value,
    duplex: Option<Arc<dyn DuplexProvider>>,
    wired: Option<Wired>,
    quiet_ms: u64,
    cap_ms: u64,
    close_grace_ms: u64,
) -> Live {
    boot_with(
        raw,
        duplex,
        wired,
        quiet_ms,
        cap_ms,
        close_grace_ms,
        TICK_MS,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn boot_with(
    raw: Value,
    duplex: Option<Arc<dyn DuplexProvider>>,
    wired: Option<Wired>,
    quiet_ms: u64,
    cap_ms: u64,
    close_grace_ms: u64,
    tick_ms: u64,
) -> Live {
    boot_full(
        raw,
        duplex,
        wired,
        quiet_ms,
        cap_ms,
        close_grace_ms,
        tick_ms,
        meclaw_cells::voice::params::DEFAULT_RENEW_AFTER_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
    )
    .await
}

/// Boot a duplex cell around a fake that renews (GH #896): the connection
/// opens the call's next session `renew_after_ms` after a session took charge,
/// and a started renewal is due to take over at its handover or after
/// `renew_grace_ms` -- and does, at the first quiet moment of the line.
///
/// The two numbers reach the I/O half from `params.duplex` through the
/// factory, which this fixture does not run — so the test sets them here, as
/// it sets the tick.
pub async fn boot_renewing(raw: Value, renew_after_ms: u64, renew_grace_ms: u64) -> Live {
    boot_renewing_clocked(raw, renew_after_ms, renew_grace_ms, TICK_MS).await
}

/// The same, with the session clock ticking every `tick_ms`.
pub async fn boot_renewing_clocked(
    raw: Value,
    renew_after_ms: u64,
    renew_grace_ms: u64,
    tick_ms: u64,
) -> Live {
    let wired = wire_fake_full(false, false, RATE, 4096, true, 64);
    boot_full(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        tick_ms,
        renew_after_ms,
        renew_grace_ms,
    )
    .await
}

/// The same, with the quiet and the cap a takeover waits on (review I-2) held
/// to the test's own numbers -- the shipped ones are 1 500 and 8 000 ms.
pub async fn boot_renewing_timed(
    raw: Value,
    renew_after_ms: u64,
    renew_grace_ms: u64,
    quiet_ms: u64,
    cap_ms: u64,
    tick_ms: u64,
) -> Live {
    let wired = wire_fake_full(false, false, RATE, 4096, true, 64);
    boot_full(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        quiet_ms,
        cap_ms,
        CLOSE_GRACE_MS,
        tick_ms,
        renew_after_ms,
        renew_grace_ms,
    )
    .await
}

/// A renewing fake whose sessions give up as soon as they are open.
pub async fn boot_renewing_failing(raw: Value, renew_after_ms: u64, renew_grace_ms: u64) -> Live {
    let wired = wire_fake_full(true, false, RATE, 4096, true, 64);
    boot_full(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        renew_after_ms,
        renew_grace_ms,
    )
    .await
}

/// Boot a duplex cell around a fake model with the two edge knobs of GH #1055
/// set: how long a session opened while the line rings waits for its media
/// connection, and the duck. Both reach the I/O half from `params.duplex`
/// through the factory, which this fixture does not run.
#[allow(dead_code)]
pub async fn boot_edge(
    raw: Value,
    prewarm_ttl_ms: u64,
    duck: meclaw_cells::voice::duck::DuckParams,
) -> Live {
    let wired = wire_fake(false, false, RATE);
    boot_core(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_AFTER_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (prewarm_ttl_ms, duck),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn boot_full(
    raw: Value,
    duplex: Option<Arc<dyn DuplexProvider>>,
    wired: Option<Wired>,
    quiet_ms: u64,
    cap_ms: u64,
    close_grace_ms: u64,
    tick_ms: u64,
    renew_after_ms: u64,
    renew_grace_ms: u64,
) -> Live {
    boot_core(
        raw,
        duplex,
        wired,
        quiet_ms,
        cap_ms,
        close_grace_ms,
        tick_ms,
        renew_after_ms,
        renew_grace_ms,
        (0, meclaw_cells::voice::duck::DuckParams::default()),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn boot_core(
    raw: Value,
    duplex: Option<Arc<dyn DuplexProvider>>,
    wired: Option<Wired>,
    quiet_ms: u64,
    cap_ms: u64,
    close_grace_ms: u64,
    tick_ms: u64,
    renew_after_ms: u64,
    renew_grace_ms: u64,
    edge: (u64, meclaw_cells::voice::duck::DuckParams),
) -> Live {
    let params = VoiceParams::parse(&raw).expect("the fixture params parse");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = mpsc::channel(256);

    let mut io = VoiceIo::new(
        MOUNT.to_string(),
        // The inert placeholder a duplex cell carries (OR-L23); the real
        // recogniser of a cascade fixture.
        Arc::new(EchoStt::new()),
        None,
        params.default_mode,
        Duration::from_millis(params.external_timeout_ms),
        Duration::from_millis(params.provider_idle_timeout_ms),
        events_tx.clone(),
    );
    io.duplex = duplex;
    io.spoken_quiet_ms = quiet_ms;
    io.spoken_cap_ms = cap_ms;
    io.close_grace_ms = close_grace_ms;
    io.duplex_tick_ms = tick_ms;
    io.duplex_renew_after_ms = renew_after_ms;
    io.duplex_renew_grace_ms = renew_grace_ms;
    io.duplex_prewarm_ttl_ms = edge.0;
    io.duck = edge.1;
    io.cell_path = Path::new("/voice");
    io.surfaces = Arc::clone(&surfaces);

    // GH #1016: the colony inbox a cell dead-letters into, read by the test.
    let (dead_tx, dead_letters) = mpsc::channel::<meclaw_colony::ColonyMsg>(64);
    let mut cell =
        VoiceCell::new(Path::new("/voice"), io, &params, &raw).with_dead_letters(dead_tx);
    let io_half = cell.split_io();
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<VoiceReconfig>(64);
    let io_task = tokio::spawn(VoiceCell::run_io(io_half, events_tx, reconfig_rx));

    let (out_tx, emissions) = mpsc::channel::<CellEmission>(512);
    let (mailbox_tx, mut mailbox_rx) = mpsc::channel::<Message>(64);
    let origin = OriginSink::new(out_tx.clone(), Path::new("/voice"), 8);
    let handler_reconfig = reconfig_tx.clone();
    let handler = tokio::spawn(async move {
        let mut db = DbConn::wrap(rusqlite::Connection::open_in_memory().expect("open"), None);
        loop {
            tokio::select! {
                // Events first, and deliberately: `register` emits `Connected`
                // before the `hello` the client is waiting for, so a test that
                // has a socket has a session — and a mailbox message that
                // overtook it would be answered `unknown_session` for a call
                // that is perfectly live.
                biased;

                event = events_rx.recv() => {
                    let Some(event) = event else { break };
                    cell.handle_event(event, &origin, &mut db).await;
                }
                msg = mailbox_rx.recv() => {
                    let Some(msg) = msg else { break };
                    let sink = OutputSink::new(
                        out_tx.clone(),
                        Path::new("/voice"),
                        Uuid::now_v7(),
                        Uuid::now_v7(),
                        64,
                        Headers::new(),
                        None,
                    );
                    cell.handle(msg, &sink, &mut db, &handler_reconfig).await;
                }
            }
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

    let (
        controls,
        sessions,
        audio,
        let_go,
        starts,
        tagged_controls,
        tagged_audio,
        journal,
        ended,
        renewed_starts,
    ) = match wired {
        Some(w) => (
            w.controls,
            w.sessions,
            w.audio,
            w.let_go,
            w.starts,
            w.tagged_controls,
            w.tagged_audio,
            w.journal,
            w.ended,
            w.renewed_starts,
        ),
        None => {
            let (_tx, controls) = mpsc::channel(1);
            let (_tx2, sessions) = mpsc::channel(1);
            let (_tx3, audio) = mpsc::channel(1);
            let (_tx4, let_go) = mpsc::channel(1);
            let (_tx5, tagged_controls) = mpsc::unbounded_channel();
            let (_tx6, tagged_audio) = mpsc::unbounded_channel();
            let (_tx7, journal) = mpsc::unbounded_channel();
            let (_tx8, ended) = mpsc::unbounded_channel();
            (
                controls,
                sessions,
                audio,
                let_go,
                Arc::new(AtomicUsize::new(0)),
                tagged_controls,
                tagged_audio,
                journal,
                ended,
                Arc::new(AtomicUsize::new(0)),
            )
        }
    };

    Live {
        ws_base: format!("ws://{addr}/{MOUNT}"),
        controls,
        sessions,
        audio,
        let_go,
        emissions,
        mailbox: mailbox_tx,
        starts,
        tagged_controls,
        tagged_audio,
        journal,
        ended,
        renewed_starts,
        dead_letters,
        handler,
        io: io_task,
        listener,
        _reconfig: reconfig_tx,
    }
}

impl Live {
    /// Connect a client, retrying while the listener is still coming up.
    pub async fn connect(&self, query: &str) -> (VoiceClient, Value) {
        let url = format!("{}/ws?{query}", self.ws_base);
        let deadline = Instant::now() + MARKER;
        loop {
            match VoiceClient::connect(&url).await {
                Ok(pair) => return pair,
                Err(e) if Instant::now() >= deadline => panic!("never connected to {url}: {e}"),
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }

    /// The handle of the session the cell opened for the connection.
    pub async fn session(&mut self) -> FakeSession {
        tokio::time::timeout(MARKER, self.sessions.recv())
            .await
            .expect("the cell opens a session with the hello, not with the first frame")
            .expect("the provider is running")
    }

    /// The next control frame the model was told.
    pub async fn control(&mut self) -> DuplexControl {
        tokio::time::timeout(MARKER, self.controls.recv())
            .await
            .expect("a control frame reaches the model within the failure marker")
            .expect("the provider is running")
    }

    /// Every control frame the model was told within `window`.
    pub async fn controls_for(&mut self, window: Duration) -> Vec<DuplexControl> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return out;
            }
            match tokio::time::timeout(left, self.controls.recv()).await {
                Ok(Some(c)) => out.push(c),
                Ok(None) | Err(_) => return out,
            }
        }
    }

    /// The next emission whose `hop.route` is `route`, skipping everything
    /// else. Panics at the failure marker, naming what did arrive.
    pub async fn emission(&mut self, route: &str) -> Value {
        let deadline = Instant::now() + MARKER;
        let mut seen: Vec<String> = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("no `{route}` emission within {MARKER:?}; saw {seen:?}");
            }
            match tokio::time::timeout(left, self.emissions.recv()).await {
                Ok(Some(e)) => {
                    let got = emission_route(&e.content).unwrap_or_default().to_string();
                    if got == route {
                        return e.content;
                    }
                    seen.push(got);
                }
                Ok(None) => panic!("the emission channel closed while waiting for `{route}`"),
                Err(_) => panic!("no `{route}` emission within {MARKER:?}; saw {seen:?}"),
            }
        }
    }

    /// Hand the cell one message, the way a topology would.
    pub async fn send(&self, msg: Message) {
        self.mailbox
            .send(msg)
            .await
            .expect("the handler is still reading its mailbox");
    }
}

/// The `hop.route` of an emission's content.
pub fn emission_route(content: &Value) -> Option<&str> {
    content
        .get("header")
        .and_then(|h| h.get("route"))
        .and_then(Value::as_str)
}

/// One header value of an emission's content.
pub fn header<'a>(content: &'a Value, key: &str) -> Option<&'a Value> {
    content.get("header").and_then(|h| h.get(key))
}

/// The `text` of the n-th turn of an emission's body.
pub fn message_text(content: &Value, n: usize) -> String {
    content
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|a| a.get(n))
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `in_advise` message: a section, a call, and a sentence.
pub fn advise_msg(
    session_id: &str,
    section: &str,
    text: &str,
    delegation_id: Option<&str>,
) -> Message {
    advise_msg_with_body(
        session_id,
        section,
        json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]}),
        delegation_id,
    )
}

/// The same lane with a body the caller writes itself.
///
/// The splitter does not write an assistant turn — it writes the section body
/// the model wrote, under `payload` (GH #797): the object as written, or a bare
/// string wrapped once (GH #799). A lock about where the text of an advise comes
/// from has to be able to send the REAL body, not a convenient one.
pub fn advise_msg_with_body(
    session_id: &str,
    section: &str,
    body: Value,
    delegation_id: Option<&str>,
) -> Message {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".into(), json!(session_id));
    if let Some(id) = delegation_id {
        context.insert("delegation_id".into(), json!(id));
    }
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!(section));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(body))
        .build()
}

/// The handover of a renewal (GH #896): an `in_advise` `context` that carries
/// `hop.renewal_n`, the shape the colony answers the `renewed` lane with.
pub fn handover_msg(session_id: &str, text: &str, renewal_n: u32) -> Message {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".into(), json!(session_id));
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("context"));
    hop.insert("renewal_n".into(), json!(renewal_n));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        })))
        .build()
}

/// An `in_speak` message: an assistant turn for a call.
pub fn speak_msg(session_id: &str, text: &str) -> Message {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("call_id".into(), json!(session_id));
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_speak"));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        })))
        .build()
}
