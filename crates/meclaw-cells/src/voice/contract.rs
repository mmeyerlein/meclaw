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
    pub const fn frame_bytes(&self) -> usize {
        2 * self.channels as usize
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
    /// The format the client must send; declared in `hello.audio_in`.
    fn input_format(&self) -> AudioFormat;
    /// Run one session for one connection. `audio` carries raw chunks in
    /// `input_format`; the session ends when `audio` closes. Events go to
    /// `events`; a full channel blocks (backpressure, never drop). Call
    /// `liveness.mark_success()` after every successful provider round trip.
    /// Returns when the session is over; `Err` when it ended abnormally.
    fn run_session(
        &self,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>>;
}

/// A text-to-speech provider. One instance per cell, shared by every connection.
pub trait TtsProvider: Send + Sync + 'static {
    /// Provider name as it appears in `hello.tts` and in logs.
    fn name(&self) -> &'static str;
    /// The format the cell sends to the client; declared in `hello.audio_out`.
    fn output_format(&self) -> AudioFormat;
    /// Synthesize `text` into `audio` (chunks in `output_format`). Stops as
    /// soon as `cancel` becomes `true` or the receiver is dropped, returning
    /// `Err(TtsError::Cancelled)`. `Ok(())` after the last chunk was sent.
    fn synthesize(
        &self,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>>;
}
