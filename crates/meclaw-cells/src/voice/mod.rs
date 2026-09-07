//! The `voice` cell (wave voice-cell, 2026-09-05): a channel bridge that takes
//! audio in over a WebSocket and puts text turns into the topology, and takes
//! assistant text out of the topology and puts speech back on the same socket.
//!
//! It follows the `web` cell position for position — long-running, dual-task,
//! owner of its own listener (`params.port` / `params.bind`, loopback by
//! default), knowing no topology and minting ingress context at the declared
//! edge of the level it sits on. One WebSocket connection is one client.
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
pub use params::VoiceParams;
pub use speech_text::to_speech;
pub use wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason, WireErrorCode};
