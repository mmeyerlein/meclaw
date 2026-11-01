//! Time marks of one duplex call (D48, R-LS-8): the points in a call at which
//! the colony is told how long it has been running.
//!
//! A telephone conversation that runs long is steered by its length -- an
//! interim assessment after 15 minutes, winding down after 30, then every 10
//! (D46 M6, R-LS-8). The colony cannot measure that itself: nothing it holds
//! runs on the call's clock. So the connection, which owns that clock, fires a
//! `call_mark` at fixed points: first the list `call_marks_s`, then every
//! `call_mark_every_s` after the last of them.
//!
//! **One timer per call, and the next one only after the last has fired**
//! (OR-LV-14): the marks are a deadline, not a heartbeat. Nothing is asked and
//! nothing is read; a deadline goes off, or the call ends first and takes the
//! timer with it -- the clock lives in the connection's own loop, so a call
//! that is over cannot fire a mark (review focus 4).

use std::time::Duration;
use tokio::time::Instant;

/// When a call is marked, in seconds since the connection opened
/// (`params.call_marks_s`, `params.call_mark_every_s`).
///
/// The default -- an empty list and no interval -- is off: no timer runs and
/// no `call_mark` lane is emitted, which is what every cell did before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallMarks {
    /// Fixed marks, strictly ascending and positive (the parse refuses
    /// anything else).
    pub marks_s: Vec<u64>,
    /// After the last fixed mark (or from the call's start, when the list is
    /// empty), one mark every this many seconds; `0` stops after the list.
    pub every_s: u64,
}

impl CallMarks {
    /// Whether no mark is ever due.
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.marks_s.is_empty() && self.every_s == 0
    }

    /// The `n`-th mark of a call (counting from 0), in seconds since it
    /// opened; `None` once the list is done and no interval follows.
    #[must_use]
    pub fn nth_s(&self, n: usize) -> Option<u64> {
        if let Some(s) = self.marks_s.get(n) {
            return Some(*s);
        }
        if self.every_s == 0 {
            return None;
        }
        let base = self.marks_s.last().copied().unwrap_or(0);
        let k = u64::try_from(n - self.marks_s.len() + 1).ok()?;
        base.checked_add(self.every_s.checked_mul(k)?)
    }
}

/// The running clock of one call's marks: where it opened, and how many marks
/// it has fired.
#[derive(Debug)]
pub struct CallMarkClock {
    marks: CallMarks,
    opened: Instant,
    fired: usize,
}

impl CallMarkClock {
    /// The clock of a call that opened at `opened`.
    #[must_use]
    pub fn new(marks: CallMarks, opened: Instant) -> Self {
        Self {
            marks,
            opened,
            fired: 0,
        }
    }

    /// When the next mark is due; `None` when none is.
    #[must_use]
    pub fn next_at(&self) -> Option<Instant> {
        self.marks
            .nth_s(self.fired)
            .map(|s| self.opened + Duration::from_secs(s))
    }

    /// The mark that is due has fired: returns its `elapsed_s` and arms the
    /// next one.
    pub fn fire(&mut self) -> u64 {
        let s = self.marks.nth_s(self.fired).unwrap_or_default();
        self.fired += 1;
        s
    }

    /// Wait until the next mark is due -- forever when none is, so a `select!`
    /// arm on it simply never fires.
    pub async fn due(&self) {
        match self.next_at() {
            None => std::future::pending().await,
            Some(at) => tokio::time::sleep_until(at).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The paused tokio clock's "now". Every test here runs under
    /// `start_paused = true`, so this is virtual time and the asserts on it
    /// are exact (AGENTS rule 11), never a reading of the machine's load.
    fn virtual_now() -> Instant {
        tokio::time::Instant::now()
    }

    /// Virtual time passed since `since`, on the paused clock.
    fn virtual_since(since: Instant) -> Duration {
        virtual_now().saturating_duration_since(since)
    }

    /// Fires `n` marks on the (paused) tokio clock and returns, for each, the
    /// `elapsed_s` it reported and the virtual time it actually fired at.
    async fn fire_n(marks: CallMarks, n: usize) -> Vec<(u64, Duration)> {
        let opened = virtual_now();
        let mut clock = CallMarkClock::new(marks, opened);
        let mut out = Vec::new();
        for _ in 0..n {
            let Some(_) = clock.next_at() else {
                panic!("a mark is due after {out:?}")
            };
            clock.due().await;
            let at = virtual_since(opened);
            out.push((clock.fire(), at));
        }
        out
    }

    /// The shipped set: 15 and 30 minutes, then every 10 (D46 M6, R-LS-8).
    /// Each mark fires exactly on its second of virtual time.
    #[tokio::test(start_paused = true)]
    async fn call_marks_fire_at_list_then_every() {
        let shipped = CallMarks {
            marks_s: vec![900, 1800],
            every_s: 600,
        };
        let fired = fire_n(shipped, 4).await;
        assert_eq!(
            fired,
            vec![
                (900, Duration::from_secs(900)),
                (1800, Duration::from_secs(1800)),
                (2400, Duration::from_secs(2400)),
                (3000, Duration::from_secs(3000)),
            ]
        );

        // The lab's short set behaves the same, scaled.
        let lab = CallMarks {
            marks_s: vec![3, 5],
            every_s: 2,
        };
        let fired = fire_n(lab, 4).await;
        assert_eq!(
            fired.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![3, 5, 7, 9]
        );
        assert_eq!(fired[3].1, Duration::from_secs(9));
    }

    /// A call that ends at 850 s -- before the first mark -- fires none: the
    /// hang-up wins the race, and with it the clock is gone.
    #[tokio::test(start_paused = true)]
    async fn call_marks_stop_after_hangup() {
        let opened = virtual_now();
        let mut clock = CallMarkClock::new(
            CallMarks {
                marks_s: vec![900, 1800],
                every_s: 600,
            },
            opened,
        );
        let hang_up = tokio::time::sleep(Duration::from_secs(850));
        let mut fired = Vec::new();
        tokio::pin!(hang_up);
        loop {
            tokio::select! {
                biased;
                () = &mut hang_up => break,
                () = clock.due() => fired.push(clock.fire()),
            }
        }
        assert!(fired.is_empty(), "no mark before 900 s: {fired:?}");
        assert_eq!(virtual_since(opened), Duration::from_secs(850));
        drop(clock);
    }

    /// Off by default, and a list without an interval stops after the list.
    #[tokio::test(start_paused = true)]
    async fn usage_and_marks_off_by_default() {
        let off = CallMarks::default();
        assert!(off.is_off());
        assert_eq!(off.nth_s(0), None);
        assert_eq!(CallMarkClock::new(off, virtual_now()).next_at(), None);

        let list_only = CallMarks {
            marks_s: vec![900],
            every_s: 0,
        };
        assert_eq!(list_only.nth_s(0), Some(900));
        assert_eq!(list_only.nth_s(1), None);

        let every_only = CallMarks {
            marks_s: Vec::new(),
            every_s: 600,
        };
        assert_eq!(every_only.nth_s(0), Some(600));
        assert_eq!(every_only.nth_s(2), Some(1800));
    }
}
