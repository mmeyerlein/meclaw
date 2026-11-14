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
use meclaw_colony::{DbConn, Link, LinkFrame, LinkRequest, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, Headers, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::voice_client::VoiceClient;
use meclaw_testing::{deadline, surface_listener, wait};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The hang mark the harness's tests pass to the client's waits: the repo's
/// `HANG` convention scaled by `MECLAW_TEST_TIME_FACTOR` (TV-Z3), never a
/// verdict. The helper's own waits use the same mark and say `HANG:` when
/// they run out.
pub fn marker() -> Duration {
    deadline::scaled(deadline::HANG)
}

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
    stop: Option<oneshot::Sender<(oneshot::Sender<()>, Option<String>)>>,
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
        self.end_task(None).await;
    }

    /// Like [`Self::stop`], but the provider task ends with the provider's
    /// own refusal: `DuplexError::Refused` carrying `code`, the verdict a
    /// renewal refused for a spent quota returns (review m1, GH #1376).
    pub async fn refuse(&mut self, code: &str) {
        self.end_task(Some(code.to_string())).await;
    }

    /// Ends the provider task, with an ordinary verdict or a refusal.
    async fn end_task(&mut self, refused: Option<String>) {
        let Some(tx) = self.stop.take() else {
            panic!("stop() is a knob of the renewing fake")
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        tx.send((ack_tx, refused))
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
    /// Every control frame, passed on without a bound: see [`Live::controls`].
    controls: mpsc::UnboundedSender<DuplexControl>,
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
            let (stop_tx, mut stop_rx) =
                oneshot::channel::<(oneshot::Sender<()>, Option<String>)>();
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
                            let _ = controls.send(DuplexControl::Close);
                            break;
                        }
                        Some(cmd) => {
                            let _ = tagged_controls.send((index, cmd.clone()));
                            let _ = journal.send((index, Seen::Control(cmd.clone())));
                            let _ = controls.send(cmd);
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
                                    provider_code: None,
                                    client_event_id: None,
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
                        Ok((ack, refused)) => {
                            drop(audio_in);
                            drop(control);
                            let _ = ack.send(());
                            return match refused {
                                Some(code) => Err(DuplexError::Refused {
                                    code,
                                    message: "the provider refused the session".to_string(),
                                }),
                                None => Ok(()),
                            };
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
    ///
    /// Unbounded, so the fake model never waits on a test that does not read
    /// it (GH #1511). A real provider writes each control to
    /// its socket under the operation timeout and ends the session when that
    /// fails (`gpt_live::send_frame`); it never stops reading its control
    /// channel for good. The bounded outlet of 256 this was before did: the
    /// fake parked on the 257th control, the connection's next
    /// `control_tx.send` waited for ever, and a call whose delivery gave up
    /// never ended (`gh_d54g_weblive_a_call_given_up_ends_past_the_models_control_outlet`,
    /// 10 of 10 runs red with the bound).
    pub controls: mpsc::UnboundedReceiver<DuplexControl>,
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
    /// Every message the cell handed to the colony as a dead letter (GH #1016),
    /// as [`drain_dead_letters`] took it out of the cell's inbox.
    pub dead_letters: mpsc::UnboundedReceiver<meclaw_colony::ColonyMsg>,
    /// The registry the cell mounted on: its entry carries the second door,
    /// the in-process link ([`Live::link`]).
    pub surfaces: Arc<SurfaceRegistry>,
    handler: tokio::task::JoinHandle<()>,
    io: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    /// The reader of the cell's dead-letter inbox ([`drain_dead_letters`]).
    dead_drain: tokio::task::JoinHandle<()>,
    /// Held: the I/O half reads this channel closing as "the handler is gone".
    _reconfig: mpsc::Sender<VoiceReconfig>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.handler.abort();
        self.io.abort();
        self.listener.abort();
        self.dead_drain.abort();
    }
}

/// The parts of the fake a caller keeps: the provider and the channels behind
/// it.
struct Wired {
    provider: Arc<dyn DuplexProvider>,
    controls: mpsc::UnboundedReceiver<DuplexControl>,
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
    let (controls_tx, controls) = mpsc::unbounded_channel();
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
        0,
        0,
    )
    .await
}

/// Boot a duplex cell that asks for its seed (D48): `seed_wait_ms` set, with
/// the ring knob and the renewal of GH #896 beside it. The fake is the
/// renewing one where `renew_after_ms` is set, the plain one otherwise.
/// `seed_wait_ms` reaches the I/O half from `params.duplex` through the
/// factory, which this fixture does not run.
pub async fn boot_seeded(
    raw: Value,
    prewarm_ttl_ms: u64,
    renew_after_ms: u64,
    renew_grace_ms: u64,
    seed_wait_ms: u64,
) -> Live {
    let renewing = renew_after_ms > 0;
    let wired = wire_fake_full(false, false, RATE, 4096, renewing, 64);
    boot_core(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        renew_after_ms,
        renew_grace_ms,
        (
            prewarm_ttl_ms,
            meclaw_cells::voice::duck::DuckParams::default(),
        ),
        seed_wait_ms,
        0,
    )
    .await
}

/// Boot a duplex cell that recovers a call after its provider ended the
/// session in charge (GH #1377): `recover_max` attempts per call, with the
/// renewing fake -- whose [`FakeSession::stop`] plays the provider ending a
/// session on its own -- and `seed_wait_ms` set, so every session that opens
/// asks for its seed. `renew_after_ms` `0` renews only where an `in_advise`
/// `renew` asks. Both reach the I/O half from `params.duplex` through the
/// factory, which this fixture does not run.
pub async fn boot_recovering(
    raw: Value,
    recover_max: u32,
    renew_after_ms: u64,
    seed_wait_ms: u64,
) -> Live {
    let wired = wire_fake_full(false, false, RATE, 4096, true, 64);
    boot_core(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        renew_after_ms,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (0, meclaw_cells::voice::duck::DuckParams::default()),
        seed_wait_ms,
        recover_max,
    )
    .await
}

/// Boot a duplex cell around a provider the test built itself -- the shipped
/// `gpt_live` adapter against `meclaw_testing::mock_gpt_live`, say (GH #1403)
/// -- recovering `recover_max` times per call and asking for seeds where
/// `seed_wait_ms` is set, as [`boot_recovering`] does. Nothing of the fake is
/// wired: what the provider saw is read off its own mock.
pub async fn boot_recovering_with(
    raw: Value,
    provider: Arc<dyn DuplexProvider>,
    recover_max: u32,
    seed_wait_ms: u64,
) -> Live {
    boot_core(
        raw,
        Some(provider),
        None,
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        0,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (0, meclaw_cells::voice::duck::DuckParams::default()),
        seed_wait_ms,
        recover_max,
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
        0,
        0,
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
    seed_wait_ms: u64,
    recover_max: u32,
) -> Live {
    boot_core_paged(
        raw,
        duplex,
        wired,
        quiet_ms,
        cap_ms,
        close_grace_ms,
        tick_ms,
        renew_after_ms,
        renew_grace_ms,
        edge,
        seed_wait_ms,
        recover_max,
        0,
    )
    .await
}

/// [`boot_core`] with the idle end of a page session (d54g weblive,
/// `params.duplex.hold_idle_ms`, `0` = never).
#[allow(clippy::too_many_arguments)]
async fn boot_core_paged(
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
    seed_wait_ms: u64,
    recover_max: u32,
    hold_idle_ms: u64,
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
    io.duplex_seed_wait_ms = seed_wait_ms;
    // D54g kprofil: the limits of the ask come from the params document, as
    // in the factory.
    io.duplex_limits = match &params.duplex {
        Some(meclaw_cells::voice::params::DuplexParams::GptLive(g)) => {
            Some(meclaw_cells::voice::providers::gpt_live::ask_limits(g))
        }
        _ => None,
    };
    io.duplex_recover_max = recover_max;
    io.duplex_hold_idle_ms = hold_idle_ms;
    // D48/DK: the call marks come from the params document, as in the factory.
    io.duplex_call_marks = params.call_marks.clone();
    io.cell_path = Path::new("/voice");
    io.surfaces = Arc::clone(&surfaces);

    // GH #1016: the colony inbox a cell dead-letters into, read by the test.
    let (dead_tx, dead_inbox) = mpsc::channel::<meclaw_colony::ColonyMsg>(DEAD_LETTER_INBOX);
    let (dead_letters, dead_drain) = drain_dead_letters(dead_inbox);
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
    // The registry has no event for a mount, so this is a condition poll
    // and goes through the kit (TV-S1 (3), TV-S3): scaled hang mark, `HANG:`
    // with the condition when it runs out, no hand-rolled sleep.
    wait::until(
        "the cell registers its mount (surface table non-empty)",
        || {
            let surfaces = Arc::clone(&surfaces);
            async move { !surfaces.table().await.is_empty() }
        },
    )
    .await;

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
            let (_tx, controls) = mpsc::unbounded_channel();
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
        dead_drain,
        surfaces,
        _reconfig: reconfig_tx,
    }
}

/// A client on the in-process link ([`Live::link`]).
pub struct LinkClient {
    link: Link,
}

impl LinkClient {
    /// The next binary (audio) frame on the link, skipping text frames.
    /// Panics at the hang mark (TV-Z3).
    pub async fn next_audio(&mut self) -> Vec<u8> {
        let from_cell = &mut self.link.from_cell;
        let found = tokio::time::timeout(deadline::scaled(deadline::HANG), async {
            while let Some(frame) = from_cell.recv().await {
                if let LinkFrame::Binary(b) = frame {
                    return Some(b.to_vec());
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(b)) => b,
            Ok(None) => panic!("the link closed while waiting for an audio frame"),
            Err(_) => panic!("HANG: no audio frame on the link"),
        }
    }

    /// The next text frame whose `type` is `ty`, skipping everything else.
    /// Panics at the hang mark (TV-Z3), naming the frame types that did come.
    pub async fn frame_of_type(&mut self, ty: &str) -> Value {
        let mut seen: Vec<String> = Vec::new();
        let from_cell = &mut self.link.from_cell;
        let found = tokio::time::timeout(deadline::scaled(deadline::HANG), async {
            while let Some(frame) = from_cell.recv().await {
                match frame {
                    LinkFrame::Text(text) => {
                        let v: Value =
                            meclaw_core::serde_json::from_str(&text).expect("a text frame is JSON");
                        let got = v.get("type").and_then(Value::as_str).unwrap_or_default();
                        if got == ty {
                            return Some(v);
                        }
                        seen.push(got.to_string());
                    }
                    LinkFrame::Binary(b) => seen.push(format!("binary({})", b.len())),
                    LinkFrame::Close { code, reason } => {
                        seen.push(format!("close({code}, {reason})"))
                    }
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(v)) => v,
            Ok(None) => panic!("the link closed while waiting for a `{ty}` frame; saw {seen:?}"),
            Err(_) => panic!(
                "HANG: no `{ty}` frame on the link within {} s; last state: saw {seen:?}",
                deadline::scaled(deadline::HANG).as_secs()
            ),
        }
    }
}

impl Live {
    /// Connect a client. The listener is bound before `boot` returns and the
    /// mount is registered, so the first attempt normally lands; a refused
    /// attempt is retried through the kit (TV-S3: scaled hang mark, `HANG:`),
    /// and each refusal is logged so a hang names the last error.
    pub async fn connect(&self, query: &str) -> (VoiceClient, Value) {
        let url = format!("{}/ws?{query}", self.ws_base);
        let desc = format!("a client connects to {url} (refusals logged above)");
        wait::until_some(&desc, || {
            let url = url.clone();
            async move {
                match VoiceClient::connect(&url).await {
                    Ok(pair) => Some(pair),
                    Err(e) => {
                        eprintln!("connect to {url} refused, retrying: {e}");
                        None
                    }
                }
            }
        })
        .await
    }

    /// The handle of the session the cell opened for the connection.
    pub async fn session(&mut self) -> FakeSession {
        tokio::time::timeout(deadline::scaled(deadline::HANG), self.sessions.recv())
            .await
            .expect(
                "HANG: no session -- the cell opens one with the hello, not with the first frame",
            )
            .expect("the provider is running")
    }

    /// The next control frame the model was told.
    pub async fn control(&mut self) -> DuplexControl {
        tokio::time::timeout(deadline::scaled(deadline::HANG), self.controls.recv())
            .await
            .expect("HANG: no control frame reached the model")
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
        // TV-Z3: the hang mark of `meclaw_testing::deadline`, on the tokio
        // clock -- under a paused clock it is virtual like every other wait
        // of the paused part, and a missing emission ends the test at once.
        let mut seen: Vec<String> = Vec::new();
        let emissions = &mut self.emissions;
        let found = tokio::time::timeout(deadline::scaled(deadline::HANG), async {
            while let Some(e) = emissions.recv().await {
                let got = emission_route(&e.content).unwrap_or_default().to_string();
                if got == route {
                    return Some(e.content);
                }
                seen.push(got);
            }
            None
        })
        .await;
        match found {
            Ok(Some(content)) => content,
            Ok(None) => {
                panic!("the emission channel closed while waiting for `{route}`; saw {seen:?}")
            }
            Err(_) => panic!(
                "HANG: no `{route}` emission within {} s; last state: saw {seen:?}",
                deadline::scaled(deadline::HANG).as_secs()
            ),
        }
    }

    /// Open a client on the cell's second door, the in-process link
    /// (`VoiceLinkOpener`, the door a display's socket topic takes): same
    /// admission and same connection code as the socket door, with the frames
    /// on tokio channels (`ClientLink::chan`). A test of time logic connects
    /// here, never over a socket (TV-Z7). Returns the client and its `hello`.
    pub async fn link(&self, session: &str, mode: &str) -> (LinkClient, Value) {
        let opened = self
            .surfaces
            .open_link(
                MOUNT,
                LinkRequest {
                    session: Some(session.to_string()),
                    mode: Some(mode.to_string()),
                    ..LinkRequest::default()
                },
            )
            .await
            .expect("the cell mounted a link opener")
            .expect("the link is admitted");
        let mut client = LinkClient { link: opened };
        let hello = client.frame_of_type("hello").await;
        (client, hello)
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

// callseam

/// The shipped `session-keeper` in a colony of its own, for the one pin of the
/// duplex harness that is about the keeper and not about the voice cell
/// (d54g callseam, contract point 2).
///
/// Taken over from `misc_2/gh1401_a_call_end_closes_the_phone_generation.rs`,
/// with neutral names: a parent hive around the shipped keeper, drawn the way
/// `talky` draws it. A turn goes in through a probe, the stamped turn comes out
/// to a stand-in that answers with the session id it was stamped with, the end
/// of a live session goes in on `in_call_end`, and every `close` the keeper
/// asks for is captured.
///
/// The cell that opens a session on a turn is `stamp` (lane `in_turn`, the
/// `look` phase inserts the row into `sessions`); nothing here names it, the
/// pin reads what the keeper as a whole did.
pub mod keeper {
    use meclaw_cells::code::CodeCellFactory;
    use meclaw_cells::store::StoreCellFactory;
    use meclaw_cells::timer::TimerCellFactory;
    use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
    use meclaw_core::serde_json::{self as sj, Value, json};
    use meclaw_core::{Body, Message, MessageBuilder, Path};
    use meclaw_testing::ColonyHandle;
    use meclaw_testing::deadline;
    use meclaw_testing::topologies::phase_3a::CaptureCell;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// The channel cell a live session comes in by.
    pub const LIVE_NODE: &str = "voice";

    fn repo(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    /// R2b / GH #49: a tree without the template skips.
    pub fn shipped() -> bool {
        repo("templates/session-keeper/stamp/config.json").is_file()
            && repo("templates/session-keeper/close/config.json").is_file()
    }

    /// Turns a harness message into an inbound turn (the lane is named by the edge).
    const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"}, "messages": d.get("messages", [])}))
"#;

    /// Everything downstream of the stamp: answers with the session id the
    /// turn was stamped with, and the turn's own words after it.
    const REPORT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
said = ""
for m in (doc.get("body") or {}).get("messages") or []:
    if isinstance(m, dict) and m.get("origin") == "user":
        said = str(m.get("text", ""))
sys.stdout.write(json.dumps({"header": {"route": "report"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("session_id", "")) + "|" + said}]}))
"#;

    /// The stand-in for the consumer of a `close`: one `closed` per close,
    /// naming the session and the reason the keeper gave.
    const CLOSED: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "closed"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("session_id", ""))
                                                   + "|" + str(ctx.get("close_reason", ""))}]}))
"#;

    fn code_cell(script: &str, routes: &[&str]) -> Value {
        json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
            "contract": {
                "version": "1.0.0",
                "settings": {},
                "multi_send_capable": true,
                "emits": {
                    "body": {"messages": {"type": "array", "required": true}},
                    "hop": {"route": {"type": "string", "values": routes, "required": false}}
                },
                "consumes": {"body": {"messages": {"type": "array", "required": true}}},
                "capabilities": ["shell:exec"]
            },
            "description": {
                "purpose": "Test stand-in around the keeper.",
                "use_when": "Test fixture only.",
                "not_in_scope": "Not a template."
            }
        })
    }

    /// The edge of a parent that draws the end of a live session onto the
    /// keeper's `in_call_end` by hand: it hands on the context it gets,
    /// `context.live` included.
    fn hand_drawn_call_end_edge() -> Value {
        json!({"from": ".", "to": "./session-keeper",
               "condition": "hop.route == 'live_end'",
               "modifier": {"set_hop": {"route": "'in_call_end'"}}})
    }

    /// The edge `talky` ships from its own door to its keeper for
    /// `in_call_end`, read from the template unchanged: whatever it promotes
    /// into the context is what the keeper gets.
    fn talky_call_end_edge() -> Value {
        let talky: Value = sj::from_str(
            &std::fs::read_to_string(repo("templates/talky/config.json"))
                .expect("read the talky template"),
        )
        .expect("the talky template is json");
        talky["params"]["graph"]["edges"]
            .as_array()
            .expect("talky draws edges")
            .iter()
            .find(|e| {
                e["from"] == "."
                    && e["to"] == "./session-keeper"
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("'in_call_end'"))
            })
            .cloned()
            .expect("talky draws in_call_end to its keeper")
    }

    fn main_config(call_end_edge: Value) -> Value {
        json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./probe", "to": "./session-keeper",
             "condition": "hop.route == 'turn'",
             "modifier": {"set_hop": {"route": "'in_turn'"}}},
            call_end_edge,
            {"from": "./session-keeper", "to": "/rejects",
             "condition": "hop.route == 'reject'"},
            {"from": ".", "to": "./session-keeper",
             "condition": "hop.route == 'answered'",
             "modifier": {"set_hop": {"route": "'in_answered'"}}},
            {"from": "./session-keeper", "to": "./report",
             "condition": "hop.route == 'turn'",
             "modifier": {"set_context": {"session_id": "hop.session_id"}}},
            {"from": "./report", "to": "/sink", "condition": "hop.route == 'report'"},
            {"from": "./session-keeper", "to": "./closed",
             "condition": "hop.route == 'close'",
             "modifier": {"set_context": {"session_id": "hop.session_id",
                                          "close_reason": "has(hop.close_reason) ? hop.close_reason : ''"}}},
            {"from": "./closed", "to": "/closes", "condition": "hop.route == 'closed'"}
        ]}}})
    }

    fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).expect("create the cell directory");
        for entry in std::fs::read_dir(src).expect("read the template directory") {
            let entry = entry.expect("a directory entry");
            let from = entry.path();
            if from.is_dir() {
                copy_cells(&from, &dst.join(entry.file_name()));
            } else if entry.file_name() == "config.json" {
                std::fs::copy(&from, dst.join("config.json")).expect("copy a config.json");
            }
        }
    }

    fn write(root: &std::path::Path, rel: &str, v: &Value) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent directory"))
            .expect("create the parent directory");
        std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write the config");
    }

    /// The keeper colony and the two places its answers arrive.
    pub struct KeeperColony {
        /// The colony itself.
        pub h: ColonyHandle,
        /// One report per turn the keeper stamped and handed on:
        /// `<session id>|<the turn's words>`.
        pub sink: mpsc::Receiver<Message>,
        /// One message per `close` the keeper asked for: `<session id>|<reason>`.
        pub closes: mpsc::Receiver<Message>,
        /// Every `reject` the keeper emitted, as it left the keeper.
        pub rejects: mpsc::Receiver<Message>,
        td: tempfile::TempDir,
    }

    /// Boot the shipped keeper with its shipped idle threshold; only the
    /// night window goes empty, so a run does not depend on its hour.
    pub async fn boot_keeper() -> KeeperColony {
        boot_keeper_with(hand_drawn_call_end_edge()).await
    }

    /// The same keeper, but the end of a live session reaches it over the
    /// edge `talky` ships to its keeper: a message on the parent's door with
    /// `hop.route` `in_call_end`, as the voice cell's final `usage` arrives
    /// (see [`call_end_as_talky_gets_it`]).
    pub async fn boot_keeper_wired_like_talky() -> KeeperColony {
        boot_keeper_with(talky_call_end_edge()).await
    }

    async fn boot_keeper_with(call_end_edge: Value) -> KeeperColony {
        let td = tempfile::TempDir::new().expect("a temp dir");
        let root = td.path();
        std::fs::write(root.join(".env"), "").expect("an empty env file");
        write(root, "main/config.json", &main_config(call_end_edge));
        copy_cells(
            &repo("templates/session-keeper"),
            &root.join("main/session-keeper"),
        );
        meclaw_testing::quiet_keeper_night(&root.join("main/session-keeper"));
        write(root, "main/probe/config.json", &code_cell(PROBE, &["turn"]));
        write(
            root,
            "main/report/config.json",
            &code_cell(REPORT, &["report"]),
        );
        write(
            root,
            "main/closed/config.json",
            &code_cell(CLOSED, &["closed"]),
        );

        let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
            vec![
                (
                    "code".to_string(),
                    Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
                ),
                ("store".to_string(), Arc::new(StoreCellFactory)),
                ("timer".to_string(), Arc::new(TimerCellFactory)),
            ]
        };
        let h = ColonyHandle::new_with_factories_at(&td, factories());
        let (sink_tx, sink) = mpsc::channel::<Message>(64);
        let (closes_tx, closes) = mpsc::channel::<Message>(64);
        h.spawn(Path::new("/sink"), move || {
            CaptureCell::new(sink_tx.clone())
        })
        .await;
        h.spawn(Path::new("/closes"), move || {
            CaptureCell::new(closes_tx.clone())
        })
        .await;
        let (rejects_tx, rejects) = mpsc::channel::<Message>(64);
        h.spawn(Path::new("/rejects"), move || {
            CaptureCell::new(rejects_tx.clone())
        })
        .await;
        let mut registry = CellFactoryRegistry::new();
        for (name, f) in factories() {
            registry.insert(name, f);
        }
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
            .await
            .expect("bootstrap_from_filesystem must succeed");
        KeeperColony {
            h,
            sink,
            closes,
            rejects,
            td,
        }
    }

    /// One turn at the keeper's door. `live` is the handle of the running
    /// session the turn belongs to (`context.live`, D58 live); `None` is a
    /// turn that belongs to none.
    pub fn turn(
        channel: &str,
        node: &str,
        text: &str,
        turn_id: &str,
        live: Option<&str>,
    ) -> Message {
        let mut ctx = sj::Map::new();
        ctx.insert("channel".into(), json!(channel));
        ctx.insert("channel_node".into(), json!(node));
        ctx.insert("turn_id".into(), json!(turn_id));
        if let Some(live) = live {
            ctx.insert("live".into(), json!(live));
        }
        MessageBuilder::new(Path::new("/probe"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
            ))
            .context(ctx)
            .ttl(64)
            .build()
    }

    /// The final answer of turn `turn_id` reaching the wall: what a parent
    /// draws from the writer's `turn_write` onto the keeper's `in_answered`.
    pub fn answered(session: &str, turn_id: &str) -> Message {
        let mut ctx = sj::Map::new();
        ctx.insert("turn_id".into(), json!(turn_id));
        let mut hop = sj::Map::new();
        hop.insert("route".into(), json!("answered"));
        hop.insert("session_id".into(), json!(session));
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": [
                {"origin": "assistant", "type": "text", "text": "the final answer"}]})))
            .ttl(64)
            .build()
    }

    /// The end of the live session `live`, as a parent hands it to the keeper:
    /// the voice cell's final `usage` on `in_call_end`, naming the session
    /// under the handle and under the key the voice cell itself uses.
    pub fn live_end(node: &str, live: &str) -> Message {
        let mut ctx = sj::Map::new();
        ctx.insert("channel_node".into(), json!(node));
        ctx.insert("live".into(), json!(live));
        ctx.insert("call_id".into(), json!(live));
        let mut hop = sj::Map::new();
        hop.insert("route".into(), json!("live_end"));
        hop.insert("call_id".into(), json!(live));
        hop.insert("session_id".into(), json!(live));
        hop.insert("final".into(), json!(true));
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": []})))
            .ttl(64)
            .build()
    }

    /// The end of the live session `call_id` as it reaches `talky`'s door:
    /// the voice cell's final `usage` of a duplex call, drawn onto
    /// `in_call_end` (`hop.call_id`, `hop.engine`, `hop.session_id`). No
    /// `context.live`: the voice cell names the session on the hop only, so
    /// whatever handle the keeper gets, the edge has to promote it.
    pub fn call_end_as_talky_gets_it(node: &str, call_id: &str) -> Message {
        let mut ctx = sj::Map::new();
        ctx.insert("channel_node".into(), json!(node));
        ctx.insert("call_id".into(), json!(call_id));
        let mut hop = sj::Map::new();
        hop.insert("route".into(), json!("in_call_end"));
        hop.insert("call_id".into(), json!(call_id));
        hop.insert("engine".into(), json!("duplex"));
        hop.insert("session_id".into(), json!(call_id));
        hop.insert("final".into(), json!(true));
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": []})))
            .ttl(64)
            .build()
    }

    /// True where the tree carries the `talky` template as well.
    pub fn talky_shipped() -> bool {
        repo("templates/talky/config.json").is_file()
    }

    /// The first text of a captured message.
    pub fn text_of(m: &Message) -> String {
        match &m.body {
            Body::Inline(v) => v["messages"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            Body::Blob(_) => panic!("inline expected"),
        }
    }

    impl KeeperColony {
        /// Send a turn and wait for the report of the turn the keeper handed
        /// on: `(session id, the turn's words)`.
        pub async fn say(&mut self, msg: Message) -> (String, String) {
            self.h.send(msg).await;
            let got = deadline::hang(
                "the keeper stamps the turn and hands it on",
                self.sink.recv(),
            )
            .await
            .expect("the sink is open");
            let text = text_of(&got);
            let (session, said) = text.split_once('|').unwrap_or((text.as_str(), ""));
            (session.to_string(), said.to_string())
        }

        /// The next `close` the keeper asked for: `(session id, reason)`.
        pub async fn next_close(&mut self) -> (String, String) {
            let got = deadline::hang("the keeper asks for a close", self.closes.recv())
                .await
                .expect("the close capture is open");
            let text = text_of(&got);
            let (session, reason) = text.split_once('|').unwrap_or((text.as_str(), ""));
            (session.to_string(), reason.to_string())
        }

        /// The next `reject` the keeper emitted, as it left the keeper.
        pub async fn next_reject(&mut self) -> Message {
            deadline::hang("the keeper refuses the turn", self.rejects.recv())
                .await
                .expect("the reject capture is open")
        }

        /// `(session_id, channel_node, closed)` of every generation the
        /// keeper holds, oldest first. A read of the test's own tree, after a
        /// `quiesce`.
        pub fn generations(&self) -> Vec<(String, String, i64)> {
            let db = self.td.path().join("main/session-keeper/sessions/cell.db");
            let conn = rusqlite::Connection::open(db).expect("sessions cell.db");
            let mut st = conn
                .prepare(
                    "SELECT session_id, COALESCE(channel_node, ''), closed FROM sessions \
                     ORDER BY opened_at",
                )
                .expect("prepare");
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .expect("query")
                .filter_map(Result::ok)
                .collect()
        }
    }
}

// prewarm

/// How the first session of a [`DeadFirst`] provider is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadHow {
    /// Its handshake failed: the provider's future returns an error before
    /// the session ever started.
    Refused,
    /// It ran and ended on its own: a close, then an ordinary verdict.
    Over,
}

/// A provider whose FIRST session is gone at once and whose later ones are
/// the plain fake's.
///
/// That is a session opened while the line rings whose provider gave up
/// before the pickup. The first session publishes no handle and counts in no
/// counter of the fake: [`Live::sessions`] and [`Live::starts`] speak of the
/// sessions after it, [`DeadFirstProbe::opened`] of all of them.
pub struct DeadFirst {
    inner: Arc<dyn DuplexProvider>,
    opened: Arc<AtomicUsize>,
    how: DeadHow,
}

impl DuplexProvider for DeadFirst {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn rates(&self) -> Vec<u32> {
        self.inner.rates()
    }

    fn run_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        if self.opened.fetch_add(1, Ordering::SeqCst) > 0 {
            return self.inner.run_session(format, session, liveness);
        }
        let how = self.how;
        Box::pin(async move {
            // Held whole until the verdict: the session's channels close when
            // its provider is gone, not before.
            let session = session;
            match how {
                DeadHow::Refused => Err(DuplexError::Closed("the handshake failed".to_string())),
                DeadHow::Over => {
                    let _ = session.events.try_send(DuplexEvent::Closed {
                        reason: "connection_lost".to_string(),
                        usage_seconds: 0.0,
                    });
                    Ok(())
                }
            }
        })
    }

    fn run_renewed_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.inner.run_renewed_session(format, session, liveness)
    }
}

/// What a test holds of a [`DeadFirst`] provider.
pub struct DeadFirstProbe {
    provider: Arc<dyn DuplexProvider>,
    opened: Arc<AtomicUsize>,
    /// Who holds the provider while no session runs.
    idle_holders: usize,
}

impl DeadFirstProbe {
    /// How many sessions the cell opened, the dead one included.
    pub fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }

    /// The first session was opened and the task that ran it is gone.
    ///
    /// The cell runs a session in a task that holds the provider and hands
    /// the verdict back before it ends. So once nobody but the idle cell
    /// holds the provider, the verdict of the dead session is where the cell
    /// can read it: a later connection finds a session that IS over, not one
    /// about to be.
    pub async fn first_run_is_over(&self) {
        wait::until(
            "the early session was opened and its provider task has ended",
            || {
                let over = self.opened.load(Ordering::SeqCst) >= 1
                    && Arc::strong_count(&self.provider) == self.idle_holders;
                async move { over }
            },
        )
        .await;
    }
}

/// [`boot_edge`] around a [`DeadFirst`] provider.
pub async fn boot_edge_dead_first(
    raw: Value,
    prewarm_ttl_ms: u64,
    duck: meclaw_cells::voice::duck::DuckParams,
    how: DeadHow,
) -> (Live, DeadFirstProbe) {
    let wired = wire_fake(false, false, RATE);
    let opened = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn DuplexProvider> = Arc::new(DeadFirst {
        inner: wired.provider.clone(),
        opened: Arc::clone(&opened),
        how,
    });
    let live = boot_core(
        raw,
        Some(provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_AFTER_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (prewarm_ttl_ms, duck),
        0,
        0,
    )
    .await;
    let idle_holders = Arc::strong_count(&provider);
    (
        live,
        DeadFirstProbe {
            provider,
            opened,
            idle_holders,
        },
    )
}

// weblive

impl LinkClient {
    /// One text frame from the client, the JSON the page pushes.
    pub async fn send_text(&self, frame: Value) {
        self.link
            .to_cell
            .send(LinkFrame::Text(frame.to_string()))
            .await
            .expect("the cell is still reading the link");
    }

    /// The key goes down: `{"type":"hold"}`.
    pub async fn hold(&self) {
        self.send_text(json!({"type": "hold"})).await;
    }

    /// The key comes up: `{"type":"release"}`.
    pub async fn release(&self) {
        self.send_text(json!({"type": "release"})).await;
    }

    /// One binary frame from the client: microphone audio.
    pub async fn send_audio(&self, bytes: Vec<u8>) {
        self.link
            .to_cell
            .send(LinkFrame::Binary(bytes))
            .await
            .expect("the cell is still reading the link");
    }

    /// The page says the key is about to go down (`{"type":"ready"}`): the
    /// pointer is on the key, or the microphone was granted (d54g weblive).
    #[allow(dead_code)]
    pub async fn ready(&self) {
        self.send_text(json!({"type": "ready"})).await;
    }

    /// Read the link to its end on a task of its own, the way a client that
    /// takes every frame does: resolves with the close code the cell sent
    /// (or `None` without one) once the cell let go of the link. Whatever the
    /// cell queued for the client is taken, so a connection parked on a full
    /// link runs on.
    ///
    /// The client's own half stays open until the cell let go: the client is
    /// still on the line. A task that kept only the reading half would hang
    /// up the moment it started -- and a hang-up ends any call, which is not
    /// what a test of another way to the end may lean on (review I-1 of d54g
    /// weblive: the first form of this helper did exactly that, and its pin
    /// was green on the code it was written against).
    #[allow(dead_code)]
    pub fn drain(self) -> tokio::task::JoinHandle<Option<u16>> {
        let Link {
            to_cell,
            mut from_cell,
        } = self.link;
        tokio::spawn(async move {
            let mut code = None;
            while let Some(frame) = from_cell.recv().await {
                if let LinkFrame::Close { code: c, .. } = frame {
                    code = Some(c);
                }
            }
            drop(to_cell);
            code
        })
    }

    /// Whether the cell closed the link, read without waiting: `Some` with the
    /// close code once a close frame is the next thing on the link, `None`
    /// while nothing is waiting. Frames before the close are skipped.
    #[allow(dead_code)]
    pub fn closed_now(&mut self) -> Option<u16> {
        while let Ok(frame) = self.link.from_cell.try_recv() {
            if let LinkFrame::Close { code, .. } = frame {
                return Some(code);
            }
        }
        None
    }
}

/// A duplex cell for a page (d54g weblive): its sessions end after
/// `hold_idle_ms` without a hold, an early word of the page opens one ahead
/// of its first hold for `prewarm_ttl_ms`, and the seed of each is asked for
/// where `seed_wait_ms` is set.
#[allow(dead_code)]
pub async fn boot_page(
    raw: Value,
    hold_idle_ms: u64,
    prewarm_ttl_ms: u64,
    seed_wait_ms: u64,
) -> Live {
    let wired = wire_fake(false, false, RATE);
    boot_core_paged(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        CLOSE_GRACE_MS,
        TICK_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_AFTER_MS,
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (
            prewarm_ttl_ms,
            meclaw_cells::voice::duck::DuckParams::default(),
        ),
        seed_wait_ms,
        0,
        hold_idle_ms,
    )
    .await
}

/// The two doors a person takes into a live session (d54g weblive, contract
/// 6): the guarantees of `callseam` and the append ceiling of `deleg` hold for
/// both, so a lock about them drives each door with the same steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// A call: the connection is the session, its ear open from the `hello`
    /// (`auto`), and the session carries the connection's name.
    Phone,
    /// A page: the connection waits in `hold` until the key goes down; each
    /// session opens with a first hold and carries the connection's name and
    /// its number (`<name>-<n>`, OR-54g-21 e).
    Page,
}

impl Access {
    /// Both doors, the phone first.
    pub const BOTH: [Access; 2] = [Access::Phone, Access::Page];

    /// The name the cell gives the `n`-th session (from 1) of a line named
    /// `tag`: what `call_id` and `session_id` of its emissions carry and what
    /// the colony addresses it by. A call has one session per connection.
    pub fn session_name(self, tag: &str, n: u32) -> String {
        match self {
            Access::Phone => tag.to_string(),
            Access::Page => format!("{tag}-{n}"),
        }
    }
}

/// How a session of a line ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The person goes: the caller hangs up, the page goes away.
    Gone,
    /// The page stays and was idle for `hold_idle_ms` (contract 4); the
    /// phone has no such end.
    Idle,
}

/// One person's line into the cell: the socket of a call or the link of a
/// page (or of a call, where a test of time logic must not use a socket,
/// TV-Z7).
pub enum Line {
    /// The socket door.
    Socket(VoiceClient),
    /// The in-process link, the door a display's socket topic takes.
    Link(LinkClient),
}

impl Line {
    /// Microphone audio from the person.
    pub async fn send_audio(&mut self, bytes: &[u8]) {
        match self {
            Line::Socket(c) => c.send_audio(bytes).await.expect("the socket is open"),
            Line::Link(c) => c.send_audio(bytes.to_vec()).await,
        }
    }

    /// The next frame of the model's audio the person hears, under the hang
    /// mark.
    pub async fn hear(&mut self) -> Vec<u8> {
        match self {
            Line::Socket(c) => {
                let heard = c.collect_until(|f| f.as_audio().is_some(), marker()).await;
                heard
                    .iter()
                    .find_map(|(_, f)| f.as_audio().map(<[u8]>::to_vec))
                    .unwrap_or_else(|| {
                        panic!(
                            "HANG: the caller heard no audio of the model within {} s",
                            marker().as_secs()
                        )
                    })
            }
            Line::Link(c) => c.next_audio().await,
        }
    }

    /// The key comes up (a page only): the end of the person's utterance.
    pub async fn release(&mut self) {
        match self {
            Line::Link(c) => c.release().await,
            Line::Socket(c) => c.release().await.expect("the socket is open"),
        }
    }

    /// The key goes down (a page only).
    pub async fn hold(&mut self) {
        match self {
            Line::Link(c) => c.hold().await,
            Line::Socket(c) => c.hold().await.expect("the socket is open"),
        }
    }

    /// The person goes: the caller hangs up, the page closes its link.
    pub async fn hang_up(self) {
        match self {
            Line::Socket(c) => c.close().await.expect("the caller hangs up"),
            Line::Link(c) => {
                let _ = c
                    .link
                    .to_cell
                    .send(LinkFrame::Close {
                        code: 1000,
                        reason: "the page goes".to_string(),
                    })
                    .await;
            }
        }
    }
}

impl Live {
    /// Open a line through `access` and take the handle of the session the
    /// cell opens for it. The phone connects over the socket in `auto` and
    /// gets its session with the `hello`; the page opens its link in `hold`,
    /// puts the key down, and the session opens with the model's ear open
    /// (the `Unmute` is read off both control channels here, so a test reads
    /// on behind it as it would behind a call's start).
    pub async fn open_line(&mut self, access: Access, tag: &str) -> (Line, FakeSession) {
        match access {
            Access::Phone => {
                let (client, hello) = self.connect(&format!("session={tag}&mode=auto")).await;
                assert_eq!(hello["duplex"], true, "got {hello}");
                let session = self.session().await;
                (Line::Socket(client), session)
            }
            Access::Page => {
                let (client, hello) = self.link(tag, "hold").await;
                assert_eq!(hello["mode"], json!("hold"), "got {hello}");
                let mut line = Line::Link(client);
                let session = self.key_down(&mut line).await;
                (line, session)
            }
        }
    }

    /// [`Self::open_line`] over the in-process link for both doors -- the
    /// phone's link in `auto` -- for the tests of time logic on a paused
    /// clock (TV-Z7).
    pub async fn open_linked(&mut self, access: Access, tag: &str) -> (Line, FakeSession) {
        match access {
            Access::Phone => {
                let (client, _hello) = self.link(tag, "auto").await;
                let session = self.session().await;
                (Line::Link(client), session)
            }
            Access::Page => {
                let (client, _hello) = self.link(tag, "hold").await;
                let mut line = Line::Link(client);
                let session = self.key_down(&mut line).await;
                (line, session)
            }
        }
    }

    /// The key of a page goes down on a line with no session open: the
    /// session that opens with it, its ear opened (contract 4).
    pub async fn key_down(&mut self, line: &mut Line) -> FakeSession {
        line.hold().await;
        let session = self.session().await;
        // The first thing the new session is told is the open ear; what an
        // earlier session of the line was told before it (the release's
        // `Mute`) is passed over on both channels.
        let first = loop {
            let (index, control) = tokio::time::timeout(marker(), self.tagged_controls.recv())
                .await
                .expect("HANG: the new session was told nothing")
                .expect("the fake is running");
            if index == session.index {
                break control;
            }
        };
        assert_eq!(
            first,
            DuplexControl::Unmute,
            "the hold opens the page's session with the model's ear open"
        );
        while self.control().await != DuplexControl::Unmute {}
        session
    }
}

/// Which fake a page cell with an idle end runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFake {
    /// The fake of [`boot_fake`].
    Plain,
    /// The fake of [`boot_lingering`]: it never finalises.
    Lingering,
    /// The fake of [`boot_renewing`] without a renewal: a session whose way
    /// in closed holds its verdict until [`FakeSession::finish`].
    Holding,
}

/// A duplex cell whose page sessions end after `hold_idle_ms` without a hold
/// (d54g weblive, contract 4), around the fake `fake`, with the close grace
/// `close_grace_ms`.
pub async fn boot_idle(raw: Value, fake: PageFake, close_grace_ms: u64, hold_idle_ms: u64) -> Live {
    let wired = match fake {
        PageFake::Plain => wire_fake(false, false, RATE),
        PageFake::Lingering => wire_fake(false, true, RATE),
        PageFake::Holding => wire_fake_full(false, false, RATE, 4096, true, 64),
    };
    boot_core_paged(
        raw,
        Some(wired.provider.clone()),
        Some(wired),
        QUIET_MS,
        CAP_MS,
        close_grace_ms,
        TICK_MS,
        if fake == PageFake::Holding {
            0
        } else {
            meclaw_cells::voice::params::DEFAULT_RENEW_AFTER_MS
        },
        meclaw_cells::voice::params::DEFAULT_RENEW_GRACE_MS,
        (0, meclaw_cells::voice::duck::DuckParams::default()),
        0,
        0,
        hold_idle_ms,
    )
    .await
}

/// The end of a live session as it reaches `talky`'s door, built from the
/// voice cell's own final `usage` of that session: its hop (`call_id`,
/// `engine`, `session_id`, `final`) as the cell sent it, the lane renamed the
/// way a parent draws it onto `in_call_end`. Where
/// [`keeper::call_end_as_talky_gets_it`] writes the hop by hand, this one
/// carries whatever the cell said.
pub fn call_end_from_usage(node: &str, usage: &Value) -> Message {
    let hop = usage
        .get("header")
        .and_then(Value::as_object)
        .expect("an emission carries its header");
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("channel_node".into(), json!(node));
    if let Some(call) = hop.get("call_id") {
        ctx.insert("call_id".into(), call.clone());
    }
    let mut out = meclaw_core::serde_json::Map::new();
    for key in ["call_id", "engine", "session_id", "final"] {
        if let Some(v) = hop.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    out.insert("route".into(), json!("in_call_end"));
    MessageBuilder::new(Path::new("/"))
        .hop(out)
        .context(ctx)
        .body(Body::Inline(json!({"messages": []})))
        .ttl(64)
        .build()
}

/// The capacity of the colony inbox the fixture's cell dead-letters into
/// (GH #1016): what a cell can hand over before a reader takes one.
pub const DEAD_LETTER_INBOX: usize = 64;

/// Read the cell's dead-letter inbox the way the colony reads its own: for as
/// long as the cell runs, never waiting on anyone. Everything taken out is
/// kept, in order, on the returned receiver, which the test reads when it
/// likes (`Live::dead_letters`).
///
/// Why (GH #1511): the cell sends a late answer into this
/// inbox with `send().await` (`VoiceCell::dead_letter_late`), and in a colony
/// that send only waits for the colony loop to take the next message -- the
/// loop reads its inbox until it stops and hands nothing to a cell with an
/// `await` (`try_send` into mailboxes, GH #1068). A fixture that left the
/// inbox unread parked the handler on the 65th late command for good:
/// `gh_d54g_weblive_a_call_ends_when_its_delivery_gives_up` hung 1 in 100
/// runs under load, and `gh_d54g_weblive_a_call_given_up_dead_letters_every_late_command`
/// sends twice the capacity plus one and hangs every time without this reader.
pub fn drain_dead_letters(
    mut inbox: mpsc::Receiver<meclaw_colony::ColonyMsg>,
) -> (
    mpsc::UnboundedReceiver<meclaw_colony::ColonyMsg>,
    tokio::task::JoinHandle<()>,
) {
    let (kept_tx, kept) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(letter) = inbox.recv().await {
            // A test that dropped its receiver reads nothing more; the inbox
            // is still emptied, as the colony empties it.
            let _ = kept_tx.send(letter);
        }
    });
    (kept, task)
}
