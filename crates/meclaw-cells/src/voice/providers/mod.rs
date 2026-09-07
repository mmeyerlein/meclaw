//! The provider registry of the `voice` cell: one function per direction that
//! turns a parsed params sub-object into a live adapter.
//!
//! A third adapter is a new file in this directory plus one match arm here —
//! that is the whole extension surface, and it is deliberately this small.

pub mod cartesia;
pub mod deepgram;
pub mod echo;
pub mod elevenlabs;
pub mod openai_stt;
pub mod openai_tts;

use crate::voice::contract::{ProviderTimeouts, SttProvider, TtsProvider};
use crate::voice::params::{SttParams, TtsParams};
use std::sync::Arc;

/// Build the speech-to-text adapter named by `p`, held to `t`.
///
/// One instance per cell, shared by every connection — the providers hold
/// configuration, never per-connection state.
pub fn build_stt(p: &SttParams, t: ProviderTimeouts) -> Result<Arc<dyn SttProvider>, String> {
    Ok(match p {
        SttParams::Echo => Arc::new(echo::EchoStt::new().with_timeouts(t)),
        SttParams::Deepgram(dp) => {
            Arc::new(deepgram::DeepgramFluxStt::new(dp.clone()).with_timeouts(t))
        }
        SttParams::Openai(op) => {
            Arc::new(openai_stt::OpenAiTranscriptionStt::new(op.clone()).with_timeouts(t))
        }
    })
}

/// Build the text-to-speech adapter named by `p`, held to `t`.
pub fn build_tts(p: &TtsParams, t: ProviderTimeouts) -> Result<Arc<dyn TtsProvider>, String> {
    Ok(match p {
        TtsParams::Cartesia(cp) => {
            Arc::new(cartesia::CartesiaTts::new(cp.clone()).with_timeouts(t))
        }
        TtsParams::Openai(op) => Arc::new(openai_tts::OpenAiTts::new(op.clone()).with_timeouts(t)),
        TtsParams::Elevenlabs(ep) => {
            Arc::new(elevenlabs::ElevenLabsTts::new(ep.clone()).with_timeouts(t))
        }
    })
}
