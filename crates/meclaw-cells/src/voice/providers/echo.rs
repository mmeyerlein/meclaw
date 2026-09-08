//! The `echo` speech-to-text provider: a wire with no model in it.
//!
//! Wave voice-cell (2026-09-05), strand t5. Its whole purpose is calibration.
//! With `stt.provider: "echo"` every binary frame comes back byte-identical and
//! nothing else is sent, so the round trip a client measures is the socket, the
//! browser's audio path and the transport — and nothing else. When the first
//! real turn later takes 900 ms, this number says how much of that was never
//! the model's fault. The July telephony work spent thirty test calls on an ASR
//! that turned out to be innocent; five clicks against an echo agent would have
//! said so in a minute.
//!
//! The loopback itself is **not** here. It lives in the connection task, which
//! recognises this provider by name and hands the frame straight back; audio
//! routed through a channel pair only to come back unchanged would have
//! measured the channel pair too. What this provider does is the other half of
//! the contract: consume the audio stream until the client closes it, mark
//! liveness so the supervisor sees a healthy connection, and report no events
//! at all — an echo session has no transcript, no turn and no failure to tell
//! anyone about.

use crate::voice::contract::{
    AudioFormat, BoxFuture, ProviderTimeouts, SttError, SttEvent, SttProvider,
};
use meclaw_colony::io_liveness::IoLivenessMark;
use tokio::sync::mpsc;

/// The rate the echo wire declares in `hello.audio_in` — and, being a loopback,
/// in `hello.audio_out` too. 16 kHz because that is what a browser capture
/// graph downsamples to most cheaply and what the fixture WAVs already are; the
/// echo provider converts nothing, so the number is a declaration, not a
/// conversion target (R-V2).
const ECHO_SAMPLE_RATE: u32 = 16_000;

/// The rates a client may negotiate against the loopback (GH #619) -- the union
/// of what the recognition providers in this tree serve, so a calibration can
/// be run at the rate the measurement that follows it will use.
const ECHO_SAMPLE_RATES: [u32; 5] = [8_000, ECHO_SAMPLE_RATE, 24_000, 44_100, 48_000];

/// Audio in, the same audio out.
pub struct EchoStt {
    /// The two deadlines this adapter is held to. Kept because
    /// [`Self::with_timeouts`] is the shape every provider has; see
    /// [`EchoStt::run_session`] for why neither can fire here.
    timeouts: ProviderTimeouts,
}

impl EchoStt {
    /// An echo provider. It has nothing to configure but its deadlines — a knob
    /// here would be a knob on the calibration itself.
    pub fn new() -> Self {
        Self {
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// Hold this adapter to the cell's two deadlines (hard rule 12).
    pub fn with_timeouts(mut self, t: ProviderTimeouts) -> Self {
        self.timeouts = t;
        self
    }
}

impl Default for EchoStt {
    fn default() -> Self {
        Self::new()
    }
}

impl SttProvider for EchoStt {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(ECHO_SAMPLE_RATE)
    }

    /// Every rate a recognition provider in this tree runs at (GH #619).
    ///
    /// A loopback converts nothing, so it could take any rate at all -- but a
    /// calibration exists to be COMPARED against the real provider that
    /// follows it, and a rate none of them serves has nothing to be compared
    /// with. So the list is theirs, and 8000 is on it: calibrating a telephony
    /// edge at the rate a call actually is, before any model is blamed, is
    /// exactly what this provider is for. What `GET /info` names and what
    /// [`Self::negotiate_input`] accepts are the same set, deliberately.
    fn input_rates(&self) -> Vec<u32> {
        ECHO_SAMPLE_RATES.to_vec()
    }

    fn negotiate_input(&self, sample_rate: u32) -> Option<AudioFormat> {
        ECHO_SAMPLE_RATES
            .contains(&sample_rate)
            .then(|| AudioFormat::pcm16_mono(sample_rate))
    }

    /// Drain the audio until the connection closes it, and say nothing.
    ///
    /// **Neither deadline applies, and that is not an oversight.** Rule 12's
    /// operation timeout wraps provider I/O; this session performs none — the
    /// only thing it waits on is the client's microphone. A timeout around that
    /// would be a deadline on a human being quiet, and the idle deadline exists
    /// to reconnect a provider socket that does not exist here.
    fn run_session(
        &self,
        _format: AudioFormat,
        mut audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let _timeouts = self.timeouts;
        Box::pin(async move {
            // Held rather than dropped: a closed event channel is how a session
            // says it is over, and this one is not over until the audio stops.
            let _events = events;
            while let Some(chunk) = audio.recv().await {
                if !chunk.is_empty() {
                    // Audio arriving IS the healthy round trip here — with no
                    // provider to answer, the supervisor would otherwise watch
                    // a connection that never proved it was alive.
                    liveness.mark_success();
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The session consumes every chunk, reports nothing, and ends when the
    /// audio ends. All three halves matter — an event on this channel would
    /// reach the turn automaton as a transcript nobody spoke.
    #[tokio::test]
    async fn echo_stt_drains_and_reports_nothing() {
        let (audio_tx, audio_rx) = mpsc::channel(8);
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let session = EchoStt::new().run_session(
            AudioFormat::pcm16_mono(16_000),
            audio_rx,
            events_tx,
            IoLivenessMark::disabled(),
        );

        let runner = tokio::spawn(session);
        for i in 0..4u8 {
            audio_tx
                .send(vec![i; 640])
                .await
                .expect("the session must keep consuming");
        }
        drop(audio_tx);

        let ended = tokio::time::timeout(Duration::from_secs(30), runner)
            .await
            .expect("the session must end when the audio does")
            .expect("the session task must not panic");
        assert!(ended.is_ok(), "a drained echo session ends cleanly");
        assert!(
            events_rx.try_recv().is_err(),
            "an echo session has no transcript to report"
        );
    }

    /// What the `hello` frame is built from.
    #[test]
    fn echo_declares_mono_pcm16_at_16k() {
        let provider = EchoStt::new();
        assert_eq!(provider.name(), "echo");
        assert_eq!(
            provider.input_format(),
            AudioFormat::pcm16_mono(ECHO_SAMPLE_RATE)
        );
        assert_eq!(provider.input_format().frame_bytes(), 2);
    }
}
