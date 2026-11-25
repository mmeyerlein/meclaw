//! How the model's audio went over one duplex call (#1637,
//! `params.emit_audio_report`).
//!
//! Two ends of the same stream measure on their own: the provider adapter how
//! each chunk arrived ([`AudioArrivals`], one per provider session), the way out
//! what the edge had to play ([`crate::voice::out_lead`], one per connection).
//! Neither knows the call. The connection does: it collects the sessions here
//! -- the first, every renewal, every recovery -- and sends one report at the end
//! of the call. Numbers and times only, never audio and never a word of it.

use crate::voice::contract::AudioArrivals;
use crate::voice::out_lead::Gap;
use meclaw_core::serde_json::{Map, Value, json};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// The account of one call's audio, as the `audio_report` lane carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioReport {
    /// When the call's duplex session began, in Unix ms.
    pub started_epoch_ms: u64,
    /// When the connection ended it, in Unix ms, ahead of the wait for the
    /// provider's close.
    pub ended_epoch_ms: u64,
    /// How the audio arrived, over every provider session of the call.
    pub arrivals: AudioArrivals,
    /// The holes at the way out, oldest first.
    pub gaps: Vec<ReportedGap>,
    /// How many more there were than the list takes.
    pub gaps_beyond_cap: u64,
    /// The longest an event of this connection waited on its way to the
    /// handler. Put in by the forwarder as the report passes it
    /// ([`crate::voice::VoiceEvent`]); `None` where the session was given up
    /// or taken over by another connection before the report.
    pub forward_wait_max_ms: Option<u64>,
    /// The longest a frame of audio waited for the client to take it.
    pub send_wait_max_ms: u64,
    /// The lead the call ran with (`params.audio_out_lead_ms`).
    pub lead_ms: u32,
}

/// One hole at the way out, see [`Gap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedGap {
    /// When the chunk that ended it was sent, in Unix ms.
    pub until_epoch_ms: u64,
    /// How long the line had been empty by then, in ms.
    pub length_ms: u64,
}

impl From<&Gap> for ReportedGap {
    fn from(gap: &Gap) -> Self {
        Self {
            until_epoch_ms: gap.until.epoch_ms,
            length_ms: whole_ms(gap.length),
        }
    }
}

impl AudioReport {
    /// One provider session of the call: counts add up, the list goes on, a
    /// longest is the longest of all.
    fn add(&mut self, session: AudioArrivals) {
        let call = &mut self.arrivals;
        call.chunks += session.chunks;
        call.late.extend(session.late);
        for (sum, n) in call.lateness.iter_mut().zip(session.lateness) {
            *sum += n;
        }
        call.late_max_ms = call.late_max_ms.max(session.late_max_ms);
        call.held_max_ms = call.held_max_ms.max(session.held_max_ms);
        call.send_wait_max_ms = call.send_wait_max_ms.max(session.send_wait_max_ms);
    }

    /// The fields of the message body, beside its empty `messages`. `model` is
    /// the model the cell runs on, `null` where its provider has none.
    pub fn body(&self, model: Option<&str>) -> Map<String, Value> {
        let late: Vec<Value> = (self.arrivals.late.iter())
            .map(|l| json!({"at_epoch_ms": l.at_epoch_ms, "late_ms": l.late_ms}))
            .collect();
        let gaps: Vec<Value> = (self.gaps.iter())
            .map(|g| json!({"until_epoch_ms": g.until_epoch_ms, "length_ms": g.length_ms}))
            .collect();
        [
            ("started_epoch_ms", json!(self.started_epoch_ms)),
            ("ended_epoch_ms", json!(self.ended_epoch_ms)),
            ("chunks", json!(self.arrivals.chunks)),
            ("late", json!(late)),
            ("lateness", json!(self.arrivals.lateness)),
            ("late_max_ms", json!(self.arrivals.late_max_ms)),
            ("held_max_ms", json!(self.arrivals.held_max_ms)),
            (
                "provider_send_wait_max_ms",
                json!(self.arrivals.send_wait_max_ms),
            ),
            ("gaps", json!(gaps)),
            ("gaps_beyond_cap", json!(self.gaps_beyond_cap)),
            ("forward_wait_max_ms", json!(self.forward_wait_max_ms)),
            ("send_wait_max_ms", json!(self.send_wait_max_ms)),
            ("lead_ms", json!(self.lead_ms)),
            ("model", json!(model)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
    }
}

/// A span in whole milliseconds.
pub(crate) fn whole_ms(span: Duration) -> u64 {
    u64::try_from(span.as_millis()).unwrap_or(u64::MAX)
}

/// What the connection keeps of a call's provider sessions until its end.
#[derive(Debug)]
pub(crate) struct CallAudio {
    /// Whether a report is ordered at all; nothing is kept without it.
    ordered: bool,
    /// Whether a session opened or carried audio: only such a call reports.
    opened: bool,
    /// What each session hands over when it ends, in the order they opened.
    sessions: Vec<oneshot::Receiver<AudioArrivals>>,
}

impl CallAudio {
    pub(crate) fn new(ordered: bool) -> Self {
        Self {
            ordered,
            opened: false,
            sessions: Vec::new(),
        }
    }

    /// The call opened one more session.
    pub(crate) fn session(&mut self, arrivals: Option<oneshot::Receiver<AudioArrivals>>) {
        if self.ordered {
            self.sessions.extend(arrivals);
        }
    }

    /// A session of the call started, or carried audio.
    pub(crate) fn opened(&mut self) {
        self.opened = true;
    }

    /// The sum over the call's sessions, waiting for the ones still closing
    /// until `by` at the latest. One that does not measure, or says nothing in
    /// time, counts as empty. `None` where there is nothing to report.
    pub(crate) async fn collect(self, by: Instant) -> Option<AudioReport> {
        let mut opened = self.opened;
        let mut report = AudioReport::default();
        for session in self.sessions {
            if let Ok(Ok(arrivals)) = tokio::time::timeout_at(by, session).await {
                opened = true;
                report.add(arrivals);
            }
        }
        (self.ordered && opened).then_some(report)
    }
}
