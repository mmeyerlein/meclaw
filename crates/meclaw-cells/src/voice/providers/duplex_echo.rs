//! The `echo` duplex provider: a duplex session with no model in it.
//!
//! It exists for the reason [`crate::voice::providers::echo`] exists, and
//! ADR-0023 said it out loud before either of them was written: "a trait with
//! one implementation is a shape borrowed from that implementation". A second
//! implementation is what turns the seam into a contract.
//!
//! What it does is the whole of the contract and nothing else: every item that
//! comes in goes straight back out, byte for byte, in order; an append is
//! answered AND mirrored as assistant text, so the connection's `speak_end`
//! heuristic can be measured without spending a cent on a model; a mute
//! swallows the input until an unmute; and the end of the audio is the end of
//! the session. What the connection has no room for yet is owed, never
//! dropped, and the guidance is read while it waits (see `run_session`).
//!
//! Unlike the cascade echo, the loopback IS here rather than in the connection
//! task. There it would have measured a channel pair for nothing; here the
//! channel pair is the contract under test.
//!
//! What it does NOT carry is the session's clock (R-L7). The tick that feeds
//! the turn machine and the delegation deadline sits in the connection, ABOVE
//! this trait, so both implementations get it and neither writes it: a
//! `DuplexProvider` says what the model said, and what time it is is not
//! something a model says. The `Usage` events below are a plausible meter for
//! a loopback and nothing hangs off them any more.

use crate::voice::contract::{
    AudioFormat, BoxFuture, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts, Speaker,
};
use meclaw_colony::io_liveness::IoLivenessMark;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::Instant;

/// The rates the loopback negotiates (public: `params.rs` refuses any other
/// one by NAME rather than letting a session open at a rate nothing serves).
/// The set is: every rate a duplex model in this tree
/// serves, plus the telephony one. A calibration exists to be compared against
/// the real session that follows it, so the set is theirs and 8000 is on it —
/// the rate a call actually is, before any model is blamed.
pub const ECHO_DUPLEX_RATES: [u32; 3] = [8_000, 16_000, 24_000];

/// How many items make one meter reading. 50 frames of 20 ms is one second of
/// audio, which is the granularity a real session's `usage.updated` has.
const ITEMS_PER_USAGE: u64 = 50;

/// How long one item is assumed to be, in seconds — 20 ms, the frame length
/// this wire runs at (`params.audio_out_frame_ms`).
const SECONDS_PER_ITEM: f64 = 0.02;

/// Audio in, the same audio out — and every event the contract has a shape for.
pub struct EchoDuplex {
    /// The rate this instance declares; `params.duplex.sample_rate`.
    sample_rate: u32,
    /// The two deadlines this adapter is held to. Kept because
    /// [`Self::with_timeouts`] is the shape every provider has; neither can
    /// fire here, for the reason [`EchoDuplex::run_session`] gives.
    timeouts: ProviderTimeouts,
}

impl EchoDuplex {
    /// A loopback duplex session at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// Hold this adapter to the cell's two deadlines (hard rule 12).
    pub fn with_timeouts(mut self, t: ProviderTimeouts) -> Self {
        self.timeouts = t;
        self
    }
}

impl DuplexProvider for EchoDuplex {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.sample_rate)
    }

    fn rates(&self) -> Vec<u32> {
        ECHO_DUPLEX_RATES.to_vec()
    }

    /// Run the loopback until the audio ends or a `Close` arrives.
    ///
    /// **Neither deadline applies, and that is not an oversight.** Rule 12's
    /// operation timeout wraps provider I/O and this session performs none —
    /// the only thing it ever waits on is the client's microphone, and a
    /// timeout around that would be a deadline on a human being quiet.
    fn run_session(
        &self,
        _format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let _timeouts = self.timeouts;
        let DuplexSession {
            mut audio_in,
            audio_out,
            events,
            mut control,
        } = session;
        Box::pin(async move {
            let started_at = Instant::now();
            let session_id = meclaw_core::Uuid::now_v7().to_string();
            // The model's clock, for a model that has none: milliseconds since
            // the session opened. A real provider stamps its own; what matters
            // to every reader of these numbers is that they are monotonic and
            // are NOT arrival times of frames.
            let clock = move || started_at.elapsed().as_millis() as u64;
            if events
                .send(DuplexEvent::Started {
                    session_id: session_id.clone(),
                })
                .await
                .is_err()
            {
                return Ok(());
            }

            let mut muted = false;
            let mut items: u64 = 0;
            // What the session owes the connection and could not hand over yet
            // (GH #1511). The connection hands guidance over
            // with `control_tx.send(..).await` and reads this session's outlets
            // only between its own sends, so a loopback that waited on a full
            // outlet and read no guidance meanwhile waited on the very
            // connection that waited on it. Nothing is dropped instead -- a
            // loopback that dropped would measure the drop, not the wire:
            // - `owed` holds the events, in order. It grows only with the
            //   guidance read (an append is two events, a mute one) and by one
            //   meter reading per `ITEMS_PER_USAGE` echoed frames, and while a
            //   meter reading waits in it the audio is not read (`usage_owed`).
            // - `held` is the one echoed frame the audio outlet had no room
            //   for; while it waits the audio is not read either, so the
            //   backpressure on the client stays what it was.
            let mut owed: VecDeque<DuplexEvent> = VecDeque::new();
            let mut usage_owed: usize = 0;
            let mut held: Option<Vec<u8>> = None;
            let reason = loop {
                tokio::select! {
                    // `biased` so a control frame is acted on before the audio
                    // that is already queued behind it — a mute that arrives
                    // late is a mute that did not happen. Guidance is read even
                    // while an outlet is full; the audio is not.
                    biased;

                    cmd = control.recv() => match cmd {
                        Some(DuplexControl::Append { kind, event_id, delegation_id, content }) => {
                            let _ = delegation_id;
                            let at = clock();
                            owed.push_back(DuplexEvent::Appended { kind, event_id, start_ms: at, end_ms: at });
                            // Mirrored as assistant text, which is what makes
                            // the connection's `speak_end` heuristic (OR-L19)
                            // measurable without a model: the heuristic waits
                            // for assistant fragments to stop, and here they
                            // stop as soon as the mirror is done.
                            owed.push_back(DuplexEvent::Transcript {
                                speaker: Speaker::Assistant,
                                delta: content,
                                start_ms: at,
                                end_ms: at,
                            });
                        }
                        Some(DuplexControl::Mute) => {
                            muted = true;
                            owed.push_back(DuplexEvent::Muted);
                        }
                        Some(DuplexControl::Unmute) => {
                            muted = false;
                            owed.push_back(DuplexEvent::Unmuted);
                        }
                        Some(DuplexControl::Close) => break "close_requested",
                        // The loopback opens no model, so it has nothing a
                        // seed could open with.
                        Some(DuplexControl::Seed { .. }) => {}
                        // The connection let the control channel go; the audio
                        // is the session and it is still open.
                        None => break "close_requested",
                    },

                    permit = events.reserve(), if !owed.is_empty() => match (permit, owed.pop_front()) {
                        (Ok(permit), Some(event)) => {
                            if matches!(event, DuplexEvent::Usage { .. }) {
                                usage_owed -= 1;
                            }
                            permit.send(event);
                        }
                        (Ok(_), None) => {}
                        (Err(_), _) => break "handler_gone",
                    },

                    permit = audio_out.reserve(), if held.is_some() => match permit {
                        Ok(permit) => {
                            if let Some(bytes) = held.take() {
                                permit.send(bytes);
                                items += 1;
                                usage_owed += owe_usage(&mut owed, items);
                            }
                        }
                        Err(_) => break "client_gone",
                    },

                    frame = audio_in.recv(), if held.is_none() && usage_owed == 0 => match frame {
                        // The client went away: `audio_in` closing IS the end
                        // of the call (contract § 1.1).
                        None => break "close_requested",
                        Some(bytes) => {
                            // Audio arriving IS the healthy round trip here —
                            // with no provider to answer, the supervisor would
                            // otherwise watch a session that never proved it
                            // was alive.
                            liveness.mark_success();
                            if muted {
                                continue;
                            }
                            // Held, never dropping: a full outlet keeps the
                            // frame (see `held`) and stops the audio until it
                            // is out.
                            match audio_out.try_send(bytes) {
                                Ok(()) => {
                                    items += 1;
                                    usage_owed += owe_usage(&mut owed, items);
                                }
                                Err(TrySendError::Full(bytes)) => held = Some(bytes),
                                Err(TrySendError::Closed(_)) => break "client_gone",
                            }
                        }
                    },
                }
            };

            // What is still owed goes out before the close, in order; the
            // session waits for room as long as the connection lives, and
            // keeps taking what it is handed meanwhile (see `draining`).
            if reason != "handler_gone" {
                for event in owed {
                    if draining(events.send(event), &mut control, &mut audio_in)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            if let (Some(bytes), true) = (held, reason != "client_gone") {
                let _ = draining(audio_out.send(bytes), &mut control, &mut audio_in).await;
            }
            let closed = DuplexEvent::Closed {
                reason: reason.to_string(),
                usage_seconds: items as f64 * SECONDS_PER_ITEM,
            };
            let _ = draining(events.send(closed), &mut control, &mut audio_in).await;
            Ok(())
        })
    }
}

/// Owes one meter reading when `items` echoed frames make one; returns how
/// many it added (0 or 1).
fn owe_usage(owed: &mut VecDeque<DuplexEvent>, items: u64) -> usize {
    if !items.is_multiple_of(ITEMS_PER_USAGE) {
        return 0;
    }
    owed.push_back(DuplexEvent::Usage {
        seconds: items as f64 * SECONDS_PER_ITEM,
        usage_ratio: None,
    });
    1
}

/// Waits for `send` to the connection while still taking what the connection
/// hands over (GH #1511; the same wait as `gpt_live`'s).
///
/// Only used where the session is ending: guidance and caller audio that
/// arrive meanwhile have no session left to reach and are dropped. What
/// matters is that they are TAKEN -- a session that stopped reading `control`
/// or `audio_in` while it waited on an outlet waited on the very connection
/// that waited on it.
async fn draining<F: std::future::Future>(
    send: F,
    control: &mut mpsc::Receiver<DuplexControl>,
    audio_in: &mut mpsc::Receiver<Vec<u8>>,
) -> F::Output {
    tokio::pin!(send);
    let mut control_open = true;
    let mut audio_open = true;
    loop {
        tokio::select! {
            biased;
            out = &mut send => return out,
            command = control.recv(), if control_open => {
                if command.is_none() {
                    control_open = false;
                }
            }
            chunk = audio_in.recv(), if audio_open => {
                if chunk.is_none() {
                    audio_open = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the `hello` frame and `GET /info` are built from.
    #[test]
    fn the_loopback_declares_one_format_for_both_directions() {
        let p = EchoDuplex::new(16_000);
        assert_eq!(p.name(), "echo");
        assert_eq!(p.format(), AudioFormat::pcm16_mono(16_000));
        assert_eq!(p.rates(), vec![8_000, 16_000, 24_000]);
        assert_eq!(p.negotiate(24_000), Some(AudioFormat::pcm16_mono(24_000)));
        assert_eq!(
            p.negotiate(44_100),
            None,
            "a rate nobody serves is a refused connection, never a conversion"
        );
    }
}
