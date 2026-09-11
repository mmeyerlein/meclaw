//! The `voice` cell (wave voice-cell, 2026-09-05): a channel bridge that takes
//! audio in over a WebSocket and puts text turns into the topology, and takes
//! assistant text out of the topology and puts speech back on the same socket.
//!
//! It follows the `web` cell position for position — long-running, dual-task,
//! knowing no topology and minting ingress context at the declared edge of the
//! level it sits on. One WebSocket connection is one client.
//!
//! **Since `voice@2.0.0` it owns no listener.** `params.mount` is required and
//! is the whole door: the I/O half registers the name once, at the top of its
//! life, and the colony's one listener hands it the streams whose first path
//! segment is that name (ADR-0031, which supersedes ADR-0014). What the cell
//! serves on a handed stream is its own three routes under the mount —
//! `/<mount>/ws`, `/<mount>/info`, `/<mount>/` — plus a topic on a display's
//! socket, which is the same admission without a socket of its own (GH #643).
//! A mount that moves in a `params` update is written down and read on the
//! next life (ruling O-P-2); there is no rebind, because there is nothing to
//! bind.
//!
//! Two things are structural rather than stylistic. **Audio terminates in the
//! I/O half** — a sample never enters a mailbox, because a mailbox is a queue
//! and speech is a deadline. And **the cell never resamples** (R-V2): the rate
//! the speech-to-text provider wants and the rate the text-to-speech provider
//! emits are both declared in the `hello` frame, and the client adapts.

pub mod cell;
pub mod connection;
pub mod contract;
pub mod factory;
pub mod io;
pub mod link;
pub mod params;
pub mod providers;
pub mod service;
pub mod speech_text;
pub mod testpage;
pub mod turns;
pub mod wire;

pub use cell::{VoiceEvent, VoiceReconfig};
pub use contract::{
    AudioFormat, BoxFuture, Encoding, ProviderTimeouts, SttError, SttEvent, SttProvider, TtsError,
    TtsProvider,
};
pub use factory::VoiceCellFactory;
pub use io::VoiceIo;
pub use link::{ClientGone, ClientLink, Door, Incoming, LinkSink, LinkStream, Outgoing};
pub use params::VoiceParams;
pub use speech_text::to_speech;
pub use wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason, WireErrorCode};
