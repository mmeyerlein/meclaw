//! The `voice` cell: handler half.
//!
//! The handler owns the per-session state and the turn state machine, and it is
//! the only half that emits. Audio never reaches it — a sample that entered a
//! mailbox would be a sample that waits behind a message.
//!
//! This file carries the two channel vocabularies of the dual task. The logic
//! that speaks them arrives with step 4 of strand t1.

use crate::voice::contract::SttEvent;
use crate::voice::io::{VoiceIo, run_io};
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
    /// Close this session's connection with a code.
    Close {
        /// The session to close.
        session_id: String,
        /// The WebSocket close code, e.g. [`crate::voice::wire::CLOSE_SESSION_REPLACED`].
        code: u16,
    },
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
    sessions: HashMap<String, SessionState>,
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
        let content = json!({
            "header": {
                "route": "speak_end",
                "session_id": session_id,
                "call_id": session_id,
                "speak_id": speak_id,
                "reason": reason,
                "platform": "voice",
            },
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
        let content = json!({
            "header": Value::Object(header),
            "messages": [],
            "meta": {"detail": detail},
        });
        let _ = sink.push(CellOutput { target, content }).await;
    }
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
