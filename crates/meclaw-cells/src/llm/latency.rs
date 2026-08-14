//! GH #124 — per-call phase instrumentation for the `llm` cell.
//!
//! The question this module exists to answer: an operator sees ~16 s between
//! the message that entered the cell and the message that left it, while the
//! provider's own dashboard reports 2–4.5 s for the request. Where did the
//! remaining ~12 s go? Without a phase split every explanation — a slow model,
//! a retry ladder, a blocked `cell.db`, a huge request build — looks the same
//! from the outside.
//!
//! The split is deliberately coarse and complete: the four measured phases plus
//! [`PhaseTimings::unaccounted_ms`] add up to the full `handle()` duration, so
//! a large residue is itself a finding ("the time is NOT in any phase we
//! measure") rather than a gap in the record.
//!
//! Nothing here touches the message body: instrumentation is tracing fields
//! only — no new UBF slot, no substrate change.

use crate::llm::wire::WireTimings;

/// Tracing target of the instrumentation. Kept out of the module path so an
/// operator can enable exactly this one stream
/// (`RUST_LOG=meclaw::llm::latency=info`) without turning on the rest of the
/// cell's logging.
pub(crate) const LATENCY_TARGET: &str = "meclaw::llm::latency";

/// Wall-clock phases of ONE `handle()` call.
///
/// Every field is milliseconds. The phases are consecutive and non-overlapping,
/// in the order `handle()` walks them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PhaseTimings {
    /// Body parse, params-update apply and the `cell.db` write of `system.*`
    /// plus `last_input` — everything up to and including the persist
    /// transaction.
    pub persist_ms: u64,
    /// `cell.db` read-back of the system tree, tool extraction, system-prompt
    /// concatenation, `attachments[]` resolution (blob reads + base64) and the
    /// provider request build.
    pub translate_ms: u64,
    /// The provider call itself. `None` when the call never reached the wire
    /// (a parse, persist or translate failure returned first).
    pub wire: Option<WireTimings>,
    /// The whole `handle()`, from entry to just before the emission.
    pub handle_ms: u64,
}

impl PhaseTimings {
    /// Milliseconds inside `handle()` that none of the measured phases claims.
    ///
    /// This is the number the issue is about. It covers the emission itself and
    /// whatever else sits between the phases — and if it is large while the
    /// wire was fast, the delay is ours and not the provider's. Saturating:
    /// clock jitter between the phase marks can never produce a negative
    /// residue, only a zero one.
    pub(crate) fn unaccounted_ms(&self) -> u64 {
        let measured = self
            .persist_ms
            .saturating_add(self.translate_ms)
            .saturating_add(self.wire.map_or(0, |w| w.total_ms));
        self.handle_ms.saturating_sub(measured)
    }

    /// Milliseconds the provider itself spent before answering — the wire's
    /// time-to-first-byte. `None` when no response head ever arrived, so
    /// "the provider was slow" and "the provider never answered" stay distinct.
    pub(crate) fn provider_ms(&self) -> Option<u64> {
        self.wire.and_then(|w| w.ttfb_ms)
    }
}

/// Stopwatch that `handle()` carries through its phases.
///
/// One `Instant` for the whole call plus a moving mark between phases. Cheap
/// (monotonic clock reads only), single-threaded, no allocation — it must never
/// be the reason a call gets slower.
pub(crate) struct PhaseClock {
    start: std::time::Instant,
    mark: std::time::Instant,
    timings: PhaseTimings,
}

impl PhaseClock {
    /// Start the clock at `handle()` entry.
    pub(crate) fn start() -> Self {
        let now = std::time::Instant::now();
        Self {
            start: now,
            mark: now,
            timings: PhaseTimings::default(),
        }
    }

    /// Milliseconds since the last mark, and move the mark to now.
    fn lap(&mut self) -> u64 {
        let now = std::time::Instant::now();
        let ms = u64::try_from((now - self.mark).as_millis()).unwrap_or(u64::MAX);
        self.mark = now;
        ms
    }

    /// Close the persist phase (body parse + `cell.db` writes).
    pub(crate) fn persisted(&mut self) {
        self.timings.persist_ms = self.lap();
    }

    /// Close the translate phase (system read-back, tools, prompt,
    /// attachments, request build).
    pub(crate) fn translated(&mut self) {
        self.timings.translate_ms = self.lap();
    }

    /// Record what the provider call cost.
    pub(crate) fn wired(&mut self, wire: WireTimings) {
        self.timings.wire = Some(wire);
        self.mark = std::time::Instant::now();
    }

    /// Freeze the total `handle()` duration and hand out the phases.
    pub(crate) fn finish(&self) -> PhaseTimings {
        let mut t = self.timings;
        t.handle_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        t
    }
}

/// Emit the one-line INFO summary for a finished provider call (GH #124).
///
/// One line per call, every phase on it, so a single `grep` over an operating
/// log answers "where did the time go". `outcome` is `ok` or the UBF
/// `error_code`, so slow calls and failed calls can be told apart without a
/// second lookup. No credential and no message content is ever a field here —
/// only durations, the model id and the dialect.
///
/// The three wire fields are `Option`s, and `tracing` omits a field whose value
/// is `None`: a line WITHOUT `wire_total_ms` is a call that never reached the
/// provider, and a line with `wire_total_ms` but no `provider_ttfb_ms` is a
/// call the provider never answered. Absence is the signal — it is never
/// rendered as a zero.
pub(crate) fn log_summary(t: &PhaseTimings, dialect: &str, model: &str, outcome: &str) {
    tracing::info!(
        target: LATENCY_TARGET,
        dialect,
        model,
        outcome,
        handle_ms = t.handle_ms,
        persist_ms = t.persist_ms,
        translate_ms = t.translate_ms,
        provider_ttfb_ms = t.provider_ms(),
        wire_total_ms = t.wire.map(|w| w.total_ms),
        wire_attempts = t.wire.map(|w| w.attempts),
        unaccounted_ms = t.unaccounted_ms(),
        "llm provider call phases"
    );
}

/// Emit the DEBUG detail line for the request that was built (GH #124).
///
/// Sizes and counts only — the numbers that explain a slow translate phase (a
/// megabyte of base64'd attachments, a thousand-turn history) without putting
/// one byte of the conversation into the log.
pub(crate) fn log_request_detail(
    dialect: &str,
    request_bytes: usize,
    input_turns: usize,
    tools: usize,
    image_parts: usize,
    system_prompt_chars: usize,
) {
    tracing::debug!(
        target: LATENCY_TARGET,
        dialect,
        request_bytes,
        input_turns,
        tools,
        image_parts,
        system_prompt_chars,
        "llm request build detail"
    );
}

#[cfg(test)]
mod tests {
    use super::{PhaseClock, PhaseTimings};
    use crate::llm::wire::WireTimings;

    fn wire(ttfb: Option<u64>, total: u64, attempts: u32) -> Option<WireTimings> {
        Some(WireTimings {
            ttfb_ms: ttfb,
            total_ms: total,
            attempts,
        })
    }

    /// The reported live shape (GH #124 operator comment): a 4 s provider call
    /// inside a 16 s handle. The residue must name the missing 12 s, not hide
    /// them in the wire figure.
    #[test]
    fn a_fast_provider_inside_a_slow_handle_shows_the_residue() {
        let t = PhaseTimings {
            persist_ms: 30,
            translate_ms: 70,
            wire: wire(Some(4_000), 4_100, 1),
            handle_ms: 16_000,
        };
        assert_eq!(t.provider_ms(), Some(4_000));
        assert_eq!(t.unaccounted_ms(), 16_000 - 30 - 70 - 4_100);
    }

    /// The opposite finding: the provider WAS the delay. Everything else is
    /// noise and the residue is small — the same line has to be able to say so.
    #[test]
    fn a_slow_provider_leaves_almost_no_residue() {
        let t = PhaseTimings {
            persist_ms: 5,
            translate_ms: 5,
            wire: wire(Some(11_800), 12_000, 1),
            handle_ms: 12_020,
        };
        assert_eq!(t.unaccounted_ms(), 10);
    }

    #[test]
    fn a_call_that_never_reached_the_wire_accounts_the_rest_as_residue() {
        let t = PhaseTimings {
            persist_ms: 12,
            translate_ms: 8,
            wire: None,
            handle_ms: 40,
        };
        assert_eq!(t.provider_ms(), None);
        assert_eq!(t.unaccounted_ms(), 20);
    }

    /// Phase marks are taken at different instants than the outer stopwatch, so
    /// rounding can make the parts exceed the whole by a millisecond. That must
    /// floor at zero, never wrap.
    #[test]
    fn rounding_jitter_cannot_produce_a_negative_residue() {
        let t = PhaseTimings {
            persist_ms: 10,
            translate_ms: 10,
            wire: wire(Some(1), 10, 1),
            handle_ms: 29,
        };
        assert_eq!(t.unaccounted_ms(), 0);
    }

    /// A retry ladder is wire time, not residue: both attempts are already
    /// summed into `total_ms`, so folding them must not inflate the residue.
    #[test]
    fn a_retry_ladder_stays_inside_the_wire_phase() {
        let t = PhaseTimings {
            persist_ms: 0,
            translate_ms: 0,
            wire: wire(Some(3_000), 3_340, 2),
            handle_ms: 3_400,
        };
        assert_eq!(t.unaccounted_ms(), 60);
        assert_eq!(t.wire.unwrap().attempts, 2);
    }

    /// The clock must attribute a sleep to the phase it happened in, and the
    /// phases must stay inside the total. Timing discriminator held loose
    /// (a floor, not a window) so cargo-parallel load cannot break it.
    #[tokio::test]
    async fn the_clock_attributes_a_sleep_to_the_phase_it_happened_in() {
        let mut clock = PhaseClock::start();
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        clock.persisted();
        clock.translated();
        let t = clock.finish();
        assert!(t.persist_ms >= 100, "persist_ms={}", t.persist_ms);
        assert!(t.translate_ms < 100, "translate_ms={}", t.translate_ms);
        assert!(
            t.handle_ms >= t.persist_ms,
            "the total must contain its phases"
        );
    }

    #[test]
    fn a_wire_result_lands_in_the_wire_phase() {
        let mut clock = PhaseClock::start();
        clock.persisted();
        clock.translated();
        clock.wired(WireTimings {
            ttfb_ms: Some(7),
            total_ms: 9,
            attempts: 1,
        });
        let t = clock.finish();
        assert_eq!(t.provider_ms(), Some(7));
        assert_eq!(t.wire.unwrap().total_ms, 9);
    }
}
