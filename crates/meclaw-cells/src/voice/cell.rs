//! The `voice` cell: handler half.
//!
//! The handler owns the per-session state and the turn state machine, and it is
//! the only half that emits. Audio never reaches it — a sample that entered a
//! mailbox would be a sample that waits behind a message.
//!
//! This file carries the two channel vocabularies of the dual task. The logic
//! that speaks them arrives with step 4 of strand t1.

use crate::voice::connection::APPEND_MAX_CHARS;
use crate::voice::contract::{AppendKind, DuplexEvent, Speaker, SttEvent};
use crate::voice::io::{VoiceIo, run_io};
use crate::voice::live_turns::{self, TurnAction, TurnInput, TurnState};
use crate::voice::params::{VoiceOverlay, VoiceParams};
use crate::voice::turns::{self, Action, Input, SessionState};
use crate::voice::wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason, WireErrorCode};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellOutput, Message, OriginSink, OutputSink, Path, Uuid};
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// What the I/O half tells the handler.
#[derive(Debug)]
pub enum VoiceEvent {
    /// The mount name is taken or malformed; nothing registered under it.
    ///
    /// The cell stays alive and is simply reachable by nobody — A1′ forbids the
    /// I/O half a voluntary return — and a `params` update naming a free name
    /// is served by this same task on the next life (ruling O-P-2).
    MountFailed(String),
    /// A client connected; `mode` already reflects `?mode=` or the default.
    Connected {
        /// The session this connection claimed.
        session_id: String,
        /// The mode this connection starts in.
        mode: Mode,
    },
    /// A client went away.
    ///
    /// **Ordering the handler relies on**: when a second connection claims a
    /// live session, the I/O half sends `Disconnected` for the connection it
    /// drops (close 4409) BEFORE `Connected` for the one that took its place.
    /// The handler keys sessions by `session_id`, so the other order would have
    /// the departure of the old connection delete the state of the new one, and
    /// a caller who reconnected would then be told `unknown_session`.
    Disconnected {
        /// The session whose connection is gone.
        session_id: String,
    },
    /// A control frame from the client.
    Control {
        /// The session the frame arrived on.
        session_id: String,
        /// The frame itself.
        frame: ClientFrame,
    },
    /// The STT session of this connection said something.
    Stt {
        /// The session the provider event belongs to.
        session_id: String,
        /// What the provider reported.
        event: SttEvent,
    },
    /// A synthesis ended (the I/O half ran it).
    SpeakEnded {
        /// The session that was being spoken to.
        session_id: String,
        /// The synthesis that ended.
        speak_id: String,
        /// Why it ended.
        reason: SpeakEndReason,
        /// Cause, on `failed`.
        detail: Option<String>,
    },
    /// The grace a `release` armed has run out; the handler cuts the turn that
    /// was draining, unless the provider already closed it.
    ///
    /// The handler has no clock of its own — it is a mailbox loop — so the wait
    /// lives in the half that already owns time and sockets, and comes back as
    /// an event like everything else the outside world does.
    ReleaseGraceExpired {
        /// The session whose boundary was draining.
        session_id: String,
        /// The generation this timer was armed for; the handler drops anything
        /// older, because that turn has already closed.
        token: u64,
    },
    /// A client stopped taking what was queued for it, and the count said so
    /// before any clock did (GH #601).
    ///
    /// The socket side of this cell is not backpressure. The colony side is: a
    /// listener that falls behind stalls the sender and loses nothing. But a
    /// WebSocket client that stops reading cannot be waited on for ever without
    /// taking the listener down with it (GH #593), so once `DISPATCH_QUEUE`
    /// commands are queued behind an already full connection channel the
    /// connection is given up on. That verdict used to be a log line and a
    /// `Disconnected`; it is a message now, so a colony learns why a call ended
    /// from its own lanes rather than from an operator's terminal.
    ClientTooSlow {
        /// The session whose client stopped taking commands.
        session_id: String,
        /// How many queued commands were given up on with it: everything the
        /// dispatch queue still held, plus the one that no longer fitted.
        dropped: usize,
    },
    /// A duplex session said something (contract § 1.1).
    ///
    /// The duplex counterpart of [`Self::Stt`], and one variant rather than
    /// ten: the provider enum already carries the vocabulary, and a second
    /// copy of it here would be a second place to forget a case.
    Live {
        /// The session the provider event belongs to.
        session_id: String,
        /// What the model reported.
        event: DuplexEvent,
    },
    /// Time passed on a duplex session and nobody said anything (R-L7).
    ///
    /// The clock of a duplex session, minted by the CONNECTION — it is the half
    /// that holds a `select!` and therefore the only half that can wake up on
    /// its own. It carries two deadlines: rule 1 of R-25-9, which closes a user
    /// turn `turn_gap_ms` after the caller fell quiet, and the delegation grace
    /// of R-L9.
    ///
    /// **A watchdog timer is not polling** (owner ruling, 2026-09-21): a
    /// watchdog timer is not classic polling, it is a timeout timer.
    /// Nothing is asked here and no state is read; a deadline goes off, and the
    /// event-driven rule (`docs/development-rules.md`, EDA+ES) forbids asking
    /// something repeatedly whether it has changed, not owning a clock. Real
    /// time semantics stay legitimate, and a turn boundary IS real time.
    ///
    /// Until R-L7 this rode on `DuplexEvent::Usage`, the provider's running
    /// meter (OR-L8). Measured, that does not hold: a session fed nothing but
    /// silence saw no meter at all inside 45 s over four runs (GH #798) — which
    /// is exactly the quiet line the tick exists for.
    LiveTick {
        /// The session whose clock this is.
        session_id: String,
        /// Where that clock stands, in milliseconds since the session opened.
        ///
        /// The connection's own elapsed time, which is the model's timeline to
        /// within the lag between the two: S0's `a-pacing.json` has the local
        /// clock 626 ms ahead of the model's `offset_ms` after 64 s, and both
        /// deadlines this feeds are measured in seconds.
        now_ms: u64,
    },
    /// The duplex provider of this connection gave up, and with it the call.
    ///
    /// **There is no reconnect** (OR-L20): the session IS the conversation, and
    /// a fresh socket would be a fresh conversation with no memory of this one.
    /// So this reaches the error lane as `duplex_failed` and the client reads a
    /// close.
    DuplexFailed {
        /// The session whose provider is gone.
        session_id: String,
        /// Human-readable cause. Never carries a credential.
        detail: String,
    },
    /// A binary frame of wrong length arrived. The I/O half **dropped the
    /// frame and kept the connection** (R-V6'): a client that mis-frames one
    /// buffer has a bug, not bad intent, and closing the socket would end a
    /// live call over a lost 20 ms.
    BadAudioFrame {
        /// The session that sent it.
        session_id: String,
        /// The length that was not whole sample frames.
        len: usize,
        /// How many such frames this connection has sent, counting this one.
        /// The I/O half keeps the counter; a systematic mis-framing is visible
        /// as a number that climbs, which needs no threshold to justify.
        count: u32,
    },
}

/// What the handler tells the I/O half.
#[derive(Debug)]
pub enum VoiceReconfig {
    /// Send a text frame to one session's client.
    ToClient {
        /// The session to address.
        session_id: String,
        /// The frame to send.
        frame: ServerFrame,
    },
    /// Start a synthesis; the I/O half runs the provider and streams audio.
    Speak {
        /// The session to speak to.
        session_id: String,
        /// The identity of this synthesis.
        speak_id: String,
        /// What to say.
        text: String,
    },
    /// Cancel the running synthesis of this session (I/O half answers with
    /// `SpeakEnded` / `cancelled`).
    CancelSpeak {
        /// The session whose synthesis is to stop.
        session_id: String,
    },
    /// Sleep `ms` for this session and answer with
    /// [`VoiceEvent::ReleaseGraceExpired`] carrying `token`.
    ///
    /// Armed on every `release` that starts a drain, and never cancelled: a
    /// timer for a boundary that has since closed answers with a token the
    /// session has already moved past, and the handler drops it. Cancelling
    /// would need a handle per session in a half that has no business knowing
    /// what a turn is.
    ArmReleaseGrace {
        /// The session whose boundary is draining.
        session_id: String,
        /// How long to wait.
        ms: u64,
        /// The generation to report back.
        token: u64,
    },
    /// Push one piece of guidance into a session's duplex model.
    ///
    /// The duplex counterpart of [`Self::Speak`]. A synthesis is text that will
    /// be read out; an append is guidance the model takes up in its own words
    /// (R-25-4). The `speak_id` is what makes the second kind audible as a
    /// section the client is told about — see [`crate::voice::io::ToConnection::Advise`].
    Advise {
        /// The session to advise.
        session_id: String,
        /// Which append channel it travels on.
        kind: AppendKind,
        /// The identity the provider's `appended` answer will carry.
        event_id: String,
        /// The delegation this answers, where it answers one.
        delegation_id: Option<String>,
        /// The text itself.
        content: String,
        /// Set where this append opens a spoken section (OR-L19).
        speak_id: Option<String>,
    },
    /// Close this session's connection with a code.
    Close {
        /// The session to close.
        session_id: String,
        /// The WebSocket close code, e.g. [`crate::voice::wire::CLOSE_SESSION_REPLACED`].
        code: u16,
    },
}

/// What the handler remembers about one live duplex session.
///
/// The counterpart of [`SessionState`], and deliberately not the same type:
/// a cascade session has a hold, a boundary phase, a release grace and a speak
/// queue, and a duplex session has none of the four. What it has instead is a
/// turn machine that runs off the model's clock, the delegations the model
/// opened, and the last meter reading — which rides out on the `turn` lane
/// rather than on a `session` lane of its own (OR-L22).
#[derive(Debug)]
pub struct LiveSessionState {
    /// Turn formation on the model's timeline (R-25-9).
    pub turns: TurnState,
    /// Delegations the model opened and this colony has not answered, with
    /// the moment each one opened on the model's clock (R-L9).
    ///
    /// A list rather than a map: a call carries none or one of these, two on a
    /// bad day, and the order they opened in is the order the deadline should
    /// take them in.
    pub open_delegations: Vec<OpenDelegation>,
    /// How full the model's context window was, last it said.
    pub last_usage_ratio: Option<f64>,
    /// The `event_id` of the append whose spoken section is open, if one is.
    pub speak_open: Option<String>,
    /// The mode this connection started in, for the hop of its emissions.
    ///
    /// Carried here because a duplex session has no [`SessionState`] and
    /// [`VoiceCell::mode_of`] reads that table. A `mode` frame on a live socket
    /// does NOT move it: the connection acts on such a frame itself (the
    /// model's ear), and it reaches no turn machine, so the handler never hears
    /// about it (OR-L.L2b.4).
    pub mode: Mode,
    /// How many turns this session has closed — the index the OPEN turn will
    /// carry, and with it the `turn_id` a `spoken` frame belongs to.
    ///
    /// [`TurnState`] counts the same thing and keeps it private; this is the
    /// handler's own copy, moved by the one action that closes a turn.
    pub closed_turns: u64,
}

/// One delegation the model opened and nobody has answered yet (R-L9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDelegation {
    /// What the provider calls it — the id an answering append has to carry.
    pub id: String,
    /// Where it opened on the model's clock, in milliseconds.
    ///
    /// `session.delegation.created` carries this as `offset_ms`, which is the
    /// same timeline [`VoiceEvent::LiveTick`] measures against: S0's
    /// `a-pacing.json` has the connection's own clock 626 ms ahead of it after
    /// 64 s, and the grace this feeds is measured in seconds.
    pub opened_at_ms: u64,
}

impl LiveSessionState {
    /// A session that has heard nothing yet.
    pub fn new(mode: Mode) -> Self {
        Self {
            turns: TurnState::new(),
            open_delegations: Vec::new(),
            last_usage_ratio: None,
            speak_open: None,
            mode,
            closed_turns: 0,
        }
    }

    /// `"<session_id>#<n>"` of the turn that is open now.
    pub fn open_turn_id(&self, session_id: &str) -> String {
        format!("{session_id}#{}", self.closed_turns)
    }
}

/// The handler half of a `voice` cell.
///
/// Holds one [`SessionState`] per live connection and is the only half that
/// emits. It never sees a sample: audio goes from the socket into the provider
/// and back out again inside [`crate::voice::io`], because a mailbox is a queue
/// and speech is a deadline.
pub struct VoiceCell {
    /// This cell's own absolute path — the target of its source emissions. The
    /// out-edges of the level decide where they go from there; the cell knows
    /// no topology.
    path: Path,
    /// The I/O half, taken once per spawn by `split_io`.
    io: Option<VoiceIo>,
    /// Live sessions, keyed by `session_id`. Owned by this task alone: no lock,
    /// because nothing is shared.
    ///
    /// **Empty in duplex mode**: there the sessions live in
    /// [`Self::live_sessions`], because the two have nothing in common but the
    /// key. The handler knows which of the two it is from its params
    /// ([`Self::duplex`]), which cannot change at runtime.
    sessions: HashMap<String, SessionState>,
    /// Live duplex sessions, keyed by `session_id`. See [`Self::sessions`].
    live_sessions: HashMap<String, LiveSessionState>,
    /// Whether this cell runs a duplex provider (`params.duplex`).
    ///
    /// Settled at birth: the block is not in
    /// [`crate::voice::params::VoiceOverlay::KNOWN_KEYS`], so a cell cannot
    /// change engines under a live call.
    duplex: bool,
    /// How long the model's timeline may stay quiet before a user turn closes,
    /// in milliseconds; only read in duplex mode (R-25-9).
    duplex_turn_gap_ms: u64,
    /// How long an interjection may be and still be a backchannel, in
    /// milliseconds; only read in duplex mode (R-25-9, OR-L18).
    duplex_backchannel_max_ms: u64,
    /// How long a delegation may stay unanswered before this cell closes it
    /// itself, in milliseconds; only read in duplex mode (R-L9, GH #793).
    duplex_delegation_grace_ms: u64,
    /// What it says when it does. An instruction, not a script: the model
    /// takes an append up in its own words (R-25-4).
    duplex_delegation_fallback: String,
    /// The name the I/O half registered on the colony's one listener — the
    /// cell's only door.
    ///
    /// Mutable, and carried here so an update persists it — but a new name
    /// takes effect on the **next life** (ruling O-P-2). The registration
    /// happens once, at the top of the I/O half's life, and this cell has no
    /// command that moves a live one: a mount that changed under an open link
    /// would leave the client holding a door that no longer exists, and the
    /// respawn is the moment both halves agree on the new name anyway.
    mount: String,
    /// The mode a new connection starts in.
    default_mode: Mode,
    /// Whether speech cancels a running synthesis.
    barge_in: bool,
    /// Whether interim transcripts reach the `partial` lane.
    emit_partials: bool,
    /// Whether the end of a synthesis reaches the `speak_end` lane.
    emit_speak_end: bool,
    /// Operation-timeout for I/O this cell initiates.
    external_timeout_ms: u64,
    /// Idle deadline per provider socket. Carried so a params update can move
    /// it and a respawn can replay it.
    provider_idle_timeout_ms: u64,
    /// Outbound frame length in milliseconds. Carried for the same reason as
    /// the idle deadline: the I/O half reads it at birth, and this is where an
    /// update keeps it until the next life.
    audio_out_frame_ms: u32,
    /// Whether an assistant turn is turned into speech text before it is
    /// queued. Read on every `in_speak`, so an update is in force at once.
    speak_plain: bool,
    /// How long a released `hold` boundary waits for the provider's own end of
    /// turn before it cuts with what it has. Handed to every session at birth
    /// and to the live ones on a params update.
    release_grace_ms: u64,
    /// The highest release-grace generation this cell has ever handed out.
    ///
    /// A session identity is **reusable**: a client that reconnects with the
    /// same `?session=` — or displaces its own connection with close 4409 —
    /// gets a fresh [`SessionState`], and a per-session counter that started at
    /// zero again would hand the new life the very numbers the old life's
    /// timers are still asleep on. Within `release_grace_ms` of a reconnect,
    /// one of those would cut the NEW boundary. So the generations are minted
    /// here, monotonically, across every session this cell serves: a token from
    /// a life that has ended is a number no later life ever uses.
    grace_seq: u64,
    /// The `stt` params sub-object verbatim, for the overlay merge base.
    stt_raw: Value,
    /// The `tts` params sub-object verbatim, for the overlay merge base.
    tts_raw: Option<Value>,
    /// The `duplex` params sub-object verbatim, for the overlay merge base.
    duplex_raw: Option<Value>,
    /// The command seam to the I/O half.
    ///
    /// The cell mints this pair itself rather than using the substrate's
    /// reconfig channel, because `handle_event` — where most of this cell's
    /// commands come from — is handed no sender at all. The `web` cell has the
    /// same split for the same reason (its `push` channel beside the reconfig
    /// one), and the receiving end lives in [`VoiceIo::from_handler`].
    to_io: mpsc::Sender<VoiceReconfig>,
}

impl VoiceCell {
    /// Build the handler half from the effective params (birth params with the
    /// `cell.db` overlay replayed over them) and the I/O half it will hand off.
    pub fn new(path: Path, mut io: VoiceIo, params: &VoiceParams, raw: &Value) -> Self {
        // Bounded: a handler that outruns the socket writer has to wait, which
        // is what backpressure is for. Unbounded here would turn a stalled
        // client into growing memory in the one task that must not stall.
        let (to_io, from_handler) = mpsc::channel(64);
        io.from_handler = Some(from_handler);
        // The declaration is set where the two halves are joined, not in the
        // factory: `speak_plain` is the handler's behaviour, and `hello` /
        // `GET /info` only REPORT it. Setting it here means the two cannot
        // disagree at birth, whoever built the I/O half.
        io.speak_plain = params.speak_plain;
        io.release_grace_ms = params.release_grace_ms;
        Self {
            to_io,
            path,
            io: Some(io),
            sessions: HashMap::new(),
            live_sessions: HashMap::new(),
            duplex: params.duplex.is_some(),
            duplex_turn_gap_ms: duplex_gap_ms(params),
            duplex_backchannel_max_ms: duplex_backchannel_ms(params),
            duplex_delegation_grace_ms: duplex_delegation_grace_ms(params),
            duplex_delegation_fallback: duplex_delegation_fallback(params),
            mount: params.mount.clone(),
            default_mode: params.default_mode,
            barge_in: params.barge_in,
            emit_partials: params.emit_partials,
            emit_speak_end: params.emit_speak_end,
            external_timeout_ms: params.external_timeout_ms,
            provider_idle_timeout_ms: params.provider_idle_timeout_ms,
            audio_out_frame_ms: params.audio_out_frame_ms,
            speak_plain: params.speak_plain,
            release_grace_ms: params.release_grace_ms,
            grace_seq: 0,
            stt_raw: raw.get("stt").cloned().unwrap_or(Value::Null),
            tts_raw: raw.get("tts").cloned(),
            duplex_raw: raw.get("duplex").filter(|v| !v.is_null()).cloned(),
        }
    }

    /// The overlay view of the current params — the merge base of a runtime
    /// params update.
    fn overlay(&self) -> VoiceOverlay {
        VoiceOverlay {
            mount: self.mount.clone(),
            default_mode: self.default_mode,
            barge_in: self.barge_in,
            emit_partials: self.emit_partials,
            emit_speak_end: self.emit_speak_end,
            external_timeout_ms: self.external_timeout_ms,
            provider_idle_timeout_ms: self.provider_idle_timeout_ms,
            audio_out_frame_ms: self.audio_out_frame_ms,
            speak_plain: self.speak_plain,
            release_grace_ms: self.release_grace_ms,
            stt: self.stt_raw.clone(),
            tts: self.tts_raw.clone(),
            duplex: self.duplex_raw.clone(),
        }
    }

    /// Carry out the half of an action that belongs to the I/O half, and hand
    /// back the half that belongs to the topology.
    ///
    /// Split out because the two paths into the machine have different sinks:
    /// an event arrives with an `OriginSink` and can emit, a mailbox message
    /// arrives with an `OutputSink` and cannot. Both have to execute the WHOLE
    /// action list — dispatching only the variant a caller expects is how an
    /// action nobody thought could occur goes missing in silence.
    async fn dispatch(&self, session_id: &str, action: Action) -> Option<Action> {
        match action {
            Action::ToClient(frame) => {
                let _ = self
                    .to_io
                    .send(VoiceReconfig::ToClient {
                        session_id: session_id.to_string(),
                        frame,
                    })
                    .await;
                None
            }
            Action::StartSpeak { speak_id, text } => {
                let _ = self
                    .to_io
                    .send(VoiceReconfig::Speak {
                        session_id: session_id.to_string(),
                        speak_id,
                        text,
                    })
                    .await;
                None
            }
            Action::CancelSpeak => {
                let _ = self
                    .to_io
                    .send(VoiceReconfig::CancelSpeak {
                        session_id: session_id.to_string(),
                    })
                    .await;
                None
            }
            lane => Some(lane),
        }
    }

    /// Turn the state machine's verdict into emissions and reconfig messages.
    ///
    /// The order is the order the actions came in: a `partial` that reached the
    /// client before the lane, or a turn that reached the lane before the
    /// client, would be a different conversation on each side.
    async fn run_actions(&self, session_id: &str, actions: Vec<Action>, sink: &OriginSink) {
        for action in actions {
            let Some(lane) = self.dispatch(session_id, action).await else {
                continue;
            };
            let mode = self.mode_of(session_id);
            let content = match lane {
                Action::EmitPartial { text, eager } => {
                    transcript_body("partial", session_id, mode, &text, json!({"eager": eager}))
                }
                Action::EmitTurn { text, turn_id } => {
                    transcript_body("turn", session_id, mode, &text, json!({"turn_id": turn_id}))
                }
                // `dispatch` handled every other variant.
                _ => continue,
            };
            self.emit(sink, content).await;
        }
    }

    /// Feed one input to one session's state machine and carry out its verdict.
    /// An input for a session nobody holds is dropped: the connection is gone,
    /// and there is nothing left to be right about.
    async fn drive(&mut self, session_id: &str, input: Input, sink: &OriginSink) {
        let Some(state) = self.sessions.get_mut(session_id) else {
            return;
        };
        let generation = state.grace_token;
        let actions = turns::step(state, session_id, input);
        // Every closed boundary produces exactly one `Turn` frame, and the empty
        // one produces no lane emission at all (`turns::turn_actions`). An empty
        // turn is a take that went missing somewhere, so it is LOUD: it is the
        // only signal the 16:28:37 incident would have left behind (GH #697).
        for action in &actions {
            if let Action::ToClient(ServerFrame::Turn { text, turn_id }) = action {
                if text.is_empty() {
                    tracing::warn!(
                        %session_id, %turn_id,
                        "voice: a boundary closed with nothing in it"
                    );
                } else {
                    tracing::info!(
                        %session_id, %turn_id, chars = text.chars().count(),
                        "voice: a boundary closed"
                    );
                }
            }
        }
        // A boundary that just started draining owes itself a deadline: a
        // provider that never reports the end of the audio it was given would
        // otherwise hold a turn open for the rest of the call. The state
        // machine says so by moving the generation while leaving the boundary
        // open and draining — which is exactly one input, `release`, and never
        // a close (a close moves the generation too, and takes the boundary).
        let arm = match &state.hold {
            Some(hold) if hold.draining && state.grace_token != generation => {
                Some((state.release_grace_ms, state.grace_token))
            }
            _ => None,
        };
        // Whatever the machine moved the generation to is now spoken for, cell
        // wide: the next session to be born starts above it, so the identity
        // being reusable cannot make two lives share a number.
        let reached = state.grace_token;
        self.observe_grace_generation(reached);
        self.run_actions(session_id, actions, sink).await;
        if let Some((ms, token)) = arm {
            let _ = self
                .to_io
                .send(VoiceReconfig::ArmReleaseGrace {
                    session_id: session_id.to_string(),
                    ms,
                    token,
                })
                .await;
        }
    }

    /// The generation a session born now starts on: one above everything this
    /// cell has ever handed out.
    fn next_grace_generation(&mut self) -> u64 {
        // `wrapping_add` for the same reason the machine's own counter uses it:
        // a generation is compared, never ordered against infinity, and a cell
        // that served 2^64 connections should not be the one place this panics.
        self.grace_seq = self.grace_seq.wrapping_add(1);
        self.grace_seq
    }

    /// Remember a generation one of this cell's sessions has reached, so the
    /// next session born starts above it.
    fn observe_grace_generation(&mut self, token: u64) {
        self.grace_seq = self.grace_seq.max(token);
    }

    /// The mode a session is in, for the hop of its emissions.
    fn mode_of(&self, session_id: &str) -> Mode {
        self.sessions
            .get(session_id)
            .map(|s| s.mode)
            .unwrap_or(self.default_mode)
    }

    /// Emit a source emission at this cell's own path.
    async fn emit(&self, sink: &OriginSink, content: Value) {
        let _ = sink
            .emit(CellOutput {
                target: self.path.clone(),
                content,
            })
            .await;
    }

    /// Stamp `hop.engine` on an emission of a duplex cell.
    ///
    /// Every duplex emission carries it and a cascade emission carries nothing
    /// — the KEY is absent rather than set to a second name (contract § 1.4).
    /// That is what lets a member's firewall edge pass the engine through with
    /// `has(hop.engine) ? hop.engine : ''` and a colony wired against an older
    /// `voice` go on working untouched.
    fn stamp_engine(&self, header: &mut Map<String, Value>) {
        if self.duplex {
            header.insert("engine".into(), json!("duplex"));
        }
    }

    /// Send one server frame to a session's client.
    async fn to_client(&self, session_id: &str, frame: ServerFrame) {
        let _ = self
            .to_io
            .send(VoiceReconfig::ToClient {
                session_id: session_id.to_string(),
                frame,
            })
            .await;
    }

    /// The body of a duplex emission (contract § 1.4).
    ///
    /// The same shape as [`transcript_body`], with two differences the lanes of
    /// a live session need: the `messages[]` are given rather than built from
    /// one string — a `turn` carries the caller's words AND the model's — and
    /// the hop is stamped with the engine.
    fn live_body(&self, route: &str, session_id: &str, extra: Value, messages: Value) -> Value {
        let mut header = Map::new();
        header.insert("route".into(), json!(route));
        header.insert("session_id".into(), json!(session_id));
        header.insert("call_id".into(), json!(session_id));
        header.insert("platform".into(), json!("voice"));
        header.insert(
            "mode".into(),
            json!(match self.live_mode(session_id) {
                Mode::Auto => "auto",
                Mode::Hold => "hold",
            }),
        );
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                header.insert(k.clone(), v.clone());
            }
        }
        self.stamp_engine(&mut header);
        json!({
            "header": Value::Object(header),
            "messages": messages,
        })
    }

    /// The mode a duplex session is in, for the hop of its emissions.
    fn live_mode(&self, session_id: &str) -> Mode {
        self.live_sessions
            .get(session_id)
            .map(|s| s.mode)
            .unwrap_or(self.default_mode)
    }

    /// Everything one duplex provider event means for this cell.
    ///
    /// One arm per variant of [`DuplexEvent`], because the provider enum is the
    /// vocabulary and a second copy of it would be a second place to forget a
    /// case. The transcript fragments are the only ones that reach a state
    /// machine; the rest are a reading, a report or a line in the log.
    async fn on_live_event(&mut self, session_id: &str, event: DuplexEvent, sink: &OriginSink) {
        let gap = self.duplex_turn_gap_ms;
        let backchannel = self.duplex_backchannel_max_ms;
        match event {
            DuplexEvent::Started {
                session_id: provider_session,
            } => {
                tracing::debug!(
                    path = self.path.as_str(),
                    %session_id, %provider_session,
                    "voice: the duplex session is open"
                );
            }
            DuplexEvent::Transcript {
                speaker,
                delta,
                start_ms,
                end_ms,
            } => {
                let Some(state) = self.live_sessions.get_mut(session_id) else {
                    return;
                };
                let actions = live_turns::step(
                    &mut state.turns,
                    TurnInput::Fragment {
                        speaker,
                        text: delta,
                        start_ms,
                        end_ms,
                    },
                    gap,
                    backchannel,
                );
                self.run_live_actions(session_id, actions, sink).await;
            }
            DuplexEvent::DelegationCreated {
                delegation_id,
                offset_ms,
            } => {
                let Some(state) = self.live_sessions.get_mut(session_id) else {
                    return;
                };
                // The delegation event carries no task text at all: what the
                // backend needs is the sentence the caller was in the middle of
                // (contract § 1.4).
                let text = live_turns::open_user_text(&state.turns).unwrap_or_default();
                let turn_id = state.open_turn_id(session_id);
                state.open_delegations.push(OpenDelegation {
                    id: delegation_id.clone(),
                    opened_at_ms: offset_ms,
                });
                let usage_ratio = state.last_usage_ratio;
                let mut extra = json!({
                    "turn_id": turn_id,
                    "delegation_id": delegation_id,
                    "offset_ms": offset_ms,
                });
                if let Some(ratio) = usage_ratio
                    && let Some(obj) = extra.as_object_mut()
                {
                    obj.insert("usage_ratio".into(), json!(ratio));
                }
                let content = self.live_body(
                    "delegation",
                    session_id,
                    extra,
                    json!([{"origin": "user", "type": "text", "text": text}]),
                );
                self.emit(sink, content).await;
            }
            DuplexEvent::Usage {
                seconds,
                usage_ratio,
            } => {
                let Some(state) = self.live_sessions.get_mut(session_id) else {
                    return;
                };
                if usage_ratio.is_some() {
                    state.last_usage_ratio = usage_ratio;
                }
                // A READING, and nothing else (R-L7). It was the heartbeat of
                // the turn machine until 2026-09-21 (OR-L8), on the strength of
                // a documented 15 s period — and the period is real (nineteen
                // intervals of 14 999-15 005 ms in S0's `a-pacing.json`) while
                // the ARRIVAL is not: a session fed nothing but silence saw no
                // meter at all inside 45 s over four runs (GH #798), which is
                // exactly the quiet line a heartbeat was wanted for. The clock
                // is now the connection's own, `VoiceEvent::LiveTick`.
                //
                // What the meter still buys: `usage_ratio`, which rides out on
                // the `turn` lane because there is no session lane (OR-L22),
                // and the log line at the end of the call.
                tracing::trace!(
                    path = self.path.as_str(),
                    %session_id, seconds,
                    "voice: the model reported its meter"
                );
            }
            DuplexEvent::Appended {
                kind,
                event_id,
                start_ms,
                end_ms,
            } => {
                tracing::debug!(
                    path = self.path.as_str(),
                    %session_id, ?kind, %event_id, start_ms, end_ms,
                    "voice: the model took an append up"
                );
            }
            DuplexEvent::Muted | DuplexEvent::Unmuted => {
                tracing::debug!(
                    path = self.path.as_str(),
                    %session_id,
                    "voice: the model's ear changed state"
                );
            }
            // One item went wrong and the session carries on. Reported, never
            // swallowed: a colony that reads its own error lane learns that the
            // model dropped something without anybody watching a log.
            DuplexEvent::Warning { detail } => {
                self.emit_error(sink, "duplex_warning", &detail, Some(session_id), None)
                    .await;
            }
            DuplexEvent::Closed {
                reason,
                usage_seconds,
            } => {
                let actions = match self.live_sessions.get_mut(session_id) {
                    Some(state) => {
                        live_turns::step(&mut state.turns, TurnInput::Close, gap, backchannel)
                    }
                    None => Vec::new(),
                };
                self.run_live_actions(session_id, actions, sink).await;
                // There is no `session` lane (OR-L22): what a call cost is a
                // line in this cell's log and, for the telephone, the
                // `call_ended` the hive writes.
                tracing::info!(
                    path = self.path.as_str(),
                    %session_id, %reason, usage_seconds,
                    "voice: the duplex session is over"
                );
            }
        }
    }

    /// One tick of the session's own clock (R-L7, R-L9).
    ///
    /// **A watchdog timer is not polling** (owner ruling, 2026-09-21): a
    /// watchdog timer is not classic polling, it is a timeout timer.
    /// Nothing is asked here and nothing is read that could have answered by
    /// message; two deadlines go off, or they do not. The rule this does not
    /// touch (`docs/development-rules.md`, EDA+ES) forbids asking something
    /// repeatedly whether it has changed, and leaves real time semantics alone
    /// — a turn boundary and a grace period are real time.
    ///
    /// Two things ride on it, in this order: rule 1 of R-25-9, which closes a
    /// user turn `turn_gap_ms` after the caller fell quiet, and the delegation
    /// grace of R-L9. The turn goes first because the delegation's fallback is
    /// an append into the same conversation, and a turn that was already over
    /// should leave before something new is said into it.
    async fn on_live_tick(&mut self, session_id: &str, now_ms: u64, sink: &OriginSink) {
        let gap = self.duplex_turn_gap_ms;
        let backchannel = self.duplex_backchannel_max_ms;
        let Some(state) = self.live_sessions.get_mut(session_id) else {
            return;
        };
        let actions = live_turns::step(
            &mut state.turns,
            TurnInput::Tick { now_ms },
            gap,
            backchannel,
        );
        // Taken out of the list HERE, before a single append is sent, and by
        // the same pass that decided they were stale. A delegation that left
        // the list cannot be found by the next tick, which is what makes the
        // fallback fall once instead of once per tick (#793).
        let grace = self.duplex_delegation_grace_ms;
        let mut stale = Vec::new();
        state.open_delegations.retain(|open| {
            if now_ms.saturating_sub(open.opened_at_ms) > grace {
                stale.push(open.id.clone());
                false
            } else {
                true
            }
        });
        self.run_live_actions(session_id, actions, sink).await;
        for delegation_id in stale {
            // The same shape a `fact` travels in — `Commentary`, spoken,
            // carrying the delegation's own id — because that is the shape
            // measured being taken up and said out loud (S0 b4: appended after
            // 584-779 ms, paraphrased in all three runs; proof B-2, six of six).
            // What this buys the caller is the difference between a holding
            // sentence followed by 55-58 s of silence and an answer.
            tracing::info!(
                path = self.path.as_str(),
                %session_id, %delegation_id, grace_ms = grace,
                "voice: no answer for this delegation in time — closing it with the fallback"
            );
            let fallback = self.duplex_delegation_fallback.clone();
            self.push_advise(
                session_id,
                AppendKind::Commentary,
                Some(delegation_id),
                &fallback,
                true,
            )
            .await;
        }
    }

    /// Turn the turn machine's verdict into frames and emissions.
    ///
    /// The order is the order the actions came in, for the reason
    /// [`Self::run_actions`] gives: a `partial` that reached the client before
    /// the lane would be a different conversation on each side.
    async fn run_live_actions(
        &mut self,
        session_id: &str,
        actions: Vec<TurnAction>,
        sink: &OriginSink,
    ) {
        for action in actions {
            match action {
                TurnAction::Emit(turn) => {
                    let turn_id = format!("{session_id}#{}", turn.index);
                    if let Some(state) = self.live_sessions.get_mut(session_id) {
                        state.closed_turns = turn.index.saturating_add(1);
                    }
                    self.to_client(
                        session_id,
                        ServerFrame::Turn {
                            text: turn.user.clone(),
                            turn_id: turn_id.clone(),
                        },
                    )
                    .await;
                    let usage_ratio = self
                        .live_sessions
                        .get(session_id)
                        .and_then(|s| s.last_usage_ratio);
                    let mut extra = json!({
                        "turn_id": turn_id,
                        "happened_at": turn.happened_at_ms,
                    });
                    if let Some(ratio) = usage_ratio
                        && let Some(obj) = extra.as_object_mut()
                    {
                        obj.insert("usage_ratio".into(), json!(ratio));
                    }
                    // Both voices in one episode: the caller's words and, where
                    // the model answered inside the same turn, its own.
                    let mut messages =
                        vec![json!({"origin": "user", "type": "text", "text": turn.user})];
                    if let Some(assistant) = turn.assistant {
                        messages.push(
                            json!({"origin": "assistant", "type": "text", "text": assistant}),
                        );
                    }
                    let content = self.live_body("turn", session_id, extra, Value::Array(messages));
                    self.emit(sink, content).await;
                }
                TurnAction::Partial { speaker, text } => {
                    // The client frame always travels; the LANE is the ordered
                    // one (`emit_partials`), exactly as in the cascade.
                    let turn_id = self
                        .live_sessions
                        .get(session_id)
                        .map(|s| s.open_turn_id(session_id))
                        .unwrap_or_else(|| format!("{session_id}#0"));
                    let (route, frame, origin) = match speaker {
                        Speaker::User => (
                            "partial",
                            ServerFrame::Partial {
                                text: text.clone(),
                                eager: false,
                            },
                            "user",
                        ),
                        Speaker::Assistant => (
                            "spoken",
                            ServerFrame::Spoken {
                                text: text.clone(),
                                turn_id: turn_id.clone(),
                            },
                            "assistant",
                        ),
                    };
                    self.to_client(session_id, frame).await;
                    if !self.emit_partials {
                        continue;
                    }
                    let mut extra = json!({"eager": false, "turn_id": turn_id});
                    if speaker == Speaker::Assistant
                        && let Some(obj) = extra.as_object_mut()
                    {
                        obj.insert("speaker".into(), json!("assistant"));
                    }
                    let content = self.live_body(
                        route,
                        session_id,
                        extra,
                        json!([{"origin": origin, "type": "text", "text": text}]),
                    );
                    self.emit(sink, content).await;
                }
                // The caller talked over the model for longer than a
                // backchannel (OR-L18). The section that was being spoken is
                // cancelled, which is what the telephony hive turns into a
                // `uuid_break` — the barge-in path of today, with a new
                // trigger (contract § 2).
                TurnAction::BargeIn => {
                    if self.barge_in {
                        let _ = self
                            .to_io
                            .send(VoiceReconfig::CancelSpeak {
                                session_id: session_id.to_string(),
                            })
                            .await;
                    }
                }
            }
        }
    }

    /// The `in_advise` lane (contract § 1.5).
    ///
    /// Guidance, not speech: the model takes a section up in its own words and
    /// its own time (R-25-4). Which of the three append channels it travels on
    /// is the caller's `hop.section`, and the three are not three names for one
    /// thing — `fact` is what the caller should HEAR next, `context` is what
    /// the model should know without saying it, `correction` changes how it
    /// behaves for the rest of the session.
    async fn advise(&mut self, msg: &Message, body: &Value, reply_target: Path, sink: &OutputSink) {
        let section = msg.headers.hop.get("section").and_then(|v| v.as_str());
        let (kind, spoken) = match section {
            Some("fact") => (AppendKind::Commentary, true),
            Some("context") => (AppendKind::Thinking, false),
            Some("correction") => (AppendKind::Instructions, false),
            other => {
                let detail = format!(
                    "hop.section must be one of fact, context, correction; got {}",
                    other.unwrap_or("nothing")
                );
                self.refuse(sink, reply_target, "bad_section", &detail, None)
                    .await;
                return;
            }
        };
        let addressed = |key: &str| {
            msg.headers
                .context
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let Some(session_id) = addressed("call_id").or_else(|| addressed("session_id")) else {
            self.refuse(
                sink,
                reply_target,
                "missing_session",
                "context.call_id required, or context.session_id where a colony still \
                 addresses this cell by the older key",
                None,
            )
            .await;
            return;
        };
        // A cascade session has no append channel at all: there is a recogniser
        // and a voice behind it, and neither takes guidance up.
        if !self.duplex {
            self.refuse(
                sink,
                reply_target,
                "wrong_engine",
                "in_advise needs a duplex session; this cell runs a cascade",
                Some(&session_id),
            )
            .await;
            return;
        }
        if !self.live_sessions.contains_key(&session_id) {
            self.refuse(
                sink,
                reply_target,
                "unknown_session",
                "no live connection holds this session",
                Some(&session_id),
            )
            .await;
            return;
        }
        // WHERE THE WORDS ARE. Three slots, and the FIRST is the one the
        // producer actually writes: measured against the twin on 2026-09-21
        // (GH #797), thirteen delegations were answered and not one was spoken,
        // because `templates/talky/splitter/config.json` cuts the `sidecar`
        // block by top-level key and emits one message per section with an
        // EMPTY `messages[]` and the section's VALUE under `payload`:
        //
        //     out.append({"header": {"route": "sidecar", "section": key},
        //                 "messages": [], "section": key, "payload": payload})
        //
        // where `payload` is `sections[key]` — so `payload` IS the section, not
        // `{section: value}`. The body #797 measured,
        // `{"payload": {"fact": "…"}, "section": "fact"}`, is a model that
        // nested twice; `section_text` reads that one too, by name.
        // Payload first, because `hop.section` NAMED this slot and the two
        // others are the generic turn beside it; a sender that writes only an
        // assistant turn or a bare `text` writes no payload at all, so nothing
        // that worked before this reads differently. A slot that carries no
        // words is passed over rather than taken as the answer.
        let section_name = section_name_of(kind);
        let text = [
            body.get("payload")
                .and_then(|p| section_text(p, section_name)),
            body.get("messages")
                .and_then(|m| m.as_array())
                .and_then(|arr| arr.last())
                .and_then(|t| t.get("text"))
                .and_then(|v| v.as_str()),
            body.get("text").and_then(|v| v.as_str()),
        ]
        .into_iter()
        .flatten()
        .find(|candidate| !candidate.trim().is_empty())
        .unwrap_or_default()
        .to_string();
        if text.trim().is_empty() {
            // NOT a `debug!` line. This was one, and that is why an advise that
            // vanished read from outside like a model ignoring an append: at the
            // default level the last link of the delegation cycle went silent.
            // The refusal goes out on the `error` lane (contract § 1.4 lists the
            // code), which is where this cell can put it and NOT, today, where
            // the sender stands: L7b measured the lane running
            // `M/channels/voice -> M/channels -> M` and on to the display and
            // `/os`, with no edge back to the assistant that wrote the section.
            // What the refusal buys is therefore the message log and the screen
            // — a dropped advise leaves a record instead of nothing at all.
            let detail = format!(
                "section {section_name} arrived with no words; the text of an advise is \
                 payload (the section itself), messages[-1].text or text"
            );
            self.refuse(
                sink,
                reply_target,
                "bad_section",
                &detail,
                Some(&session_id),
            )
            .await;
            return;
        }
        let delegation_id = msg
            .headers
            .context
            .get("delegation_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // A delegation this colony has now answered is no longer open. Without
        // this the list would only ever grow, for the whole length of a call.
        if let Some(id) = delegation_id.as_ref()
            && let Some(state) = self.live_sessions.get_mut(&session_id)
        {
            state.open_delegations.retain(|open| open.id != *id);
        }
        self.push_advise(&session_id, kind, delegation_id, &text, spoken)
            .await;
    }

    /// An `in_speak` on a duplex session (OR-L6).
    ///
    /// It is an APPEND, not a synthesis: the model paraphrases what it is given
    /// rather than reading it out, and what the caller hears is the model's own
    /// voice mid-conversation instead of a second one talking over it.
    async fn speak_duplex(
        &mut self,
        session_id: &str,
        text: String,
        reply_target: Path,
        sink: &OutputSink,
    ) {
        if !self.live_sessions.contains_key(session_id) {
            self.refuse(
                sink,
                reply_target,
                "unknown_session",
                "no live connection holds this session",
                Some(session_id),
            )
            .await;
            return;
        }
        // Markdown becomes speech text here for the reason it does in the
        // cascade: what reaches the provider is what will be heard.
        let text = if self.speak_plain {
            crate::voice::speech_text::to_speech(&text)
        } else {
            text
        };
        if text.trim().is_empty() {
            tracing::debug!(
                path = self.path.as_str(),
                %session_id,
                "voice: nothing to speak — the answer has no words in it"
            );
            return;
        }
        self.push_advise(session_id, AppendKind::Commentary, None, &text, true)
            .await;
    }

    /// Hand one piece of guidance to the connection, split where the provider's
    /// ceiling demands it.
    ///
    /// Every part gets an `event_id` of its own, because the model answers each
    /// append separately. Only the FIRST carries the `speak_id`, and that
    /// `speak_id` IS its `event_id`: the connection recognises the model's own
    /// `appended` answer by that equality and starts the quiet clock of the
    /// spoken section on it (contract § 1.3).
    async fn push_advise(
        &mut self,
        session_id: &str,
        kind: AppendKind,
        delegation_id: Option<String>,
        text: &str,
        spoken: bool,
    ) {
        for (i, part) in split_append(text).into_iter().enumerate() {
            let event_id = Uuid::now_v7().to_string();
            let speak_id = (spoken && i == 0).then(|| event_id.clone());
            if let Some(id) = speak_id.as_ref()
                && let Some(state) = self.live_sessions.get_mut(session_id)
            {
                state.speak_open = Some(id.clone());
            }
            let _ = self
                .to_io
                .send(VoiceReconfig::Advise {
                    session_id: session_id.to_string(),
                    kind,
                    event_id,
                    delegation_id: delegation_id.clone(),
                    content: part,
                    speak_id,
                })
                .await;
        }
    }

    /// Announce the end of one synthesis on the `speak_end` lane.
    ///
    /// One emission per `Speak` this cell accepted, whatever ended it — the
    /// last chunk (`done`), a cancel or a barge-in (`cancelled`), a provider
    /// that gave up (`failed`). It carries no words: a colony that has to know
    /// when the sentence is over does not have to be told the sentence again,
    /// and the `turn` lane is where words live.
    ///
    /// It leaves whether or not the session is still held. A synthesis that
    /// ended because the connection went away is exactly the case a waiter
    /// downstream must not be left hanging on (the telephony hive holds a
    /// hang-up until this arrives), so this sits beside `drive` rather than
    /// inside it — `drive` drops an input for a session nobody holds.
    async fn emit_speak_end_lane(
        &self,
        sink: &OriginSink,
        session_id: &str,
        speak_id: &str,
        reason: SpeakEndReason,
    ) {
        let mut header = Map::new();
        header.insert("route".into(), json!("speak_end"));
        header.insert("session_id".into(), json!(session_id));
        header.insert("call_id".into(), json!(session_id));
        header.insert("speak_id".into(), json!(speak_id));
        header.insert("reason".into(), json!(reason));
        header.insert("platform".into(), json!("voice"));
        self.stamp_engine(&mut header);
        let content = json!({
            "header": Value::Object(header),
            "messages": [],
        });
        self.emit(sink, content).await;
    }

    /// Emit an error on the `error` lane. Never silent: every refusal this cell
    /// makes leaves it as a message, so a colony can be told what went wrong by
    /// something other than a log line nobody is reading.
    async fn emit_error(
        &self,
        sink: &OriginSink,
        code: &str,
        detail: &str,
        session: Option<&str>,
        extra: Option<Value>,
    ) {
        let mut header = Map::new();
        header.insert("route".into(), json!("error"));
        header.insert("error_code".into(), json!(code));
        header.insert("msg_type".into(), json!("voice_error"));
        if let Some(s) = session {
            header.insert("session_id".into(), json!(s));
            header.insert("call_id".into(), json!(s));
        }
        if let Some(obj) = extra.as_ref().and_then(|e| e.as_object()) {
            for (k, v) in obj {
                header.insert(k.clone(), v.clone());
            }
        }
        self.stamp_engine(&mut header);
        let content = json!({
            "header": Value::Object(header),
            "messages": [],
            "meta": {"detail": detail},
        });
        self.emit(sink, content).await;
    }

    /// Apply a runtime params update: merge, **move**, then persist — the
    /// `web` cell's order (GH #410), for the same reason. Nothing reaches
    /// `cell.db` that the socket did not accept, so a respawn cannot replay an
    /// address this cell was never on.
    async fn apply_params_update(
        &mut self,
        update: &Map<String, Value>,
        reply_target: Path,
        sink: &OutputSink,
        db: &mut DbConn,
    ) {
        let current = self.overlay();
        let (merged, overlay) = match crate::params_overlay::apply_update(&current, update) {
            Ok(ok) => ok,
            Err(e) => {
                self.refuse(sink, reply_target, "invalid_input", &e.detail(), None)
                    .await;
                return;
            }
        };

        // Written down, not acted on: the next life registers under it
        // (ruling O-P-2). Nothing about a surface moves during an update any
        // more — the cell owns no socket to move.
        self.mount = merged.mount.clone();

        let now = crate::params_overlay::now_unix_seconds();
        let persist = db
            .call(move |c| crate::params_overlay::persist_params_overlay(c, &overlay, now))
            .await;
        if let Err(e) = persist {
            self.refuse(
                sink,
                reply_target,
                "invalid_input",
                &format!(
                    "cell.db params write failed: {e} — the endpoint moved but a respawn \
                     will not remember it"
                ),
                None,
            )
            .await;
            return;
        }

        self.default_mode = merged.default_mode;
        self.barge_in = merged.barge_in;
        self.emit_partials = merged.emit_partials;
        self.emit_speak_end = merged.emit_speak_end;
        self.external_timeout_ms = merged.external_timeout_ms;
        self.provider_idle_timeout_ms = merged.provider_idle_timeout_ms;
        self.audio_out_frame_ms = merged.audio_out_frame_ms;
        self.speak_plain = merged.speak_plain;
        self.release_grace_ms = merged.release_grace_ms;
        db.set_query_timeout(Some(Duration::from_millis(self.external_timeout_ms)));

        // The three per-session settings reach the calls that are already up.
        // Turning barge-in off and having it stay on for whoever happens to be
        // talking is the kind of half-applied setting an operator debugs for an
        // hour. Only those three move: `mode`, the hold buffer, the boundary's
        // phase and the speak queue are the turn state of a live conversation,
        // and a params update is not an event in it. `default_mode` is
        // deliberately not pushed either — it names what a NEW connection
        // starts as.
        for session in self.sessions.values_mut() {
            session.barge_in = merged.barge_in;
            session.emit_partials = merged.emit_partials;
            // Read at the next `release`, so a boundary that is draining right
            // now finishes on the deadline it was armed with. Moving a timer
            // that is already asleep would be a turn cut by a number nobody
            // sent it with.
            session.release_grace_ms = merged.release_grace_ms;
        }
    }

    /// The refusal shape of a message this cell could not act on. `code` comes
    /// from the closed list in the spec: `invalid_body`, `missing_session`,
    /// `unknown_session`, `invalid_input`. Never silent — a caller that sent a
    /// voice cell something it could not say has to learn that nothing was said.
    ///
    /// `session` is present whenever the message named one (R-V15). The two
    /// refusals that go without it are the two that have nothing to name: a
    /// body this cell cannot read, and one that never said which call it meant.
    /// `unknown_session` is NOT one of those — it knows exactly which session
    /// it could not find, and a caller with two calls in flight needs to be
    /// told which of them ended.
    async fn refuse(
        &self,
        sink: &OutputSink,
        target: Path,
        code: &str,
        detail: &str,
        session: Option<&str>,
    ) {
        let mut header = Map::new();
        header.insert("route".into(), json!("error"));
        header.insert("error_code".into(), json!(code));
        header.insert("msg_type".into(), json!("voice_error"));
        if let Some(s) = session {
            header.insert("session_id".into(), json!(s));
            header.insert("call_id".into(), json!(s));
        }
        self.stamp_engine(&mut header);
        let content = json!({
            "header": Value::Object(header),
            "messages": [],
            "meta": {"detail": detail},
        });
        let _ = sink.push(CellOutput { target, content }).await;
    }
}

/// The name of a section, from the channel it was routed to. The match at the
/// top of `advise()` already left the function for every other name, so this
/// never invents one that was not on the wire.
fn section_name_of(kind: AppendKind) -> &'static str {
    match kind {
        AppendKind::Commentary => "fact",
        AppendKind::Thinking => "context",
        AppendKind::Instructions => "correction",
    }
}

/// A string that carries words, or nothing.
fn words(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.as_str()),
        _ => None,
    }
}

/// The words of one section, as the splitter hands it over (GH #797).
///
/// `payload` IS the section value the model wrote — `sections[key]` in
/// `templates/talky/splitter/config.json` — so a string here is the advice,
/// which is the shape talky's own offer asks for
/// (`templates/talky/schemas/config.json`: every section is a
/// `{"type": "string"}`). An OBJECT is a model that nested, which the block
/// contract INVITED until GH #799: its preamble showed `{"fact": {...}}` for
/// every section while the heading under it showed `{"fact": "<sentence>"}`.
/// The frame prints the form of each offer now
/// (`templates/collector/assemble/config.json`, `shape_slot`), so the nest is
/// no longer asked for — and it is still read here, because a model that
/// nested once is not a caller to punish. Since GH #799 the object is also
/// what the splitter itself hands over for a bare string — `{"payload": "…"}`,
/// the wrapper that lets the string ride the object slot the lane declares —
/// and the `payload` name below is what finds the sentence in it.
///
/// Inside an object the sentence is found by NAME and not by position: the
/// section's own key first (that is the double nest GH #797 measured), then
/// `text`, then `payload` — the wrapper the splitter itself writes around a
/// bare string, which is the form 17 of 22 sections arrived in when the twin
/// was measured on 2026-09-21. It is named rather than left to the guess
/// below, because it is not a shape a model invented: this cell's own producer
/// writes it, and a lane that carries three quarters of its traffic on a
/// fallback has no lock on the shape at all.
///
/// With none of the three names there, the first string in SORTED key order.
/// Sorting is written out rather than left to the map: `serde_json` is
/// a `BTreeMap` today only because nothing in the tree turns on
/// `preserve_order`, and a promise that a dependency can flip without touching
/// this cell is not a promise. One guess at the end, and it costs the caller
/// nothing against a dropped advise.
fn section_text<'a>(payload: &'a Value, section: &str) -> Option<&'a str> {
    match payload {
        Value::String(_) => words(payload),
        Value::Object(map) => map
            .get(section)
            .and_then(|v| section_text(v, section))
            .or_else(|| map.get("text").and_then(words))
            .or_else(|| map.get("payload").and_then(words))
            .or_else(|| {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                keys.into_iter().find_map(|k| map.get(k).and_then(words))
            }),
        _ => None,
    }
}

/// How long the model's timeline may stay quiet before a user turn closes, for
/// this cell. The cascade default where there is no duplex block: the number is
/// never read there, and a second `Option` in the handler would be a branch
/// with no second behaviour behind it.
fn duplex_gap_ms(params: &VoiceParams) -> u64 {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => p.turn_gap_ms,
        _ => crate::voice::params::DEFAULT_TURN_GAP_MS,
    }
}

/// The same for the backchannel ceiling (OR-L18).
fn duplex_backchannel_ms(params: &VoiceParams) -> u64 {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => p.backchannel_max_ms,
        _ => crate::voice::params::DEFAULT_BACKCHANNEL_MAX_MS,
    }
}

/// The same for the delegation deadline (R-L9).
fn duplex_delegation_grace_ms(params: &VoiceParams) -> u64 {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => p.delegation_grace_ms,
        _ => crate::voice::params::DEFAULT_DELEGATION_GRACE_MS,
    }
}

/// And for the sentence that closes one (R-L9).
fn duplex_delegation_fallback(params: &VoiceParams) -> String {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => p.delegation_fallback.clone(),
        _ => crate::voice::params::DEFAULT_DELEGATION_FALLBACK.to_string(),
    }
}

/// Cut one piece of guidance into appends the provider will take.
///
/// One append may carry about 500 tokens (contract § 1.5,
/// [`APPEND_MAX_CHARS`]), and a longer one is split at PARAGRAPH boundaries:
/// a section cut mid-sentence would be read out as two thoughts. A single
/// paragraph that is itself too long is cut at the last space before the
/// ceiling — the one place a word boundary has to do, because the alternative
/// is an append the model refuses whole.
fn split_append(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.chars().count() <= APPEND_MAX_CHARS {
        return vec![text.to_string()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        for piece in split_paragraph(paragraph) {
            let joined = current.chars().count() + 2 + piece.chars().count();
            if current.is_empty() {
                current = piece;
            } else if joined <= APPEND_MAX_CHARS {
                current.push_str("\n\n");
                current.push_str(&piece);
            } else {
                parts.push(std::mem::take(&mut current));
                current = piece;
            }
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// One paragraph, cut at word boundaries where it exceeds the ceiling alone.
fn split_paragraph(paragraph: &str) -> Vec<String> {
    if paragraph.chars().count() <= APPEND_MAX_CHARS {
        return vec![paragraph.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = paragraph;
    while rest.chars().count() > APPEND_MAX_CHARS {
        // The byte index just past the last character that still fits.
        let limit = rest
            .char_indices()
            .nth(APPEND_MAX_CHARS)
            .map_or(rest.len(), |(i, _)| i);
        let cut = rest[..limit].rfind(char::is_whitespace).unwrap_or(limit);
        let (head, tail) = rest.split_at(cut);
        out.push(head.trim().to_string());
        rest = tail.trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// The body of a `partial` or a `turn` emission.
fn transcript_body(route: &str, session_id: &str, mode: Mode, text: &str, extra: Value) -> Value {
    let mut header = Map::new();
    header.insert("route".into(), json!(route));
    header.insert("session_id".into(), json!(session_id));
    // The same value under the name a CHANNEL uses (GH #620). `session_id` is
    // also what a member's `session-keeper` mints for its own generation, so a
    // colony with two calls in flight needs a key that only ever means the
    // call.
    header.insert("call_id".into(), json!(session_id));
    header.insert("platform".into(), json!("voice"));
    header.insert(
        "mode".into(),
        json!(match mode {
            Mode::Auto => "auto",
            Mode::Hold => "hold",
        }),
    );
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            header.insert(k.clone(), v.clone());
        }
    }
    json!({
        "header": Value::Object(header),
        "messages": [{"origin": "user", "type": "text", "text": text}],
    })
}

impl LongRunningCell for VoiceCell {
    type Event = VoiceEvent;
    type Reconfig = VoiceReconfig;
    /// The `Option` is not decoration. `split_io` is called exactly once per
    /// spawn, and the other factories say so with an `expect` — a panic on the
    /// respawn path, where a panic takes the whole colony task rather than one
    /// cell. Making the emptiness part of the type says the same thing without
    /// arming that gun: a second call yields `None`, and `run_io` parks (A1′).
    type Io = Option<VoiceIo>;

    fn split_io(&mut self) -> Self::Io {
        self.io.take()
    }

    fn attach_liveness(io: &mut Self::Io, mark: meclaw_colony::io_liveness::IoLivenessMark) {
        if let Some(io) = io {
            io.liveness = mark;
        }
    }

    /// Hand the I/O half the sender the substrate minted for this spawn. The
    /// one it was built with reports nowhere the handler is listening — the
    /// receiving end of THIS channel is the one the handler loop selects on.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let Some(mut io) = io else {
                // Unreachable by the trait's own contract, and a park rather
                // than a return because a voluntary return from `run_io` while
                // the cell lives is the A1′ violation.
                std::future::pending::<()>().await;
                return;
            };
            io.events_tx = events_tx;
            run_io(io, reconfig_rx).await
        }
    }

    /// The inbound leg: `in_speak`, and the params slot.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            let Body::Inline(body) = &msg.body else {
                self.refuse(
                    sink,
                    reply_target,
                    "invalid_body",
                    "expected inline json",
                    None,
                )
                .await;
                return;
            };

            // The params slot first and exclusively: a message carrying it is
            // not something to say out loud.
            if let Some(params_val) = body.get("params") {
                match params_val.as_object() {
                    Some(update) => {
                        let update = update.clone();
                        self.apply_params_update(&update, reply_target, sink, db)
                            .await;
                    }
                    None => {
                        self.refuse(
                            sink,
                            reply_target,
                            "invalid_input",
                            "params slot: not a JSON object",
                            None,
                        )
                        .await;
                    }
                }
                return;
            }

            // The advise lane, before the text is read: its text may come out
            // of `body.text` rather than out of an assistant turn, and its
            // section decides which of the three append channels it takes
            // (contract § 1.5).
            if msg.headers.hop.get("route").and_then(|v| v.as_str()) == Some("in_advise") {
                self.advise(&msg, body, reply_target, sink).await;
                return;
            }

            let text = body
                .get("messages")
                .and_then(|m| m.as_array())
                .and_then(|arr| {
                    arr.iter().rev().find(|t| {
                        t.get("origin").and_then(|v| v.as_str()) == Some("assistant")
                            && t.get("type").and_then(|v| v.as_str()) == Some("text")
                    })
                })
                .and_then(|t| t.get("text"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let Some(text) = text else {
                self.refuse(
                    sink,
                    reply_target,
                    "invalid_body",
                    "messages[] has no assistant-text turn",
                    None,
                )
                .await;
                return;
            };

            // The call first, the session second (GH #620, wave ruling
            // R-0908-6). `context.session_id` has two owners — this cell
            // selects a connection by it, and a member's `session-keeper`
            // mints one of its own on the same key for every turn that passes
            // it — so a channel standing behind a keeper has to be able to say
            // which CALL it means. Reading `session_id` where the call key is
            // absent is what keeps a colony wired against an older `voice`
            // working: the key is ADDED, and nothing about the old one moves.
            let addressed = |key: &str| {
                msg.headers
                    .context
                    .get(key)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let Some(session_id) = addressed("call_id").or_else(|| addressed("session_id")) else {
                self.refuse(
                    sink,
                    reply_target,
                    "missing_session",
                    "context.call_id required, or context.session_id where a colony still \
                     addresses this cell by the older key (promote it on the voice cell's \
                     out-edge)",
                    None,
                )
                .await;
                return;
            };

            // An `in_speak` on a duplex cell is an APPEND, not a synthesis
            // (OR-L6): the model paraphrases what it is given rather than
            // reading it out. The sessions it addresses live in
            // `live_sessions`, so the cascade lookup below would answer
            // `unknown_session` for a call that is perfectly live.
            if self.duplex {
                self.speak_duplex(&session_id, text, reply_target, sink)
                    .await;
                return;
            }

            if !self.sessions.contains_key(&session_id) {
                self.refuse(
                    sink,
                    reply_target,
                    "unknown_session",
                    "no live connection holds this session",
                    Some(&session_id),
                )
                .await;
                return;
            }

            // Markdown becomes speech text HERE, before the queue: the queue
            // holds the text that will be synthesised, so rewriting after an
            // answer was queued would let a later `params` flip reach answers
            // that were accepted under the old one. Off by param, never by
            // accident (`speak_plain`).
            let text = if self.speak_plain {
                crate::voice::speech_text::to_speech(&text)
            } else {
                text
            };

            // An answer with no words in it — a horizontal rule, an empty
            // emphasis, a bare code fence, or an assistant turn that arrived
            // empty — is not queued. It is not a refusal either: nobody did
            // anything wrong, and answering `invalid_body` would have an agent
            // retry a turn that was fine. A synthesis of nothing costs a
            // provider call and gives a listener a `speak_start`/`speak_end`
            // pair around silence. Both shapes take the same path, so an
            // operator who finds one has found the other.
            if text.trim().is_empty() {
                tracing::debug!(
                    path = self.path.as_str(),
                    session_id = %session_id,
                    "voice: nothing to speak — the answer has no words in it"
                );
                return;
            }

            let speak_id = Uuid::now_v7().to_string();
            let Some(state) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let actions = turns::step(state, &session_id, Input::Speak { speak_id, text });
            for action in actions {
                // A speak order can only produce `StartSpeak` or nothing, and
                // the whole list goes through `dispatch` anyway: a lane action
                // here would mean the machine grew a path this leg has no sink
                // for, and that is worth a loud line rather than a silent drop.
                if let Some(lane) = self.dispatch(&session_id, action).await {
                    tracing::error!(
                        path = self.path.as_str(),
                        session_id = %session_id,
                        action = ?lane,
                        "voice: a speak order produced a lane action, which the mailbox \
                         leg has no origin sink for"
                    );
                }
            }
        }
    }

    /// The source leg: everything the I/O half saw.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                VoiceEvent::MountFailed(e) => {
                    // The cell stays alive and is simply not reachable under
                    // that name: a name collision must not look like a crash
                    // loop, and a params update naming a free one is served by
                    // this same task on the next life.
                    tracing::error!(
                        path = self.path.as_str(),
                        error = %e,
                        "voice: mount refused — send this cell a params update with a free mount"
                    );
                }
                VoiceEvent::Connected { session_id, mode } => {
                    // Two kinds of session, one event. Which one it is comes
                    // from the params rather than from the event, because the
                    // engine cannot change under a live cell.
                    if self.duplex {
                        self.live_sessions
                            .insert(session_id, LiveSessionState::new(mode));
                        return;
                    }
                    // A generation this cell has never handed out, so a timer
                    // armed by the connection this one replaces cannot cut the
                    // boundary of the connection that replaced it.
                    let generation = self.next_grace_generation();
                    let mut state = SessionState::new(
                        mode,
                        self.barge_in,
                        self.emit_partials,
                        self.release_grace_ms,
                    );
                    state.grace_token = generation;
                    self.sessions.insert(session_id, state);
                }
                VoiceEvent::Disconnected { session_id } => {
                    self.sessions.remove(&session_id);
                    self.live_sessions.remove(&session_id);
                }
                VoiceEvent::Control { session_id, frame } => {
                    self.drive(&session_id, Input::Control(frame), sink).await;
                }
                VoiceEvent::Stt { session_id, event } => {
                    // Both the fatal and the non-fatal provider message reach
                    // the error lane under the same code: a colony that wants
                    // to know its speech provider is unhappy should not have to
                    // subscribe twice, and the session outliving the second one
                    // is a fact about the session, not about the report.
                    let reportable = match &event {
                        SttEvent::Failed { detail } | SttEvent::Warning { detail } => {
                            Some(detail.clone())
                        }
                        _ => None,
                    };
                    if let Some(detail) = reportable {
                        self.emit_error(sink, "stt_failed", &detail, Some(&session_id), None)
                            .await;
                    }
                    self.drive(&session_id, Input::Stt(event), sink).await;
                }
                VoiceEvent::SpeakEnded {
                    session_id,
                    speak_id,
                    reason,
                    detail,
                } => {
                    if reason == SpeakEndReason::Failed {
                        let detail = detail.unwrap_or_else(|| "synthesis failed".to_string());
                        self.emit_error(sink, "speak_failed", &detail, Some(&session_id), None)
                            .await;
                    }
                    if self.emit_speak_end {
                        self.emit_speak_end_lane(sink, &session_id, &speak_id, reason)
                            .await;
                    }
                    // A duplex session has no cascade turn machine to tell —
                    // the section that ended was an append, not a synthesis in
                    // a queue — so the state it keeps is the one bit the
                    // handler owns: whether a section is still open.
                    if self.duplex {
                        if let Some(state) = self.live_sessions.get_mut(&session_id)
                            && state.speak_open.as_deref() == Some(speak_id.as_str())
                        {
                            state.speak_open = None;
                        }
                        return;
                    }
                    self.drive(&session_id, Input::SpeakEnded { speak_id, reason }, sink)
                        .await;
                }
                VoiceEvent::ReleaseGraceExpired { session_id, token } => {
                    self.drive(&session_id, Input::ReleaseGraceExpired { token }, sink)
                        .await;
                }
                VoiceEvent::ClientTooSlow {
                    session_id,
                    dropped,
                } => {
                    // The `Disconnected` right behind this one says the call is
                    // over; this says why, and it is the only place that does.
                    // A colony that reads its own error lane can tell a caller
                    // who hung up from a client that stopped reading, which is
                    // the difference between a person leaving and a bug.
                    let detail = format!(
                        "the client did not take {dropped} queued commands; \
                         the connection was given up"
                    );
                    self.emit_error(
                        sink,
                        "client_too_slow",
                        &detail,
                        Some(&session_id),
                        Some(json!({"dropped_frames": dropped})),
                    )
                    .await;
                }
                VoiceEvent::LiveTick { session_id, now_ms } => {
                    self.on_live_tick(&session_id, now_ms, sink).await;
                }
                VoiceEvent::Live { session_id, event } => {
                    self.on_live_event(&session_id, event, sink).await;
                }
                VoiceEvent::DuplexFailed { session_id, detail } => {
                    // The one duplex path that IS built here: a provider that
                    // is gone ends the call, and a call that ends without a
                    // word on the error lane is a call nobody can explain
                    // (OR-L20 — there is no reconnect to wait for).
                    self.emit_error(sink, "duplex_failed", &detail, Some(&session_id), None)
                        .await;
                }
                VoiceEvent::BadAudioFrame {
                    session_id,
                    len,
                    count,
                } => {
                    // The frame is gone and the call goes on (R-V6'). Both the
                    // client and the topology are told, and both are told how
                    // many times it has happened — the count is what separates
                    // one lost buffer from a client that has the sample format
                    // wrong, without this cell having to pick a threshold it
                    // could not defend.
                    let detail =
                        format!("binary frame of {len} bytes is not whole pcm_s16le sample frames");
                    self.emit_error(
                        sink,
                        "bad_audio_frame",
                        &detail,
                        Some(&session_id),
                        Some(json!({"bad_frames": count})),
                    )
                    .await;
                    let _ = self
                        .dispatch(
                            &session_id,
                            Action::ToClient(ServerFrame::Error {
                                code: WireErrorCode::BadAudioFrame,
                                detail,
                                bad_frames: Some(count),
                            }),
                        )
                        .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::contract::ProviderTimeouts;
    use crate::voice::params::VoiceParams;
    use crate::voice::providers::build_stt;
    use crate::voice::turns::SessionState;

    /// **A transcript names its call.** GH #620: `turn` and `partial` are the
    /// two lanes a colony reads a conversation off, and a phone hive with two
    /// calls in flight cannot attribute either of them without the call on the
    /// hop. Both come out of one builder, so both are measured here — the
    /// integration file pins the other two lanes, which need a live socket.
    #[test]
    fn a_transcript_names_the_call_it_belongs_to() {
        for route in ["turn", "partial"] {
            let body = transcript_body(route, "call-7", Mode::Auto, "hello", json!({}));
            let header = body
                .get("header")
                .and_then(|h| h.as_object())
                .expect("a transcript carries a header");
            assert_eq!(
                header.get("call_id"),
                Some(&json!("call-7")),
                "the `{route}` lane names the call: {body}"
            );
            assert_eq!(
                header.get("call_id"),
                header.get("session_id"),
                "one value under both names — a connection IS the session: {body}"
            );
        }
    }

    /// A cell with an I/O half that exists and is never driven.
    fn parked_cell(external_timeout_ms: u64) -> VoiceCell {
        let raw = json!({
            "mount": "voice",
            "stt": {"provider": "echo"},
            "external_timeout_ms": external_timeout_ms,
        });
        let params = VoiceParams::parse(&raw).expect("the echo config parses");
        let stt = build_stt(&params.stt, ProviderTimeouts::default()).expect("the echo provider");
        let (events_tx, _events_rx) = mpsc::channel(8);
        let io = VoiceIo::new(
            params.mount.clone(),
            stt,
            None,
            Mode::Auto,
            Duration::from_millis(params.external_timeout_ms),
            Duration::from_millis(params.provider_idle_timeout_ms),
            events_tx,
        );
        // `run_io` is never spawned, so the receiver inside `io` stays alive
        // and stays unread.
        VoiceCell::new(
            Path::new("/main/members/tester/channels/voice"),
            io,
            &params,
            &raw,
        )
    }

    /// A session identity outlives a connection, and a release grace outlives a
    /// hang-up.
    ///
    /// The defect this pins: the generation used to be minted by
    /// [`SessionState::new`], which starts every session at zero. A caller who
    /// let the key go and immediately reconnected under the same `?session=`
    /// — a redial, or the close-4409 displacement a second connection causes —
    /// got a fresh session whose FIRST generations were the very numbers the
    /// old session's timer was still asleep on. Within `release_grace_ms` that
    /// timer woke into the new call and cut a boundary it had never seen.
    ///
    /// The two halves of the fix are tested together, because either alone is
    /// no fix: the cell mints generations monotonically across sessions, and
    /// the machine ignores a token that is not the one it is draining on.
    #[test]
    fn a_reconnect_never_reuses_a_generation_a_timer_is_asleep_on() {
        let mut cell = parked_cell(5_000);
        let session = "redial";

        // The first life: born, holds, releases — which arms a timer on the
        // generation its machine moved to, and the handler remembers it.
        let mut first = SessionState::new(Mode::Hold, true, true, 1_500);
        first.grace_token = cell.next_grace_generation();
        turns::step(&mut first, session, Input::Control(ClientFrame::Hold));
        turns::step(&mut first, session, Input::Control(ClientFrame::Release));
        let asleep_on = first.grace_token;
        cell.observe_grace_generation(asleep_on);

        // The same identity comes back before that timer has fired, and gets
        // to the same point in its own take.
        let mut second = SessionState::new(Mode::Hold, true, true, 1_500);
        second.grace_token = cell.next_grace_generation();
        turns::step(&mut second, session, Input::Control(ClientFrame::Hold));
        turns::step(&mut second, session, Input::Control(ClientFrame::Release));
        assert!(
            second.grace_token > asleep_on,
            "a new life must start above every generation still in flight:              {} is not above {asleep_on}",
            second.grace_token
        );

        // The old timer wakes up inside the new call.
        let actions = turns::step(
            &mut second,
            session,
            Input::ReleaseGraceExpired { token: asleep_on },
        );
        assert_eq!(
            actions,
            Vec::new(),
            "a timer armed by a connection that is gone cuts nothing"
        );
        assert!(
            second
                .hold
                .as_ref()
                .expect("the new boundary is still open")
                .draining,
            "and the new take is still waiting for its own provider"
        );
    }
}
