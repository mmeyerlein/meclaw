//! What one duplex call used, over every provider session it ran on (D48, DK).
//!
//! A call renewed at every fact turn (GH #896, LC2 `renew_again`) runs on many
//! provider sessions, and its cost is their sum. No single `Closed` carries
//! that sum, and not every session reaches its `Closed`: a session still
//! retiring when the caller hangs up, one let go by the next takeover, a
//! renewal that fails before it takes over, the session in charge whose
//! verdict times out. So the connection -- the one place that sees every
//! session of the call -- keeps one entry per session: its last meter
//! reading, replaced by what it closed with once it closes. The total is the
//! sum of those entries, and it is `complete` only while every session that
//! counts has closed.
//!
//! A session counts from its first reading (a meter reading or its close). One
//! that never sent one -- a renewal that failed before it opened, or one still
//! waiting to take over when the caller hung up -- adds nothing and does not
//! make the total incomplete: there is no number of its own to stand in for.
//! The one exception is a renewal that opened and failed without a close
//! ([`CallUsage::unclosed`]): it was billed, so the total says it may be short.

use crate::voice::contract::DuplexEvent;

/// One provider session of a call, as the ledger knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageSlot(usize);

/// What one session reported so far.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SessionUsage {
    /// Its last meter reading, or what it closed with.
    seconds: f64,
    /// Whether it closed: `seconds` is then its own final number.
    closed: bool,
}

/// The ledger of one call (D48, DK): one entry per provider session.
#[derive(Debug, Default)]
pub struct CallUsage {
    /// Indexed by [`UsageSlot`]; `None` until the session said anything.
    sessions: Vec<Option<SessionUsage>>,
}

impl CallUsage {
    /// A slot for a session the call opens.
    pub fn session(&mut self) -> UsageSlot {
        self.sessions.push(None);
        UsageSlot(self.sessions.len() - 1)
    }

    /// Read one event of the session in `slot`. Returns whether the call's
    /// total moved: a meter reading of a session that has not closed, or its
    /// close.
    pub fn observe(&mut self, slot: UsageSlot, event: &DuplexEvent) -> bool {
        let Some(entry) = self.sessions.get_mut(slot.0) else {
            return false;
        };
        match event {
            DuplexEvent::Usage { seconds, .. } => match entry {
                Some(e) if e.closed => false,
                _ => {
                    *entry = Some(SessionUsage {
                        seconds: *seconds,
                        closed: false,
                    });
                    true
                }
            },
            DuplexEvent::Closed { usage_seconds, .. } => match entry {
                Some(e) if e.closed => false,
                _ => {
                    *entry = Some(SessionUsage {
                        seconds: *usage_seconds,
                        closed: true,
                    });
                    true
                }
            },
            _ => false,
        }
    }

    /// The session in `slot` is gone without a close (review re-check Re-1):
    /// a session that opened was billed, so one that never said a number
    /// leaves the total open instead of adding nothing. Returns whether that
    /// changed the ledger.
    pub fn unclosed(&mut self, slot: UsageSlot) -> bool {
        match self.sessions.get_mut(slot.0) {
            Some(entry @ None) => {
                *entry = Some(SessionUsage {
                    seconds: 0.0,
                    closed: false,
                });
                true
            }
            _ => false,
        }
    }

    /// The call's total so far: every session's close, or its last reading.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.sessions.iter().flatten().map(|e| e.seconds).sum()
    }

    /// Whether every session that counts has closed -- `false` where a
    /// session's last reading stands in for a close it never sent, so the
    /// total may be short.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.sessions.iter().flatten().all(|e| e.closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter(seconds: f64) -> DuplexEvent {
        DuplexEvent::Usage {
            seconds,
            usage_ratio: None,
        }
    }

    fn closed(usage_seconds: f64) -> DuplexEvent {
        DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds,
        }
    }

    fn started() -> DuplexEvent {
        DuplexEvent::Started {
            session_id: "provider".to_string(),
        }
    }

    #[test]
    fn a_call_without_sessions_used_nothing_and_is_complete() {
        let usage = CallUsage::default();
        assert_eq!(usage.total(), 0.0);
        assert!(usage.complete());
    }

    #[test]
    fn the_total_sums_every_closed_session() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        let second = usage.session();
        assert!(usage.observe(first, &meter(5.0)));
        assert!(usage.observe(first, &closed(12.0)));
        assert!(usage.observe(second, &closed(30.0)));
        assert_eq!(usage.total(), 42.0);
        assert!(usage.complete());
    }

    /// Between a takeover and the old session's close, both meters count:
    /// the running total never falls below what it said before.
    #[test]
    fn the_running_total_holds_across_a_handover() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        usage.observe(first, &meter(5.0));
        let second = usage.session();
        usage.observe(second, &meter(2.0));
        assert_eq!(usage.total(), 7.0);
    }

    /// A session that never closed counts with its last reading, and the
    /// total says it may be short.
    #[test]
    fn a_session_without_close_counts_its_last_meter_and_is_incomplete() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        let second = usage.session();
        usage.observe(first, &meter(4.0));
        usage.observe(first, &meter(8.0));
        usage.observe(second, &closed(3.0));
        assert_eq!(usage.total(), 11.0);
        assert!(!usage.complete());
    }

    /// A renewal that metered before it took over and never closed counts
    /// with that reading; a start alone is no reading.
    #[test]
    fn a_renewal_counts_from_its_first_reading() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        let renewal = usage.session();
        usage.observe(first, &closed(4.0));
        assert!(!usage.observe(renewal, &started()));
        assert!(usage.complete());
        assert!(usage.observe(renewal, &meter(2.0)));
        assert_eq!(usage.total(), 6.0);
        assert!(!usage.complete());
    }

    /// A renewal that never said a word about itself used nothing.
    #[test]
    fn a_silent_session_does_not_count() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        let _failed = usage.session();
        usage.observe(first, &closed(6.0));
        assert_eq!(usage.total(), 6.0);
        assert!(usage.complete());
    }

    /// A session gone without a close leaves the total open; one that closed
    /// keeps its close.
    #[test]
    fn an_unclosed_session_leaves_the_total_open() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        let failed = usage.session();
        usage.observe(first, &closed(6.0));
        assert!(usage.unclosed(failed));
        assert_eq!(usage.total(), 6.0);
        assert!(!usage.complete());
        assert!(!usage.unclosed(first));
        assert!(!usage.unclosed(failed));
    }

    /// What a session closed with is its number; a late reading or a second
    /// close changes nothing.
    #[test]
    fn a_closed_session_keeps_its_close() {
        let mut usage = CallUsage::default();
        let first = usage.session();
        usage.observe(first, &closed(10.0));
        assert!(!usage.observe(first, &meter(99.0)));
        assert!(!usage.observe(first, &closed(50.0)));
        assert_eq!(usage.total(), 10.0);
        assert!(usage.complete());
    }
}
