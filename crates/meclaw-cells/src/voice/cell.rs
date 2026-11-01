//! The `voice` cell: handler half.
//!
//! The handler owns the per-session state and the turn state machine, and it is
//! the only half that emits. Audio never reaches it — a sample that entered a
//! mailbox would be a sample that waits behind a message.
//!
//! This file carries the two channel vocabularies of the dual task. The logic
//! that speaks them arrives with step 4 of strand t1.

use crate::credential::CredentialSlots;
use crate::voice::connection::APPEND_MAX_CHARS;
use crate::voice::contract::{AppendKind, DuplexEvent, SeedMessage, Speaker, SttEvent};
use crate::voice::io::{VoiceIo, run_io};
use crate::voice::live_turns::{self, TurnAction, TurnInput, TurnState};
use crate::voice::params::{CredentialSlot, VoiceOverlay, VoiceParams};
use crate::voice::turns::{self, Action, Input, SessionState};
use crate::voice::wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason, WireErrorCode};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellOutput, Message, OriginSink, OutputSink, Path, Uuid};
use std::collections::{HashMap, VecDeque};
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
    ///
    /// **The connection waits for this event to be taken** (GH #836). The
    /// handler's loop is biased `stop > mailbox > events`, so an `in_speak`
    /// that was already waiting overtook this event and was refused
    /// `unknown_session` for a call whose client already held its `hello`
    /// (measured: 1 of 500 runs of `voice_t5_behaviour`/`gh620`, a 30 s timeout
    /// in `barge_in_cancels_speak`). The bias is a contract of every
    /// long-running cell and stays; what moved is the handshake: the
    /// connection sends `hello` only after `ack` has fired, and the handler
    /// fires it once the session is in its table.
    Connected {
        /// The session this connection claimed.
        session_id: String,
        /// The mode this connection starts in.
        mode: Mode,
        /// Fired by the handler once the session is in its table; the
        /// connection says `hello` only after it. `None` on an event built by
        /// hand (a test standing in for the I/O half), which nobody waits on.
        /// A handler of its own calls [`VoiceEvent::acknowledge`].
        ack: Option<tokio::sync::oneshot::Sender<()>>,
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
    /// The connection sent the model's audio to the client: the first audible
    /// frame since the last transcript fragment it passed on (BAT R1e, C3;
    /// silence does not count, fix-round C-1). The
    /// turn machine reads the answer's `t_first_audio_ms` from it.
    LiveAudioOut {
        /// The session whose audio this is.
        session_id: String,
        /// When the frame went out, on the clock [`VoiceEvent::LiveTick`]
        /// carries.
        at_ms: u64,
    },
    /// The duplex provider of this connection gave up, and with it the call.
    ///
    /// **There is no reconnect** (OR-L20): the session IS the conversation, and
    /// a fresh socket would be a fresh conversation with no memory of this one.
    /// So this reaches the error lane as `duplex_failed` and the client reads a
    /// close. The one exception is a renewal in place of the dead session
    /// where `params.duplex.recover_max` asks for it (GH #1377): it opens on a
    /// fresh seed, and once the attempts are spent the next end of the session
    /// in charge is this, also where the provider ended it in good order.
    DuplexFailed {
        /// The session whose provider is gone.
        session_id: String,
        /// Human-readable cause. Never carries a credential.
        detail: String,
    },
    /// The duplex provider session of this call was renewed, inside the same
    /// call (GH #896).
    ///
    /// A live model ends a session at a limit of its own, and the call has to
    /// outlive it: the connection opened a second session while the first was
    /// still running, and that one has now started. It is NOT a reconnect —
    /// the call, the connection and `session_id` stay what they were, and a
    /// provider that drops its socket still ends the call. The handler answers
    /// with the `renewed` lane, and the colony's answer to THAT — an
    /// `in_advise` `context` carrying `hop.renewal_n` — is the handover the
    /// new session hears before the caller's audio reaches it.
    Renewed {
        /// The call whose provider session was renewed.
        session_id: String,
        /// Which renewal of this call this is, counting from 1.
        renewal_n: u32,
        /// When the renewed session started, in milliseconds since the Unix
        /// epoch.
        renewed_at_ms: u64,
    },
    /// A duplex session of this call is opening and waits for its seed (D48):
    /// the handler answers with the `seed_ask` lane, and the colony's answer to
    /// THAT comes back on `in_seed`. Only where `seed_wait_ms` is set.
    SeedAsk {
        /// The call whose session opens.
        session_id: String,
        /// Why it opens: the call's first session, or a renewal of it.
        reason: SeedReason,
        /// Which session of the call asks: `0` for the first, else the
        /// `renewal_n` it will carry on `renewed` (review M1). The answer
        /// gives it back on `hop.renewal_n` and reaches only that session.
        renewal_n: u32,
    },
    /// A time mark of this call fell due (`params.call_marks_s` /
    /// `call_mark_every_s`, D48): the handler emits it on the `call_mark`
    /// lane. The connection's own timer, so a call that is over fires none.
    CallMark {
        /// The call.
        session_id: String,
        /// The mark, in seconds since the call's connection opened.
        elapsed_s: u64,
    },
    /// What the call used so far, over every provider session it ran on
    /// (D48, DK): the connection's ledger, which sees every session -- the
    /// one in charge, the one retiring, a renewal before it takes over. The
    /// handler emits it on the `usage` lane where `params.emit_usage` asks.
    CallUsage {
        /// The call.
        session_id: String,
        /// The call's total, in seconds.
        seconds: f64,
        /// Whether this is the one total of the ended call (`final`).
        last: bool,
        /// Whether every session that counts closed; `false` where a last
        /// meter reading stands in for a close that never arrived.
        complete: bool,
    },
    /// An `in_seed` named a call no session holds -- neither parked at the
    /// ring nor held by a connection (review K5). The handler says so on the
    /// error lane as `unknown_session`.
    SeedUnheld {
        /// The call the seed named.
        session_id: String,
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
    /// GH #1059: a vaulted slot's credential round ended without a box. Sent
    /// by the I/O half, which owns the slot's clock; the handler asks again
    /// with a fresh recipient. Without it a lost `on_start` question (broker
    /// not spawned yet, vault locked, an error instead of a box) would leave
    /// the slot without a key for the whole life: nothing parked here opens a
    /// new round the way an `llm` turn does.
    CredentialRoundExpired {
        /// The slot still waiting.
        slot: CredentialSlot,
        /// The round that starts now (1 = the `on_start` question).
        round: u32,
        /// Its wait in ms (doubling, capped at `credential_backoff_max_ms`).
        wait_ms: u64,
    },
}

/// Why a duplex session asks for its seed (D48), as the `seed_ask` lane says
/// it in `hop.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedReason {
    /// The call's first session.
    Start,
    /// A renewal of the call's session (GH #896).
    Renew,
    /// A session that takes the place of one the provider ended mid-call
    /// (GH #1377, `params.duplex.recover_max`).
    Recover,
}

impl SeedReason {
    /// The word on the lane.
    pub const fn as_str(self) -> &'static str {
        match self {
            SeedReason::Start => "start",
            SeedReason::Renew => "renew",
            SeedReason::Recover => "recover",
        }
    }
}

impl VoiceEvent {
    /// Tell the I/O half that the session of a `Connected` is taken (GH #836).
    ///
    /// [`VoiceCell`]'s own handler does it right after the session is in its
    /// table. Whoever consumes the events of a [`crate::voice::io::VoiceIo`]
    /// without a `VoiceCell` behind it -- a test that stands in for the handler
    /// -- calls this, or every connection waits `external_timeout` for a
    /// `hello` and is then closed with `1013`. A no-op on every other event and
    /// on a second call.
    pub fn acknowledge(&mut self) {
        if let Self::Connected { ack, .. } = self
            && let Some(ack) = ack.take()
        {
            // The connection may already have given up (its deadline ran out,
            // or the client left); an answer nobody waits for is not an error.
            let _ = ack.send(());
        }
    }
}

/// What the handler tells the I/O half.
#[derive(Debug)]
pub enum VoiceReconfig {
    /// Open the next provider session of this duplex call now, the way a due
    /// `renew_after_ms` does (D48, `in_advise` section `renew`): it asks for
    /// its seed again and takes over at the first quiet moment. A renewal that
    /// is already opening absorbs it -- one renewal per boundary.
    Renew {
        /// The session whose provider session is renewed.
        session_id: String,
    },
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
        /// The renewal this append is the handover for (GH #896); see
        /// [`crate::voice::io::ToConnection::Advise`].
        takeover: Option<u32>,
    },
    /// Hand the seed of a call to the duplex session that is opening (D48).
    Seed {
        /// The call.
        session_id: String,
        /// The session of the call the seed answers (`seed_ask`'s
        /// `renewal_n`, `0` for the first).
        renewal_n: u32,
        /// The messages, oldest first.
        messages: Vec<crate::voice::contract::SeedMessage>,
    },
    /// Open the duplex session of a call whose line still rings, so the
    /// media connection that names it finds the model ready (GH #1055).
    Prewarm {
        /// The call, as the media connection will name it (`?session=`).
        session_id: String,
    },
    /// The call ended; an early session nobody connected to is closed.
    DropPrewarm {
        /// The call that ended.
        session_id: String,
    },
    /// Close this session's connection with a code.
    Close {
        /// The session to close.
        session_id: String,
        /// The WebSocket close code, e.g. [`crate::voice::wire::CLOSE_SESSION_REPLACED`].
        code: u16,
    },
    /// GH #1059: the key of `slot`, opened from the vault's sealed box. The
    /// I/O half builds the slot's adapter with it; sessions waiting on the
    /// slot start, later ones find it at once. `Secret` prints `<sealed>`, so
    /// a `{:?}` of this frame cannot leak it.
    Credential {
        /// The slot the key is for.
        slot: CredentialSlot,
        /// The delivered key.
        secret: crate::credential::Secret,
    },
}

/// What the handler remembers about one live duplex session.
///
/// The counterpart of [`SessionState`], and deliberately not the same type:
/// a cascade session has a hold, a boundary phase, a release grace and a speak
/// queue, and a duplex session has none of the four. What it has instead is a
/// turn machine that runs off the model's clock, the delegations the model
/// opened, and the last meter reading — whose `usage_ratio` rides out on the
/// `turn` lane rather than on a `session` lane of its own (OR-L22). What the
/// call used goes out on the `usage` lane, the one exception to OR-L22; its
/// ledger is the connection's (`call_usage`, D48), which sees every session.
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
    /// Delegations the deadline closed with the fallback sentence, with the
    /// turn count each one opened at (GH #728).
    ///
    /// The colony may still answer such a delegation later. Spoken while the
    /// caller is in the turn that asked, the answer is a late but welcome
    /// fact; spoken after the conversation moved on, it is a sentence about
    /// something nobody is talking about any more. The list is what lets
    /// `advise` tell the two apart, and it is short by construction: at most
    /// [`CLOSED_DELEGATIONS_KEPT`] entries, the oldest dropped first.
    pub closed_delegations: Vec<OpenDelegation>,
    /// Delegations a spoken answer has already been said for (BAT R1e, C4).
    ///
    /// One delegated sentence can be answered twice: the delegation's own
    /// round and the turn's round of the same words, which copies the
    /// `delegation_id` from the window (05-P2: two `fact`s, +23.28 s and
    /// +28.28 s). The model gets exactly one answer per delegation, the
    /// first; this list is how a second one is recognised. Bounded like
    /// [`Self::closed_delegations`], by [`CLOSED_DELEGATIONS_KEPT`].
    pub answered_delegations: Vec<String>,
    /// How many times the provider session of this call has been renewed —
    /// the `renewal_n` of the last `renewed` emission, `0` before the first
    /// (GH #896).
    pub renewals: u32,
    /// When the last `partial` lane emission of the caller's side left, on
    /// the monotonic clock; `None` before the first one of the open turn
    /// (D48, LC2: the throttle of `delta_min_chars` / `delta_min_ms`).
    pub partial_sent_at: Option<tokio::time::Instant>,
    /// How many characters that emission carried.
    pub partial_sent_chars: usize,
    /// The newest caller text the throttle held back, sent before the `turn`
    /// that closes it so the last state always leaves.
    pub partial_held: Option<String>,
    /// Advice held for the end of the caller's next turn (`hop.at_turn_end`,
    /// OR-LV-9), in the order it came; at most [`HELD_ADVISE_KEPT`].
    pub held_advise: Vec<HeldAdvise>,
    /// Whether the session is closing (`DuplexEvent::Closed`): the turn it
    /// closes releases nothing into it; what is held goes to the colony as a
    /// `session_ended` dead letter at the disconnect (review M4, GH #1016).
    pub closing: bool,
}

/// How many held pieces of advice a session keeps (review K5). A held list
/// waits for the caller's next turn, and a caller on hold or muted says
/// nothing for as long as that lasts; a turn boundary carries a handful.
pub const HELD_ADVISE_KEPT: usize = 16;

/// One piece of advice held until the caller's turn closes (D48, LC2).
#[derive(Debug, Clone)]
pub enum HeldAdvise {
    /// An append, with the arguments `push_advise` takes.
    Push {
        /// The message it came in, on the first append a message makes
        /// (an `interrupt` makes two): what a call that ends first hands to
        /// the colony as a `session_ended` dead letter (review M4).
        message: Option<Box<Message>>,
        /// Whether the section is a spoken one (an `interrupt` counts, it
        /// carries a spoken `fact`), which is what makes an answer to a
        /// delegation the fallback closed a straggler (GH #728) and a second
        /// answer to an answered one a duplicate (BAT R1e, C4).
        section_spoken: bool,
        /// The append channel.
        kind: AppendKind,
        /// The delegation it answers, where it answers one.
        delegation_id: Option<String>,
        /// The words.
        text: String,
        /// Whether it opens a spoken section.
        spoken: bool,
        /// The renewal it hands over to, where it is a handover.
        takeover: Option<u32>,
    },
    /// A renewal of the provider session; held at most once per boundary.
    Renew,
}

/// Hold one piece of advice for the caller's turn, dropping the oldest past
/// [`HELD_ADVISE_KEPT`] with a warning (review K5). Returns what it dropped:
/// the caller dead-letters a dropped message, so nothing vanishes (GH #1016,
/// review kv-2 A-k1).
#[must_use]
fn hold(
    path: &Path,
    session_id: &str,
    state: &mut LiveSessionState,
    advice: HeldAdvise,
) -> Vec<HeldAdvise> {
    state.held_advise.push(advice);
    let mut excess = state.held_advise.len().saturating_sub(HELD_ADVISE_KEPT);
    // The second push of an `interrupt` carries no message of its own: it
    // goes with the first, never stays behind as half an instruction.
    while excess > 0
        && matches!(
            state.held_advise.get(excess),
            Some(HeldAdvise::Push { message: None, .. })
        )
    {
        excess += 1;
    }
    let dropped: Vec<HeldAdvise> = state.held_advise.drain(..excess).collect();
    for d in &dropped {
        tracing::warn!(
            path = path.as_str(),
            %session_id,
            advice = ?d,
            "voice: too much advice held for the caller's turn -- the oldest is dropped"
        );
    }
    dropped
}

/// How many fallback-closed delegations a session remembers (GH #728). A call
/// carries none or one delegation, two on a bad day; eight is headroom, and a
/// bound keeps a very long call from growing the list without end.
pub const CLOSED_DELEGATIONS_KEPT: usize = 8;

/// How many ended sessions a `voice` cell remembers (GH #1016).
///
/// A late answer comes for the call that just ended, seconds after it: the
/// longest measured gap between a hang-up and the last answer written for that
/// call was eleven seconds (2026-09-28). A cell carries one call at a time on
/// a telephone and a handful on a screen, so 64 is many calls of slack, and a
/// session older than that is answered as it always was, `unknown_session`.
pub const ENDED_SESSIONS_KEPT: usize = 64;

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
    /// How many turns the session had closed when the delegation opened — the
    /// index of the turn the model delegated from (GH #728).
    pub opened_turn: u64,
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
            closed_delegations: Vec::new(),
            answered_delegations: Vec::new(),
            renewals: 0,
            partial_sent_at: None,
            partial_sent_chars: 0,
            partial_held: None,
            held_advise: Vec::new(),
            closing: false,
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
    /// The verified sender of a session, as the `in_session` lane handed it
    /// (GH #979), keyed by `session_id`.
    ///
    /// A table of its own and not a field of either session type: the lane
    /// comes from the signalling half of a telephone, which hears the call
    /// from the switch at the same moment the switch opens the audio fork, so
    /// the sender may arrive BEFORE the connection does. It is stamped as
    /// `hop.user_id` on every lane that carries the caller's own words
    /// ([`CALLER_ROUTES`]) and dropped with the connection. A session nobody
    /// named carries no `user_id` at all -- the cell itself knows nobody.
    senders: HashMap<String, String>,
    /// The sessions whose connection this cell held and lost, newest last,
    /// at most [`ENDED_SESSIONS_KEPT`] of them (GH #1016).
    ///
    /// The media half is the one place that knows a call is over -- an edge
    /// condition reads the message, not the line -- so this is where the door
    /// of a session closes: an `in_speak` or `in_advise` for a session in here
    /// is a late answer and goes to the colony as a `session_ended` dead
    /// letter instead of to the model. A session id that connects again
    /// leaves the list: it is a new call.
    ended: VecDeque<String>,
    /// Where a late answer is handed as a dead letter: the colony inbox the
    /// substrate gives every cell at spawn (GH #1016). `None` only where a
    /// test builds the cell without a colony; the late answer is then logged
    /// and dropped, never sent on.
    dead_letters: Option<mpsc::Sender<meclaw_colony::ColonyMsg>>,
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
    /// Whether a duplex call reports its usage on the `usage` lane
    /// (`params.emit_usage`, D48). Settled at birth.
    emit_usage: bool,
    /// The call-time marks this life was born with (`call_marks_s`,
    /// `call_mark_every_s`). The I/O half rings them; the handler keeps them
    /// only so its overlay view -- the merge base of a params update -- says
    /// what the cell runs on (GH #1379).
    call_marks: crate::voice::call_marks::CallMarks,
    /// The throttle of the caller's `partial` lane in duplex mode
    /// (`params.duplex.delta_min_chars` / `delta_min_ms`, D48); `(0, 0)`
    /// sends every delta. Settled at birth like the whole duplex block.
    duplex_delta_min: (usize, u64),
    /// Whether the model's transcript reaches the `spoken` lane
    /// (`params.duplex.emit_spoken`); `None` follows `emit_partials`.
    duplex_emit_spoken: Option<bool>,
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
    /// GH #1059: the grants this cell spends, one entry per provider slot that
    /// names one. Empty for a cell on literal keys alone.
    grants: Vec<(CredentialSlot, String)>,
    /// GH #1059: key pairs and opened keys of those grants, RAM-only — every
    /// (re)start asks again. Nothing is ever parked in it: a session waits in
    /// the I/O half, where the key is used, not here.
    credentials: CredentialSlots<()>,
    /// Review V2 M2 (GH #1061): the newest round each grant was asked in.
    /// Slots that share a grant share the question, and with equal waits
    /// their clocks expire together: without this, every later round asked
    /// twice and the first box came back `Late`.
    asked_round: HashMap<String, u32>,
    /// GH #1059: carried for the overlay only (immutable; the I/O half's
    /// clocks read the value of their life).
    credential_backoff_max_ms: u64,
}

impl VoiceCell {
    /// Hand this cell the colony inbox it dead-letters a late answer into
    /// (GH #1016). The factory passes the inbox every cell gets at spawn.
    #[must_use]
    pub fn with_dead_letters(mut self, inbox: mpsc::Sender<meclaw_colony::ColonyMsg>) -> Self {
        self.dead_letters = Some(inbox);
        self
    }

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
        // GH #1059: nothing is ever parked in the credential slots (a session
        // waits in the I/O half), so no round expires with items to answer.
        let on_expired: crate::credential::ExpiryFn<()> =
            std::sync::Arc::new(|_, _| Box::pin(async {}));
        Self {
            to_io,
            path,
            io: Some(io),
            sessions: HashMap::new(),
            live_sessions: HashMap::new(),
            senders: HashMap::new(),
            ended: VecDeque::new(),
            dead_letters: None,
            duplex: params.duplex.is_some(),
            duplex_turn_gap_ms: duplex_gap_ms(params),
            duplex_backchannel_max_ms: duplex_backchannel_ms(params),
            duplex_delegation_grace_ms: duplex_delegation_grace_ms(params),
            duplex_delegation_fallback: duplex_delegation_fallback(params),
            mount: params.mount.clone(),
            default_mode: params.default_mode,
            barge_in: params.barge_in,
            emit_partials: params.emit_partials,
            emit_usage: params.emit_usage,
            call_marks: params.call_marks.clone(),
            duplex_delta_min: duplex_delta_min(params),
            duplex_emit_spoken: duplex_emit_spoken(params),
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
            grants: CredentialSlot::ALL
                .into_iter()
                .filter_map(|s| params.slot_grant(s).map(|g| (s, g.to_string())))
                .collect(),
            credentials: CredentialSlots::new(
                crate::credential::default_credential_wait_ms(),
                crate::credential::default_credential_wait_max(),
                on_expired,
            ),
            credential_backoff_max_ms: params.credential_backoff_max_ms,
            asked_round: HashMap::new(),
        }
    }

    /// GH #1059: the public recipient key of `slot`'s question in flight
    /// (tests/diagnosis; never a secret).
    #[must_use]
    pub fn credential_recipient_hex(&self, slot: CredentialSlot) -> Option<String> {
        let (_, grant) = self.grants.iter().find(|(s, _)| *s == slot)?;
        self.credentials.recipient_hex(grant)
    }

    /// GH #1059: ask the access hive for the key of `grant`. A source emission
    /// on this cell's own path like every other one; the template's edge on
    /// `hop.route == "credential_request"` decides where it goes. Slots that
    /// share one grant share the question.
    async fn ask_for_credential(&mut self, grant: &str, sink: &OriginSink) {
        let content = match self.credentials.request(grant) {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(
                    path = self.path.as_str(),
                    error = %e,
                    "voice: no random source for a credential request"
                );
                return;
            }
        };
        tracing::info!(
            path = self.path.as_str(),
            grant = %grant,
            slots = %self.slots_of(grant).join(","),
            "voice: asking the vault for a provider credential"
        );
        let _ = sink
            .emit(CellOutput {
                target: self.path.clone(),
                content,
            })
            .await;
    }

    /// GH #1059: the slot names that spend `grant`, for log lines.
    fn slots_of(&self, grant: &str) -> Vec<&'static str> {
        self.grants
            .iter()
            .filter(|(_, g)| g == grant)
            .map(|(s, _)| s.as_str())
            .collect()
    }

    /// GH #1059: take a sealed delivery. Opened → every slot of that grant gets
    /// the key over the I/O seam (its adapter is built with it). Not opened →
    /// the slot stays without a key and its next round asks again — no retry
    /// path of its own. Never answered with an emission, never echoes a value.
    async fn accept_credential(&mut self, content: &Value) {
        match self.credentials.accept_sealed(content).await {
            Ok(accepted) => {
                let Some(secret) = self.credentials.secret_handle(&accepted.grant) else {
                    tracing::warn!(
                        path = self.path.as_str(),
                        grant = %accepted.grant,
                        "voice: the sealed credential opened empty — the slot stays without a key"
                    );
                    return;
                };
                let slots: Vec<CredentialSlot> = self
                    .grants
                    .iter()
                    .filter(|(_, g)| *g == accepted.grant)
                    .map(|(s, _)| *s)
                    .collect();
                tracing::info!(
                    path = self.path.as_str(),
                    grant = %accepted.grant,
                    slots = %self.slots_of(&accepted.grant).join(","),
                    "voice: credential received sealed and opened in RAM"
                );
                for slot in slots {
                    let secret = secret.clone();
                    let _ = self
                        .to_io
                        .send(VoiceReconfig::Credential { slot, secret })
                        .await;
                }
            }
            Err(refusal) => match refusal.detail() {
                None => tracing::warn!(
                    path = self.path.as_str(),
                    "voice: a sealed box of an earlier credential round arrived late and was \
                     discarded"
                ),
                Some(detail) => tracing::warn!(
                    path = self.path.as_str(),
                    %detail,
                    "voice: the sealed credential was refused — the slot stays without a key and \
                     asks again after the round's wait"
                ),
            },
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
            credential_backoff_max_ms: self.credential_backoff_max_ms,
            emit_usage: self.emit_usage,
            call_marks_s: self.call_marks.marks_s.clone(),
            call_mark_every_s: self.call_marks.every_s,
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
    ///
    /// Every source emission passes here, so this is the one place the
    /// verified sender of a session is stamped (GH #979): a lane that carries
    /// the caller's words gets `hop.user_id`, and every other lane -- and
    /// every session nobody named -- is left exactly as it was built.
    async fn emit(&self, sink: &OriginSink, mut content: Value) {
        self.stamp_sender(&mut content);
        let _ = sink
            .emit(CellOutput {
                target: self.path.clone(),
                content,
            })
            .await;
    }

    /// Stamp the verified sender of the emission's session as `hop.user_id`
    /// and `hop.verified_user` (GH #979), on the lanes that carry what the
    /// caller said.
    fn stamp_sender(&self, content: &mut Value) {
        let Some(header) = content.get_mut("header").and_then(Value::as_object_mut) else {
            return;
        };
        let route = header
            .get("route")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !CALLER_ROUTES.contains(&route) {
            return;
        }
        let Some(sender) = header
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|s| self.senders.get(s))
            .cloned()
        else {
            return;
        };
        // Both keys, one value: `user_id` is the sender a firewall allowlists
        // by, `verified_user` the proof a member's ingress stamps
        // `context.speaker` from (GH #979, OR-NL.I.6). This lane only ever
        // carries a sender somebody verified, so the two coincide here.
        header.insert("user_id".into(), json!(sender));
        header.insert("verified_user".into(), json!(sender));
    }

    /// A fact of the line (GH #1055): a ring opens the call's duplex session
    /// early, an end closes one nobody connected to.
    ///
    /// Measured on a telephone line (five calls, quiet window): pickup to the
    /// first syllable was 1.84 s p50 / 2.14 s p95, because the session was
    /// opened only when the media fork connected after the pickup — the
    /// provider handshake (median 505 ms) and the greeting's time to first
    /// audio (median 938 ms) were both spent with the caller on the line. A
    /// ring lasts longer than both together.
    async fn line_fact(&self, msg: &Message, route: &str) {
        if !self.duplex {
            return;
        }
        let call = ["call_uuid", "call_id", "session_id"].iter().find_map(|k| {
            msg.headers
                .hop
                .get(*k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
        let Some(session_id) = call else {
            tracing::debug!(%route, "voice: a line fact without a call id, ignored");
            return;
        };
        let cmd = if route == "call_ringing" {
            VoiceReconfig::Prewarm { session_id }
        } else {
            VoiceReconfig::DropPrewarm { session_id }
        };
        let _ = self.to_io.send(cmd).await;
    }

    /// The `in_seed` lane (D48): the colony's answer to `seed_ask`.
    ///
    /// The call is named like an `in_advise` names it (`context.call_id`, then
    /// `context.session_id`, then `hop.call_id`). No live-session check: the
    /// session this seed opens may still be ringing, parked for its media
    /// connection (GH #1055); a seed for a call nobody holds goes nowhere and
    /// says so in the log. Each message is `{role, text}` (`origin` is read
    /// where `role` is missing, the universal body's key); a role the live
    /// model does not take (developer, user, assistant) is left out. The
    /// sender's `est_tokens` is not read: the adapter counts for itself.
    async fn take_seed(&self, msg: &Message, body: &Value, reply_target: Path, sink: &OutputSink) {
        let named = |map: &Map<String, Value>, key: &str| {
            map.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let session_id = named(&msg.headers.context, "call_id")
            .or_else(|| named(&msg.headers.context, "session_id"))
            .or_else(|| named(&msg.headers.hop, "call_id"));
        let Some(session_id) = session_id else {
            self.refuse(
                sink,
                reply_target,
                "missing_session",
                "in_seed names the call on context.call_id (or context.session_id, hop.call_id)",
                None,
            )
            .await;
            return;
        };
        if !self.duplex {
            self.refuse(
                sink,
                reply_target,
                "wrong_engine",
                "in_seed needs a duplex session; this cell runs a cascade",
                Some(&session_id),
            )
            .await;
            return;
        }
        // Which session the seed answers (review M1): the `renewal_n` of its
        // `seed_ask`, given back. Without it a late answer to one session
        // would open the next with the wrong conversation.
        let renewal_n = msg
            .headers
            .hop
            .get("renewal_n")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .and_then(|n| u32::try_from(n).ok());
        let Some(renewal_n) = renewal_n else {
            self.refuse(
                sink,
                reply_target,
                "missing_renewal_n",
                "in_seed gives back the renewal_n of its seed_ask on hop.renewal_n (0 = start)",
                Some(&session_id),
            )
            .await;
            return;
        };
        let mut dropped = 0usize;
        let messages: Vec<SeedMessage> = body
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let role = m
                            .get("role")
                            .or_else(|| m.get("origin"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        let text = m.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                        if !matches!(role, "developer" | "user" | "assistant")
                            || text.trim().is_empty()
                        {
                            dropped += 1;
                            return None;
                        }
                        Some(SeedMessage {
                            role: role.to_string(),
                            text: text.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if dropped > 0 {
            tracing::warn!(
                path = self.path.as_str(),
                %session_id, dropped,
                "voice: seed messages without a role the live model takes, or without text, \
                 were left out"
            );
        }
        let _ = self
            .to_io
            .send(VoiceReconfig::Seed {
                session_id,
                renewal_n,
                messages,
            })
            .await;
    }

    /// The `in_session` lane (GH #979): who is speaking in one session.
    ///
    /// Sent by the signalling half of a telephone, which knows the caller the
    /// switch verified while this cell knows only the call's UUID. The session
    /// is read off the HOP, where that half writes it, and not off context: a
    /// context key is inherited from whatever came before, a hop key is what
    /// the sender of this one message said. An empty `user_id` withdraws the
    /// name -- fail-closed, the session's turns name nobody again. No answer
    /// is emitted: the lane is a fact handed over, not a question.
    async fn take_sender(&mut self, msg: &Message, reply_target: Path, sink: &OutputSink) {
        let on_hop = |key: &str| {
            msg.headers
                .hop
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let Some(session_id) = on_hop("call_id").or_else(|| on_hop("session_id")) else {
            self.refuse(
                sink,
                reply_target,
                "missing_session",
                "in_session names the session on hop.call_id (or hop.session_id)",
                None,
            )
            .await;
            return;
        };
        match on_hop("user_id") {
            Some(user) => {
                tracing::info!(
                    path = self.path.as_str(),
                    %session_id,
                    "voice: the session's sender is named"
                );
                self.senders.insert(session_id, user);
            }
            None => {
                self.senders.remove(&session_id);
            }
        }
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
                    opened_turn: state.closed_turns,
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
                // the call's running total on the `usage` lane (D48, DK, the
                // one exception to OR-L22; summed by the connection, which
                // sees every session, `VoiceEvent::CallUsage`), and the log
                // line at the end of the call.
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
                        state.closing = true;
                        live_turns::step(&mut state.turns, TurnInput::Close, gap, backchannel)
                    }
                    None => Vec::new(),
                };
                self.run_live_actions(session_id, actions, sink).await;
                // OR-L22 holds -- there is no `session` lane -- with ONE
                // exception since D48/DK: the `usage` lane, because a call
                // renewed at every fact turn spans many sessions and its
                // cost is their sum, which no single `Closed` carries. The
                // connection counts this close in the call's ledger and sends
                // the `final` once, at the end of the call
                // (`VoiceEvent::CallUsage`), not here: a session in charge
                // that never closes still ends a call.
                tracing::info!(
                    path = self.path.as_str(),
                    %session_id, %reason, usage_seconds,
                    "voice: the duplex session is over"
                );
            }
        }
    }

    /// One `usage` emission of a duplex call (D48, DK), where
    /// `params.emit_usage` asks for it: `seconds` is the call's total so far,
    /// over every session it ran on; `final` marks the one at the end of the
    /// call, and only that one carries `complete`.
    async fn emit_usage(
        &self,
        session_id: &str,
        seconds: f64,
        last: bool,
        complete: bool,
        sink: &OriginSink,
    ) {
        if !self.emit_usage {
            return;
        }
        let hop = if last {
            json!({"seconds": seconds, "final": true, "complete": complete})
        } else {
            json!({"seconds": seconds, "final": false})
        };
        let content = self.live_body("usage", session_id, hop, json!([]));
        self.emit(sink, content).await;
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
        let mut closed = Vec::new();
        state.open_delegations.retain(|open| {
            if now_ms.saturating_sub(open.opened_at_ms) > grace {
                stale.push(open.id.clone());
                closed.push(open.clone());
                false
            } else {
                true
            }
        });
        // Remembered, not forgotten (GH #728): the colony may still answer a
        // delegation the fallback closed, and `advise` has to know whether the
        // conversation has moved on since it opened.
        state.closed_delegations.extend(closed);
        let excess = state
            .closed_delegations
            .len()
            .saturating_sub(CLOSED_DELEGATIONS_KEPT);
        state.closed_delegations.drain(..excess);
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
                None,
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
                    // The last state of the caller's stream leaves before the
                    // turn that closes it (D48, LC2): a throttled `partial`
                    // the curator never saw would leave it speculating on an
                    // older sentence than the one the turn carries.
                    let held = self.live_sessions.get_mut(session_id).and_then(|state| {
                        state.closed_turns = turn.index.saturating_add(1);
                        state.partial_sent_at = None;
                        state.partial_sent_chars = 0;
                        state.partial_held.take()
                    });
                    // The turn's own text, not the held one: after a
                    // barge-in the held text already carries the run that
                    // went to the next turn (review K2).
                    if held.is_some() && self.emit_partials {
                        let text = turn.user.clone();
                        let content = self.live_body(
                            "partial",
                            session_id,
                            json!({"eager": false, "turn_id": turn_id}),
                            json!([{"origin": "user", "type": "text", "text": text}]),
                        );
                        self.emit(sink, content).await;
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
                    // When the caller stopped and the model's first answer
                    // audio went out, on the call's clock (D48, LC2: "time to
                    // the first word" for the counselor and the battery; the
                    // audio since BAT R1e, C3).
                    if let Some(obj) = extra.as_object_mut() {
                        if let Some(ms) = turn.person_end_ms {
                            obj.insert("t_person_end_ms".into(), json!(ms));
                        }
                        if let Some(ms) = turn.first_audio_ms {
                            obj.insert("t_first_audio_ms".into(), json!(ms));
                        }
                    }
                    // A turn the caller spoke in releases what is held -- not
                    // the one a closing session closes (review M4).
                    let person_turn = !turn.user.is_empty()
                        && self
                            .live_sessions
                            .get(session_id)
                            .is_some_and(|s| !s.closing);
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
                    if person_turn {
                        self.release_held_advise(session_id).await;
                    }
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
                    let lane_on = match speaker {
                        Speaker::User => self.emit_partials,
                        Speaker::Assistant => self.duplex_emit_spoken.unwrap_or(self.emit_partials),
                    };
                    if !lane_on {
                        continue;
                    }
                    if speaker == Speaker::User && !self.partial_due(session_id, &text) {
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

    /// Whether the caller's text may leave on the `partial` lane now, under
    /// `delta_min_chars` / `delta_min_ms` (D48, LC2). Event-driven, one check
    /// per incoming delta and no timer (R-AG-1): a held text waits for the
    /// next delta or for the `turn` that closes it. Marks the send where it
    /// says yes.
    fn partial_due(&mut self, session_id: &str, text: &str) -> bool {
        let (min_chars, min_ms) = self.duplex_delta_min;
        if min_chars == 0 && min_ms == 0 {
            return true;
        }
        let Some(state) = self.live_sessions.get_mut(session_id) else {
            return true;
        };
        let now = tokio::time::Instant::now();
        let chars = text.chars().count();
        let due = partial_throttle_due(
            state
                .partial_sent_at
                .map(|at| (at, state.partial_sent_chars)),
            now,
            chars,
            (min_chars, min_ms),
        );
        if due {
            state.partial_sent_at = Some(now);
            state.partial_sent_chars = chars;
            state.partial_held = None;
        } else {
            state.partial_held = Some(text.to_string());
        }
        due
    }

    /// Hand over what `hop.at_turn_end` held, now that the caller's turn has
    /// closed (OR-LV-9), in the order it came; a renewal at most once.
    async fn release_held_advise(&mut self, session_id: &str) {
        let held = match self.live_sessions.get_mut(session_id) {
            Some(state) => std::mem::take(&mut state.held_advise),
            None => return,
        };
        // Whether the push before was sent: the second half of an `interrupt`
        // (the push without a message of its own) follows its first half.
        let mut previous_sent = true;
        for advice in held {
            match advice {
                HeldAdvise::Push {
                    message,
                    kind,
                    delegation_id,
                    text,
                    spoken,
                    takeover,
                    section_spoken,
                } => {
                    // The delegation's bookkeeping happens HERE, when it is
                    // sent: held, it stayed open, so its fallback could still
                    // fall (review M2). The two halves of ONE `interrupt` are
                    // ONE answer and are settled once, on the first: settled
                    // each, the spoken half read as a second answer to its
                    // own delegation and was dropped (delta re-check N-1).
                    // `hold` never splits the pair, so the halves stay
                    // adjacent.
                    let send = if message.is_none() {
                        previous_sent
                    } else {
                        self.settle_delegation(session_id, delegation_id.as_deref(), section_spoken)
                    };
                    previous_sent = send;
                    if !send {
                        continue;
                    }
                    self.push_advise(session_id, kind, delegation_id, &text, spoken, takeover)
                        .await;
                }
                HeldAdvise::Renew => {
                    let _ = self
                        .to_io
                        .send(VoiceReconfig::Renew {
                            session_id: session_id.to_string(),
                        })
                        .await;
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
        // `interrupt` (D48, OR-LV-7) is a `correction` followed by a `fact`
        // with the same words: the model changes course AND says so. `renew`
        // carries no words at all; its kind is never read.
        let (kind, spoken) = match section {
            Some("fact") => (AppendKind::Commentary, true),
            Some("context") => (AppendKind::Thinking, false),
            Some("correction") | Some("interrupt") | Some("renew") => {
                (AppendKind::Instructions, false)
            }
            other => {
                let detail = format!(
                    "hop.section must be one of fact, context, correction, interrupt, renew; \
                     got {}",
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
        // `hop.at_turn_end` (OR-LV-9): held until the caller's next turn
        // closes, then sent before the model hears anything new.
        let at_turn_end = msg.headers.hop.get("at_turn_end").is_some_and(|v| match v {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_u64() == Some(1),
            Value::String(s) => matches!(s.trim(), "1" | "true"),
            _ => false,
        });
        // `renew` (OR-LV-6): a fresh provider session that asks for its seed
        // again — how facts reach a live model at a turn boundary. Several
        // held for one boundary are one renewal, and the connection lets a
        // renewal that is already opening absorb another.
        if section == Some("renew") {
            if at_turn_end {
                if let Some(state) = self.live_sessions.get_mut(&session_id)
                    && !state
                        .held_advise
                        .iter()
                        .any(|h| matches!(h, HeldAdvise::Renew))
                {
                    // A held `renew` carries no message: nothing to hand on.
                    let _ = hold(&self.path, &session_id, state, HeldAdvise::Renew);
                }
            } else {
                let _ = self.to_io.send(VoiceReconfig::Renew { session_id }).await;
            }
            return;
        }
        let interrupt = section == Some("interrupt");
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
        let section_name = if interrupt {
            "interrupt"
        } else {
            section_name_of(kind)
        };
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
        // Held advice settles its delegation when it is sent (review M2):
        // see `release_held_advise`. An `interrupt` carries a spoken `fact`
        // (the commentary below), so it answers its delegation like one
        // (fix-round M-4): exactly one spoken answer per `delegation_id`.
        let answers = spoken || interrupt;
        if !at_turn_end && !self.settle_delegation(&session_id, delegation_id.as_deref(), answers) {
            return;
        }
        // THE HANDOVER OF A RENEWAL (GH #896). A `context` that carries
        // `hop.renewal_n` is the colony's answer to the `renewed` lane: what
        // the renewed session has to know about the conversation so far. The
        // number travels on to the connection, which knows which renewal is
        // waiting and which one is in charge: a block reaches the renewal it
        // names, or nobody where that renewal failed (review M-8). Every part
        // of a split block carries it. A number that does not read as one is
        // no handover, and the section is the ordinary `context` it looks like.
        let takeover = if section == Some("context") {
            msg.headers
                .hop
                .get("renewal_n")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                })
                .and_then(|n| u32::try_from(n).ok())
        } else {
            None
        };
        let mut pushes = vec![(kind, spoken)];
        if interrupt {
            pushes.push((AppendKind::Commentary, true));
        }
        let section_spoken = answers;
        let mut message = Some(Box::new(msg.clone()));
        for (kind, spoken) in pushes {
            if at_turn_end && let Some(state) = self.live_sessions.get_mut(&session_id) {
                let advice = HeldAdvise::Push {
                    message: message.take(),
                    section_spoken,
                    kind,
                    delegation_id: delegation_id.clone(),
                    text: text.clone(),
                    spoken,
                    takeover,
                };
                for dropped in hold(&self.path, &session_id, state, advice) {
                    if let HeldAdvise::Push {
                        message: Some(m), ..
                    } = dropped
                    {
                        self.dead_letter_late(&m, &session_id).await;
                    }
                }
                continue;
            }
            self.push_advise(
                &session_id,
                kind,
                delegation_id.clone(),
                &text,
                spoken,
                takeover,
            )
            .await;
        }
    }

    /// Settle the delegation an answer names as it is SENT; `false` where it
    /// is a straggler and is not to be said.
    ///
    /// A STRAGGLER OF THE SIDECAR (GH #728, OR-T13). A `fact` for a
    /// delegation the deadline already closed with the fallback sentence,
    /// arriving after another turn has closed since the delegation opened, is
    /// an answer to a question the caller has left behind: the header the
    /// splitter builds carries no `late`, so the cell's own bookkeeping is
    /// what decides. Dropped without a refusal -- nobody did anything wrong,
    /// and an error line would put a non-event on the screen. Otherwise the
    /// delegation this colony has now answered is no longer open; without
    /// this the list would only ever grow, for the whole length of a call.
    /// Held advice runs this at its release, not on arrival (review M2): a
    /// delegation taken off the list while its answer waits for the caller's
    /// turn would never hear its fallback.
    fn settle_delegation(
        &mut self,
        session_id: &str,
        delegation_id: Option<&str>,
        spoken: bool,
    ) -> bool {
        let Some(id) = delegation_id else {
            return true;
        };
        let Some(state) = self.live_sessions.get_mut(session_id) else {
            return true;
        };
        if spoken
            && state
                .closed_delegations
                .iter()
                .any(|c| c.id == id && state.closed_turns > c.opened_turn)
        {
            tracing::info!(
                path = self.path.as_str(),
                %session_id, delegation_id = %id,
                "voice: a fact for a delegation the fallback closed arrived after the turn \
                 changed — not spoken"
            );
            return false;
        }
        if spoken {
            if state.answered_delegations.iter().any(|a| a == id) {
                tracing::warn!(
                    path = self.path.as_str(),
                    %session_id, delegation_id = %id,
                    "voice: a second answer for a delegation already answered -- not spoken"
                );
                return false;
            }
            state.answered_delegations.push(id.to_string());
            let excess = state
                .answered_delegations
                .len()
                .saturating_sub(CLOSED_DELEGATIONS_KEPT);
            state.answered_delegations.drain(..excess);
        }
        state.open_delegations.retain(|open| open.id != id);
        true
    }

    /// Whether an answer is a straggler for a turn this call has left (GH #728).
    ///
    /// Only a LATE answer can be one (`hop.late == "1"`), and only one that
    /// names a turn: an answer without `turn_id` is none of this rule's
    /// business. The running turn is the cascade's `"<session>#<turn_seq>"` —
    /// the last one it emitted — or, on a duplex session, the open turn or the
    /// one that closed last: the model may still be finishing the answer to
    /// the sentence it just heard. A session this cell does not hold is left
    /// to the lookups below, which refuse it by name.
    fn is_straggler(&self, msg: &Message, session_id: &str) -> bool {
        let hop = |key: &str| msg.headers.hop.get(key).and_then(|v| v.as_str());
        if hop("late") != Some("1") {
            return false;
        }
        let Some(turn_id) = hop("turn_id").filter(|t| !t.is_empty()) else {
            return false;
        };
        if self.duplex {
            let Some(state) = self.live_sessions.get(session_id) else {
                return false;
            };
            let open = state.open_turn_id(session_id);
            let last = state
                .closed_turns
                .checked_sub(1)
                .map(|n| format!("{session_id}#{n}"));
            turn_id != open && Some(turn_id) != last.as_deref()
        } else {
            let Some(state) = self.sessions.get(session_id) else {
                return false;
            };
            turn_id != format!("{session_id}#{}", state.turn_seq)
        }
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
        self.push_advise(session_id, AppendKind::Commentary, None, &text, true, None)
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
    ///
    /// `takeover` marks the handover to a renewed session (GH #896), and every
    /// part carries it: see [`crate::voice::io::ToConnection::Advise`].
    async fn push_advise(
        &mut self,
        session_id: &str,
        kind: AppendKind,
        delegation_id: Option<String>,
        text: &str,
        spoken: bool,
        takeover: Option<u32>,
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
                    takeover,
                })
                .await;
        }
    }

    /// The provider session of a duplex call was renewed (GH #896).
    ///
    /// One emission on the `renewed` lane, in the shape of every duplex lane
    /// ([`Self::live_body`]) with `renewal_n` and `renewed_at` beside the rest
    /// and no words at all: what the new session needs to hear is the
    /// colony's to write, and the handover it writes comes back on `in_advise`
    /// with `hop.renewal_n` on it. A session this cell no longer holds is gone,
    /// and a renewal of it is nobody's news.
    async fn on_renewed(
        &mut self,
        session_id: &str,
        renewal_n: u32,
        renewed_at_ms: u64,
        sink: &OriginSink,
    ) {
        let Some(state) = self.live_sessions.get_mut(session_id) else {
            tracing::debug!(
                path = self.path.as_str(),
                %session_id, renewal_n,
                "voice: a renewal for a session this cell no longer holds"
            );
            return;
        };
        state.renewals = renewal_n;
        tracing::info!(
            path = self.path.as_str(),
            %session_id, renewal_n,
            "voice: the duplex session was renewed inside the call"
        );
        let content = self.live_body(
            "renewed",
            session_id,
            json!({"renewal_n": renewal_n, "renewed_at": renewed_at_ms}),
            json!([]),
        );
        self.emit(sink, content).await;
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

    /// Remember that `session_id` ended (GH #1016), newest last, bounded by
    /// [`ENDED_SESSIONS_KEPT`].
    fn close_door(&mut self, session_id: &str) {
        self.ended.retain(|gone| gone != session_id);
        self.ended.push_back(session_id.to_string());
        while self.ended.len() > ENDED_SESSIONS_KEPT {
            self.ended.pop_front();
        }
    }

    /// The session a message addresses, if this cell saw it end (GH #1016).
    /// Read the way the speak path reads it: the call key first, the older
    /// session key where the call key is absent.
    fn ended_session_of(&self, msg: &Message) -> Option<String> {
        let addressed = |key: &str| {
            msg.headers
                .context
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let session_id = addressed("call_id").or_else(|| addressed("session_id"))?;
        self.ended.contains(&session_id).then_some(session_id)
    }

    /// Hand a late answer to the colony as a `session_ended` dead letter
    /// (GH #1016): nothing reaches the model, nothing travels on as an error,
    /// and the message itself is the record. A colony inbox that is gone is
    /// named in the log, so the answer never vanishes without a line.
    async fn dead_letter_late(&self, msg: &Message, session_id: &str) {
        let lane = msg
            .headers
            .hop
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        tracing::info!(
            path = self.path.as_str(),
            %session_id,
            %lane,
            "voice: an answer for a call that is over -- dead-lettered as session_ended"
        );
        let delivered = match &self.dead_letters {
            Some(inbox) => inbox
                .send(meclaw_colony::ColonyMsg::DeadLetterMessage {
                    message: msg.clone(),
                    reason: meclaw_colony::DeadLetterReason::SessionEnded,
                })
                .await
                .is_ok(),
            None => false,
        };
        if !delivered {
            tracing::warn!(
                path = self.path.as_str(),
                %session_id,
                %lane,
                "voice: a session_ended dead letter has no colony inbox to go to"
            );
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

/// Whether a caller's text of `chars` characters may leave on the `partial`
/// lane at `now`, given the last emission of the open turn (`sent`: when, and
/// how many characters) and the throttle (`min_chars`, `min_ms`). The first
/// emission of a turn always leaves; after it, BOTH steps have to be reached
/// (D48, LC2: 12 characters / 250 ms on a telephone line).
fn partial_throttle_due(
    sent: Option<(tokio::time::Instant, usize)>,
    now: tokio::time::Instant,
    chars: usize,
    (min_chars, min_ms): (usize, u64),
) -> bool {
    match sent {
        None => true,
        Some((at, sent_chars)) => {
            chars.saturating_sub(sent_chars) >= min_chars
                && now.saturating_duration_since(at) >= Duration::from_millis(min_ms)
        }
    }
}

/// The throttle of the caller's `partial` lane (D48); `(0, 0)` where there
/// is no `gpt_live` block, which sends every delta as before.
fn duplex_delta_min(params: &VoiceParams) -> (usize, u64) {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => (
            usize::try_from(p.delta_min_chars).unwrap_or(usize::MAX),
            p.delta_min_ms,
        ),
        _ => (0, 0),
    }
}

/// Whether the `spoken` lane runs apart from `emit_partials` (D48).
fn duplex_emit_spoken(params: &VoiceParams) -> Option<bool> {
    match &params.duplex {
        Some(crate::voice::params::DuplexParams::GptLive(p)) => p.emit_spoken,
        _ => None,
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
/// The lanes that carry what the CALLER said, and therefore the verified
/// sender of their session (GH #979): a finished turn, and a delegation, whose
/// body is the user text of the open turn. `partial` is an interim of the same
/// words and is left unnamed on purpose -- it is overtaken by its own `turn`,
/// and a name on it would be a second place for the stamp to drift from.
const CALLER_ROUTES: [&str; 2] = ["turn", "delegation"];

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

    /// GH #1059: every granted slot asks for its key before anything else —
    /// one question per grant. Key pairs and keys are RAM-only, so a respawn
    /// asks again; the I/O half meanwhile takes sessions and holds each one
    /// at most its slot's `credential_wait_ms` for the key.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let mut asked: Vec<String> = Vec::new();
            let grants: Vec<String> = self.grants.iter().map(|(_, g)| g.clone()).collect();
            for grant in grants {
                if asked.contains(&grant) {
                    continue;
                }
                self.ask_for_credential(&grant, sink).await;
                self.asked_round.insert(grant.clone(), 1);
                asked.push(grant);
            }
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

            // GH #1059: a sealed credential, before everything else. It never
            // touches `cell.db` and is never answered with an emission.
            if body.get("sealed").is_some() {
                self.accept_credential(body).await;
                return;
            }

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

            // THE LINE, NOT THE CALLER (GH #1055). `call_ringing` and
            // `call_ended` are facts of the switch; the telephony hive hands
            // them here as well as to its signalling half. They are handled
            // before anything reads a text, and they answer nothing: no lane,
            // no refusal, no dead letter. A cascade cell, or a duplex cell
            // with `prewarm_ttl_ms: 0`, has nothing to open early and ignores
            // both.
            if let Some(route @ ("call_ringing" | "call_ended")) =
                msg.headers.hop.get("route").and_then(|v| v.as_str())
            {
                self.line_fact(&msg, route).await;
                return;
            }

            // A CALL THAT IS OVER HEARS NOTHING (GH #1016). Measured
            // 2026-09-28: after the caller hung up, the answers still being
            // written for the call kept arriving as `in_speak`/`in_advise`,
            // were refused as `unknown_session` on the error lane and ended as
            // `hive_no_route` dead letters at the colony root -- one per
            // answer, named for a routing gap that is not there. An edge reads
            // the message, not the line, so the door closes here, in the one
            // place that saw the connection go.
            let late =
                if msg.headers.hop.get("route").and_then(|v| v.as_str()) == Some("in_session") {
                    None
                } else {
                    self.ended_session_of(&msg)
                };
            if let Some(session_id) = late {
                self.dead_letter_late(&msg, &session_id).await;
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

            // The seed lane (D48): the history a duplex session opens with.
            if msg.headers.hop.get("route").and_then(|v| v.as_str()) == Some("in_seed") {
                self.take_seed(&msg, body, reply_target, sink).await;
                return;
            }

            // The sender lane (GH #979): no text to speak, a name to keep.
            if msg.headers.hop.get("route").and_then(|v| v.as_str()) == Some("in_session") {
                self.take_sender(&msg, reply_target, sink).await;
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

            // A STRAGGLER AFTER A TURN CHANGE (GH #728, ruling 2026-09-22 point
            // 2). The collector marks an answer that comes back after the
            // assistant's deadline with `hop.late = "1"` and keeps the member's
            // turn on `hop.turn_id`. If that turn is no longer the one this call
            // is in, the answer is about something the caller stopped talking
            // about, and a voice call drops it: no synthesis, no append, no
            // `speak_end` -- and no refusal, because nothing went wrong. Inside
            // the deadline (`late = "0"`) or for the turn still running, the
            // answer is spoken as every answer is.
            if self.is_straggler(&msg, &session_id) {
                tracing::info!(
                    path = self.path.as_str(),
                    %session_id,
                    turn_id = msg.headers.hop.get("turn_id").and_then(|v| v.as_str()).unwrap_or_default(),
                    "voice: a late answer for a turn the call has left — not spoken"
                );
                return;
            }

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
                // GH #1059: a slot's round ended without a box → ask again,
                // fresh recipient. A box that opened in between makes it moot.
                VoiceEvent::CredentialRoundExpired {
                    slot,
                    round,
                    wait_ms,
                } => {
                    let grant = self
                        .grants
                        .iter()
                        .find(|(s, _)| *s == slot)
                        .map(|(_, g)| g.clone());
                    // A second slot of the same grant whose clock ran out in
                    // the same round finds the question already out (M2).
                    if let Some(grant) = grant
                        && self.credentials.secret(&grant).is_none()
                        && self.asked_round.get(&grant).is_none_or(|r| *r < round)
                    {
                        self.asked_round.insert(grant.clone(), round);
                        tracing::warn!(
                            path = self.path.as_str(),
                            round,
                            wait_ms,
                            grant = %grant,
                            "voice: no sealed {} credential arrived in time — asking again",
                            slot.as_str()
                        );
                        self.ask_for_credential(&grant, sink).await;
                    }
                }
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
                VoiceEvent::Connected {
                    session_id,
                    mode,
                    ack,
                } => {
                    // A session id that connects again is a new call
                    // (GH #1016): its door opens again -- first thing, before
                    // any branch below can return.
                    self.ended.retain(|gone| gone != &session_id);
                    // GH #836: the connection holds its `hello` until `ack`
                    // fires, and it fires only AFTER the insert below -- so a
                    // client that has a `hello` has a session this table knows,
                    // and an answer queued in the mailbox can no longer be
                    // refused `unknown_session` for it. A connection that gave
                    // up waiting has dropped its receiver; the send is then a
                    // no-op.
                    let take = |ack: Option<tokio::sync::oneshot::Sender<()>>| {
                        if let Some(ack) = ack {
                            let _ = ack.send(());
                        }
                    };
                    // Two kinds of session, one event. Which one it is comes
                    // from the params rather than from the event, because the
                    // engine cannot change under a live cell.
                    if self.duplex {
                        self.live_sessions
                            .insert(session_id, LiveSessionState::new(mode));
                        take(ack);
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
                    take(ack);
                }
                VoiceEvent::Disconnected { session_id } => {
                    self.sessions.remove(&session_id);
                    // What `hop.at_turn_end` still held is the colony's, as a
                    // `session_ended` dead letter (review M4, GH #1016):
                    // nothing vanishes and nothing is said into a call that is
                    // over.
                    if let Some(state) = self.live_sessions.remove(&session_id) {
                        for advice in state.held_advise {
                            match advice {
                                HeldAdvise::Push {
                                    message: Some(message),
                                    ..
                                } => self.dead_letter_late(&message, &session_id).await,
                                HeldAdvise::Push { .. } => {}
                                HeldAdvise::Renew => tracing::info!(
                                    path = self.path.as_str(),
                                    %session_id,
                                    "voice: a held renewal for a call that is over -- dropped"
                                ),
                            }
                        }
                    }
                    // GH #1016: the call is over, so the door to it closes.
                    // What still arrives for it is a late answer.
                    self.close_door(&session_id);
                    // The name goes with the connection (GH #979): a session
                    // id that comes back is a new connection, and it is named
                    // again or not at all -- never by a leftover.
                    self.senders.remove(&session_id);
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
                VoiceEvent::LiveAudioOut { session_id, at_ms } => {
                    let gap = self.duplex_turn_gap_ms;
                    let backchannel = self.duplex_backchannel_max_ms;
                    if let Some(state) = self.live_sessions.get_mut(&session_id) {
                        // Never an action: the mark only lands in the open turn.
                        let _ = live_turns::step(
                            &mut state.turns,
                            TurnInput::AudioOut { at_ms },
                            gap,
                            backchannel,
                        );
                    }
                }
                VoiceEvent::LiveTick { session_id, now_ms } => {
                    self.on_live_tick(&session_id, now_ms, sink).await;
                }
                VoiceEvent::Live { session_id, event } => {
                    self.on_live_event(&session_id, event, sink).await;
                }
                VoiceEvent::Renewed {
                    session_id,
                    renewal_n,
                    renewed_at_ms,
                } => {
                    self.on_renewed(&session_id, renewal_n, renewed_at_ms, sink)
                        .await;
                }
                VoiceEvent::SeedAsk {
                    session_id,
                    reason,
                    renewal_n,
                } => {
                    // No words, no session check: the call may still be
                    // ringing (GH #1055), and the ask is what lets the colony
                    // answer before the model opens (D48).
                    let content = self.live_body(
                        "seed_ask",
                        &session_id,
                        json!({"reason": reason.as_str(), "renewal_n": renewal_n}),
                        json!([]),
                    );
                    self.emit(sink, content).await;
                }
                VoiceEvent::CallMark {
                    session_id,
                    elapsed_s,
                } => {
                    // D48, DK: the connection's timer, so only a live call
                    // gets here -- and not one whose session already closed:
                    // between that `Closed` and the end of its events the
                    // loop still runs, and a mark then would follow the
                    // call's last turn (review B-k1).
                    if self
                        .live_sessions
                        .get(&session_id)
                        .is_none_or(|state| state.closing)
                    {
                        return;
                    }
                    let content = self.live_body(
                        "call_mark",
                        &session_id,
                        json!({"elapsed_s": elapsed_s}),
                        json!([]),
                    );
                    self.emit(sink, content).await;
                }
                VoiceEvent::CallUsage {
                    session_id,
                    seconds,
                    last,
                    complete,
                } => {
                    self.emit_usage(&session_id, seconds, last, complete, sink)
                        .await;
                }
                VoiceEvent::SeedUnheld { session_id } => {
                    // Review K5: a curator that names the wrong call hears
                    // it, the way an `in_advise` for nobody is answered.
                    self.emit_error(
                        sink,
                        "unknown_session",
                        "in_seed: no session of this call is parked or held",
                        Some(&session_id),
                        None,
                    )
                    .await;
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

    fn held_push(message: Option<Box<Message>>, text: &str) -> HeldAdvise {
        HeldAdvise::Push {
            message,
            section_spoken: true,
            kind: AppendKind::Commentary,
            delegation_id: None,
            text: text.to_string(),
            spoken: true,
            takeover: None,
        }
    }

    /// Review kv-2 A-k1: past [`HELD_ADVISE_KEPT`] the oldest advice goes as a
    /// whole -- an `interrupt` is two pushes and one message, and its second
    /// half never stays behind without the first -- and `hold` hands the
    /// dropped message back for the dead letter.
    #[test]
    fn an_overfull_hold_drops_a_whole_interrupt_and_returns_its_message() {
        let mut state = LiveSessionState::new(Mode::Auto);
        let msg = Box::new(
            meclaw_core::MessageBuilder::new(Path::new("/advise"))
                .body(Body::Inline(json!({"messages": []})))
                .build(),
        );
        let path = &Path::new("/voice");
        let mut dropped = hold(
            path,
            "call-1",
            &mut state,
            held_push(Some(msg.clone()), "a"),
        );
        dropped.extend(hold(path, "call-1", &mut state, held_push(None, "a2")));
        for n in 0..HELD_ADVISE_KEPT - 2 {
            dropped.extend(hold(path, "call-1", &mut state, HeldAdvise::Renew));
            assert!(dropped.is_empty(), "nothing dropped at {n}");
        }
        dropped.extend(hold(path, "call-1", &mut state, HeldAdvise::Renew));
        assert_eq!(dropped.len(), 2, "both halves of the interrupt go");
        assert!(matches!(&dropped[0], HeldAdvise::Push { message: Some(m), .. } if m.id == msg.id));
        assert!(
            !state
                .held_advise
                .iter()
                .any(|h| matches!(h, HeldAdvise::Push { .. })),
            "no orphaned half stays held"
        );
    }

    /// D48 / LC2: the throttle of the `partial` lane needs both steps, the
    /// characters AND the time, and lets the first emission of a turn out.
    #[tokio::test(start_paused = true)]
    async fn partial_throttle_needs_both_steps() {
        let t0 = tokio::time::Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let knobs = (12, 250);
        assert!(partial_throttle_due(None, t0, 4, knobs), "the first leaves");
        assert!(
            !partial_throttle_due(Some((t0, 4)), ms(300), 12, knobs),
            "8 chars"
        );
        assert!(
            !partial_throttle_due(Some((t0, 4)), ms(249), 40, knobs),
            "249 ms"
        );
        assert!(
            partial_throttle_due(Some((t0, 4)), ms(250), 16, knobs),
            "both reached"
        );
        assert!(
            partial_throttle_due(Some((t0, 4)), t0, 5, (0, 0)),
            "unthrottled: every delta"
        );
    }
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
