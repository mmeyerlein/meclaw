//! The provider contract of the `voice` cell: what a speech-to-text and a
//! text-to-speech provider look like from the cell's side, and nothing about
//! any particular vendor. Wave voice-cell (2026-09-05).
//!
//! # No `thiserror` here
//!
//! The wave's contract was drafted with `#[derive(thiserror::Error)]` on the
//! two error enums. `meclaw-cells` does not depend on `thiserror` (only
//! `meclaw-colony` does), and a `Cargo.toml` change is not part of this wave —
//! so the `Display` and `std::error::Error` impls below are written out by
//! hand, with the message strings the contract named, character for character.
//! The established shape in this crate is the same (see
//! `crate::stdio_child::error`).

use meclaw_colony::io_liveness::IoLivenessMark;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::{mpsc, watch};

/// A boxed, sendable future — the provider traits return these so they stay
/// object-safe behind `Arc<dyn …>`.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The one sample encoding on the wire. Raw PCM, signed 16-bit little-endian.
///
/// # Why there is still only one (GH #619)
///
/// Telephony carries 8 kHz, and the obvious second variant is companded
/// G.711 — `mulaw` or `alaw`. Deepgram's own encoding list allows both on the
/// Flux endpoint ("Flux supports `linear16`, `linear32`, `mulaw`, `alaw`,
/// `opus`, and `ogg-opus` for non-containerized/raw audio",
/// <https://developers.deepgram.com/docs/encoding>), so the recogniser is not
/// what stops it. The EDGE is: FreeSWITCH's `mod_audio_stream` "attaches a
/// media bug and starts streaming audio (in L16 format) to the websocket
/// server" (<https://github.com/amigniter/mod_audio_stream>) and takes exactly
/// two rates, `8k` and `16k`. Nothing in this tree produces a companded frame,
/// and both reference implementations of the Flux protocol send `linear16` and
/// nothing else — LiveKit's `stt_v2.py` hardcodes `"encoding": "linear16"`,
/// pipecat's `flux/stt.py` documents its own parameter as `Must be
/// "linear16"`. So the native telephony path here is **`pcm_s16le` at
/// 8000 Hz**, and a second variant would be a wire nobody speaks.
///
/// What did change is that the sample size is read off the encoding
/// ([`AudioFormat::sample_bytes`]) instead of being the constant `2`: a later
/// variant is one match arm, and the frame-parity check follows it by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// Signed 16-bit little-endian PCM, interleaved (mono here, so no interleave).
    ///
    /// Named on the wire, explicitly: serde's `snake_case` would render this
    /// variant as `pcm_s16_le`, and the protocol says `pcm_s16le` — the same
    /// spelling every vendor in this wave uses.
    #[serde(rename = "pcm_s16le")]
    PcmS16Le,
}

impl Encoding {
    /// The encoding's name on the wire, as `hello` and `?encoding=` spell it.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Encoding::PcmS16Le => "pcm_s16le",
        }
    }

    /// The encoding a client named in its query string, or `None` for a name
    /// this version does not speak.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pcm_s16le" => Some(Encoding::PcmS16Le),
            _ => None,
        }
    }

    /// Bytes per sample in this encoding.
    pub const fn sample_bytes(&self) -> usize {
        match self {
            Encoding::PcmS16Le => 2,
        }
    }
}

/// An audio format as declared in the `hello` frame. The cell never converts
/// between two of these (R-V2): the client adapts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioFormat {
    /// Sample encoding.
    pub encoding: Encoding,
    /// Samples per second.
    pub sample_rate: u32,
    /// Channel count; always 1 in this wave.
    pub channels: u8,
}

impl AudioFormat {
    /// Mono PCM16 at the given rate.
    pub const fn pcm16_mono(sample_rate: u32) -> Self {
        Self {
            encoding: Encoding::PcmS16Le,
            sample_rate,
            channels: 1,
        }
    }
    /// Bytes per sample frame (all channels). 2 for mono PCM16.
    ///
    /// Read off the encoding rather than written as a constant: the parity
    /// check on the inbound socket is this number, so an encoding whose sample
    /// is not two bytes wide moves the check with it and not a line later.
    pub const fn frame_bytes(&self) -> usize {
        self.encoding.sample_bytes() * self.channels as usize
    }
}

/// The two deadlines every provider adapter is held to (hard rule 12).
///
/// They are params, not constants: `external` wraps each connect, each first
/// answer and each send, and `idle` is the per-socket deadline whose expiry is
/// the reconnect path rather than a standstill. Passed to a provider at
/// construction so the adapter never has to reach for a global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderTimeouts {
    /// Operation-timeout around a single provider I/O.
    pub external: std::time::Duration,
    /// Idle deadline for a provider socket with nothing on it.
    pub idle: std::time::Duration,
}

impl Default for ProviderTimeouts {
    /// The params defaults: 5 s per operation, 30 s of silence on a socket.
    fn default() -> Self {
        Self {
            external: std::time::Duration::from_millis(5000),
            idle: std::time::Duration::from_millis(30000),
        }
    }
}

/// What a speech-to-text session reports. One enum for every provider; a
/// provider that cannot produce a variant simply never sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    /// The provider heard speech begin (Flux `StartOfTurn`, OpenAI
    /// `speech_started`). Drives barge-in in `auto` mode.
    SpeechStarted,
    /// An interim transcript of the current turn, replacing the previous one.
    /// `eager` marks a preflight transcript (turn probably over, not certain).
    Partial {
        /// The interim transcript of the current turn.
        text: String,
        /// `true` for a preflight transcript below the end-of-turn threshold.
        eager: bool,
    },
    /// The provider decided the turn is over. Exactly one `turn` lane emission
    /// follows in `auto` mode.
    EndOfTurn {
        /// The final transcript of the turn that just ended.
        text: String,
    },
    /// The provider withdrew its last `EndOfTurn` (Flux `TurnResumed`). The
    /// cell does not un-emit; the continuation becomes the next turn.
    TurnResumed,
    /// A non-fatal provider message: something went wrong with one item and
    /// the session carries on. OpenAI reports a per-item transcription failure
    /// this way. It is reported, never swallowed, and it ends nothing.
    Warning {
        /// Human-readable cause. Never carries a credential.
        detail: String,
    },
    /// The provider closed the session on its side.
    Closed,
    /// The session failed; the connection task reconnects or reports.
    Failed {
        /// Human-readable cause. Never carries a credential.
        detail: String,
    },
}

/// Speech-to-text failures. Never carries a credential.
#[derive(Debug)]
pub enum SttError {
    /// The provider socket could not be opened.
    Connect(String),
    /// The provider refused the credentials it was given.
    Auth(String),
    /// The provider spoke something this adapter does not understand.
    Protocol(String),
    /// An A-timeout (rule 12) elapsed around a bounded provider operation.
    Timeout,
    /// The provider closed the session.
    Closed(String),
    /// This provider is not usable at all (not built, not configured).
    Unavailable(String),
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttError::Connect(e) => write!(f, "stt connect failed: {e}"),
            SttError::Auth(e) => write!(f, "stt rejected credentials: {e}"),
            SttError::Protocol(e) => write!(f, "stt protocol error: {e}"),
            SttError::Timeout => write!(f, "stt operation timed out"),
            SttError::Closed(e) => write!(f, "stt session closed: {e}"),
            SttError::Unavailable(e) => write!(f, "stt provider unavailable: {e}"),
        }
    }
}

impl std::error::Error for SttError {}

/// Text-to-speech failures. Never carries a credential.
#[derive(Debug)]
pub enum TtsError {
    /// The provider socket could not be opened.
    Connect(String),
    /// The provider refused the credentials it was given.
    Auth(String),
    /// The provider spoke something this adapter does not understand.
    Protocol(String),
    /// An A-timeout (rule 12) elapsed around a bounded provider operation.
    Timeout,
    /// The synthesis was cancelled — `cancel` went `true`, or the receiver went
    /// away. Not a fault.
    Cancelled,
    /// This provider is not usable at all (not built, not configured).
    Unavailable(String),
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtsError::Connect(e) => write!(f, "tts connect failed: {e}"),
            TtsError::Auth(e) => write!(f, "tts rejected credentials: {e}"),
            TtsError::Protocol(e) => write!(f, "tts protocol error: {e}"),
            TtsError::Timeout => write!(f, "tts operation timed out"),
            TtsError::Cancelled => write!(f, "tts synthesis cancelled"),
            TtsError::Unavailable(e) => write!(f, "tts provider unavailable: {e}"),
        }
    }
}

impl std::error::Error for TtsError {}

/// A speech-to-text provider. One instance per cell, shared by every connection.
pub trait SttProvider: Send + Sync + 'static {
    /// Provider name as it appears in `hello.stt` and in logs.
    fn name(&self) -> &'static str;
    /// The format a client is sent to when it asks for nothing; declared in
    /// `hello.audio_in` on such a connection.
    fn input_format(&self) -> AudioFormat;
    /// Every input rate this provider serves, its own included (GH #619).
    ///
    /// It is the discovery half of [`Self::negotiate_input`] and `GET /info`
    /// prints it, so a client learns what it may ask for by reading instead of
    /// by being refused. The default is the one rate the provider declares.
    fn input_rates(&self) -> Vec<u32> {
        vec![self.input_format().sample_rate]
    }
    /// The format this provider will run a session at when the client says it
    /// sends `sample_rate` — `None` when it cannot, which is a refused
    /// connection and never a conversion (R-V2).
    fn negotiate_input(&self, sample_rate: u32) -> Option<AudioFormat> {
        let mine = self.input_format();
        (sample_rate == mine.sample_rate).then_some(mine)
    }
    /// Run one session for one connection. `format` is what
    /// [`Self::negotiate_input`] agreed to for this connection, and `audio`
    /// carries raw chunks in it; the session ends when `audio` closes. Events
    /// go to `events`; a full channel blocks (backpressure, never drop). Call
    /// `liveness.mark_success()` after every successful provider round trip.
    /// Returns when the session is over; `Err` when it ended abnormally.
    fn run_session(
        &self,
        format: AudioFormat,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>>;
}

/// A text-to-speech provider. One instance per cell, shared by every connection.
pub trait TtsProvider: Send + Sync + 'static {
    /// Provider name as it appears in `hello.tts` and in logs.
    fn name(&self) -> &'static str;
    /// The format the cell sends to a client that asked for nothing; declared
    /// in `hello.audio_out` on such a connection.
    fn output_format(&self) -> AudioFormat;
    /// Every output rate this provider serves, its own included (GH #619).
    /// `GET /info` prints it beside [`SttProvider::input_rates`].
    fn output_rates(&self) -> Vec<u32> {
        vec![self.output_format().sample_rate]
    }
    /// The format this provider will synthesise at when the client asked for
    /// `sample_rate`, or `None` when it cannot.
    ///
    /// A `None` here is **not** a refused connection: what a client sends has
    /// to be understood, what it is sent it can merely dislike. The cell falls
    /// back to [`Self::output_format`] and declares that in `hello`, so the
    /// two directions of one connection may well run at two rates.
    fn negotiate_output(&self, sample_rate: u32) -> Option<AudioFormat> {
        let mine = self.output_format();
        (sample_rate == mine.sample_rate).then_some(mine)
    }
    /// Synthesize `text` into `audio` (chunks in `format`, which is what
    /// [`Self::negotiate_output`] agreed to for this connection). Stops as
    /// soon as `cancel` becomes `true` or the receiver is dropped, returning
    /// `Err(TtsError::Cancelled)`. `Ok(())` after the last chunk was sent.
    fn synthesize(
        &self,
        format: AudioFormat,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>>;
}

// ───────────────────────────── the third seam: a duplex provider

/// Which of the two voices a transcript fragment belongs to.
///
/// A duplex model transcribes BOTH sides of the conversation — it hears the
/// caller and it knows what it said itself — so a fragment without a speaker
/// would be half a sentence with no owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    /// The person on the other end.
    User,
    /// The model.
    Assistant,
}

/// Which append channel a piece of guidance travels on.
///
/// The three are not three names for one thing. `Commentary` is what the
/// caller should HEAR next (the model paraphrases it), `Thinking` is what the
/// model should KNOW without saying it, and `Instructions` changes how it
/// behaves for the rest of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendKind {
    /// Say this next, in your own words.
    Commentary,
    /// Know this; do not read it out.
    Thinking,
    /// Behave like this from now on.
    Instructions,
}

/// The four channels one duplex session runs on.
///
/// Audio in one direction, audio in the other, the model's events, and the
/// guidance the colony pushes at it. All four are bounded: backpressure is the
/// design (ADR-0023 § Consequences), because a queue that grows is a queue that
/// is already late.
pub struct DuplexSession {
    /// Client → model. **One client frame is one item, never merged**: the
    /// wave measured that a buffer here is latency nobody asked for (R-L4).
    pub audio_in: mpsc::Receiver<Vec<u8>>,
    /// Model → client. One decoded chunk is one item, never merged.
    pub audio_out: mpsc::Sender<Vec<u8>>,
    /// Everything the model said about itself. A full channel blocks.
    pub events: mpsc::Sender<DuplexEvent>,
    /// What the colony tells the model between two turns.
    pub control: mpsc::Receiver<DuplexControl>,
}

/// What a duplex session reports. One enum for every provider; a provider that
/// cannot produce a variant simply never sends it.
#[derive(Debug, Clone, PartialEq)]
pub enum DuplexEvent {
    /// The session is open and the model answers to this identity.
    Started {
        /// The provider's own session identity, for logs and for the close.
        session_id: String,
    },
    /// A transcript fragment on the **model's** timeline.
    ///
    /// `start_ms`/`end_ms` are the model's clock, not the arrival time — the
    /// two differ by the network, and a turn cut on arrival time is a turn cut
    /// by the network (R-25-9).
    Transcript {
        /// Whose words these are.
        speaker: Speaker,
        /// The fragment itself; it EXTENDS what came before rather than
        /// replacing it.
        delta: String,
        /// Where the fragment starts on the model's clock.
        start_ms: u64,
        /// Where it ends.
        end_ms: u64,
    },
    /// The model handed a piece of work to the client side (R-25-4:
    /// `delegation.type: client`, so the backend is this colony).
    DelegationCreated {
        /// The delegation's identity; a later append may name it.
        delegation_id: String,
        /// Where on the model's clock it was created.
        offset_ms: u64,
    },
    /// One append was taken up, and where it landed on the model's clock.
    Appended {
        /// Which channel it travelled on.
        kind: AppendKind,
        /// The `event_id` the append carried, echoed back.
        event_id: String,
        /// Where the model placed it.
        start_ms: u64,
        /// Where it ends.
        end_ms: u64,
    },
    /// The running meter. Sent about every 15 s by a hosted model.
    Usage {
        /// Session seconds spent so far.
        seconds: f64,
        /// How full the context window is, where the provider says.
        usage_ratio: Option<f64>,
    },
    /// The model's ear is closed; audio sent now is not heard.
    Muted,
    /// The ear is open again.
    Unmuted,
    /// Something went wrong with one item and the session carries on. It is
    /// reported, never swallowed, and it ends nothing.
    Warning {
        /// Human-readable cause. Never carries a credential.
        detail: String,
    },
    /// The session is over and this is the final meter reading.
    Closed {
        /// Why it ended, in the provider's own words.
        reason: String,
        /// The session's total, as the provider counted it.
        usage_seconds: f64,
    },
}

/// What the colony tells a running duplex session.
#[derive(Debug, Clone, PartialEq)]
pub enum DuplexControl {
    /// Push one piece of guidance into the session.
    Append {
        /// Which channel it travels on.
        kind: AppendKind,
        /// The identity the `Appended` answer will carry.
        event_id: String,
        /// The delegation this answers, where it answers one.
        delegation_id: Option<String>,
        /// The text itself.
        content: String,
    },
    /// Stop listening.
    Mute,
    /// Listen again.
    Unmute,
    /// End the session in an orderly way.
    Close,
}

/// Duplex failures. Never carries a credential.
#[derive(Debug)]
pub enum DuplexError {
    /// The provider socket could not be opened.
    Connect(String),
    /// The provider refused the credentials it was given.
    Auth(String),
    /// The provider spoke something this adapter does not understand.
    Protocol(String),
    /// An A-timeout (rule 12) elapsed around a bounded provider operation.
    Timeout,
    /// The provider closed the session on its side, mid-conversation.
    Closed(String),
    /// This provider is not usable at all (not built, not configured).
    Unavailable(String),
}

impl std::fmt::Display for DuplexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DuplexError::Connect(e) => write!(f, "duplex connect failed: {e}"),
            DuplexError::Auth(e) => write!(f, "duplex rejected credentials: {e}"),
            DuplexError::Protocol(e) => write!(f, "duplex protocol error: {e}"),
            DuplexError::Timeout => write!(f, "duplex operation timed out"),
            DuplexError::Closed(e) => write!(f, "duplex session closed: {e}"),
            DuplexError::Unavailable(e) => write!(f, "duplex provider unavailable: {e}"),
        }
    }
}

impl std::error::Error for DuplexError {}

/// A duplex provider: audio in, audio out, and the model's events, on one
/// session. One instance per cell, shared by every connection.
///
/// # Why this is a third trait and not a third cell type
///
/// ADR-0023 named its own way out: "a speech-to-speech provider behind the
/// traits … is this ADR being built out". This is that clause taken up.
/// Audio still terminates in the I/O half of the `voice` cell — no sample
/// becomes a message, no model becomes an actor — and what changes is only
/// that ONE socket now carries both directions instead of two sockets carrying
/// one each.
///
/// # One format for both directions
///
/// A cascade has two formats, because the recogniser and the synthesiser are
/// two vendors with two opinions. A duplex model has one: its `audio.format`
/// applies to what it hears and to what it says. So there is a single
/// [`Self::format`] here rather than an input and an output one, and
/// `negotiate` answers for both directions at once.
///
/// # When the client goes away
///
/// `audio_in` closing IS the end of the call. The provider then sends its own
/// close, waits for the far side's acknowledgement for at most
/// `close_grace_ms`, emits [`DuplexEvent::Closed`] and returns `Ok(())`. An
/// `Err` is a disturbance — a dead provider, a refused credential, a protocol
/// this adapter does not speak — and never the ordinary end. There is no
/// `Failed` event: the return value is the verdict, exactly as with
/// [`SttProvider`].
pub trait DuplexProvider: Send + Sync + 'static {
    /// Provider name as it appears in `hello.duplex`, `GET /info` and in logs.
    fn name(&self) -> &'static str;
    /// The one format this provider runs at, in both directions.
    fn format(&self) -> AudioFormat;
    /// Every rate this provider serves, its own included. `GET /info` prints
    /// it, so a client learns what it may ask for by reading.
    fn rates(&self) -> Vec<u32> {
        vec![self.format().sample_rate]
    }
    /// The format this provider will run a session at when the client says it
    /// sends `sample_rate` — `None` is a refused connection and never a
    /// conversion (R-V2).
    fn negotiate(&self, sample_rate: u32) -> Option<AudioFormat> {
        self.rates()
            .contains(&sample_rate)
            .then(|| AudioFormat::pcm16_mono(sample_rate))
    }
    /// Run one session for one connection. `format` is what
    /// [`Self::negotiate`] agreed to. Call `liveness.mark_success()` after
    /// every successful provider round trip. Returns when the session is over;
    /// `Err` when it ended abnormally.
    fn run_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>>;
}
