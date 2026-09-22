//! The provider registry of the `voice` cell: one function per direction that
//! turns a parsed params sub-object into a live adapter.
//!
//! A third adapter is a new file in this directory plus one match arm here —
//! that is the whole extension surface, and it is deliberately this small.

pub mod cartesia;
pub mod deepgram;
pub mod duplex_echo;
pub mod echo;
pub mod elevenlabs;
pub mod gpt_live;
pub mod openai_stt;
pub mod openai_tts;

use crate::voice::contract::{DuplexProvider, ProviderTimeouts, SttProvider, TtsProvider};
use crate::voice::params::{DuplexParams, SttParams, TtsParams};
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

/// Build the duplex adapter named by `p`, held to `t`.
///
/// The third direction of the registry, and the same shape as the other two:
/// one instance per cell, shared by every connection, holding configuration and
/// never per-connection state. A cell in duplex mode has this and no cascade;
/// the two are mutually exclusive in `params` (contract § 1.2).
pub fn build_duplex(
    p: &DuplexParams,
    t: ProviderTimeouts,
) -> Result<Arc<dyn DuplexProvider>, String> {
    Ok(match p {
        DuplexParams::Echo(ep) => {
            Arc::new(duplex_echo::EchoDuplex::new(ep.sample_rate).with_timeouts(t))
        }
        DuplexParams::GptLive(gp) => {
            Arc::new(gpt_live::GptLiveDuplex::new(gp.clone()).with_timeouts(t))
        }
    })
}
