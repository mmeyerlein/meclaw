//! GH #1059: the credential round of a LONG-RUNNING cell (proxy, voice).
//!
//! [`crate::credential`] is the grant mechanics every cell shares (slots, key
//! pair, opening the box). A long-running cell adds one thing the llm cell
//! never needed: a clock. The llm cell parks a turn and the NEXT turn opens a
//! new round when the old one timed out. A proxy or a voice cell asks once in
//! `on_start` and has nothing parked — if that one question is lost (the
//! broker is not spawned yet, the vault is locked, an error came back instead
//! of a box) nothing would ever ask again and the cell would sleep forever.
//!
//! So the I/O half, which sleeps until its secret arrives anyway, keeps a
//! [`RoundClock`]: no box within `credential_wait_ms` → it tells the handler,
//! the handler asks again with a fresh recipient, and the next wait doubles up
//! to `credential_backoff_max_ms`. One log line per round, written by the cell.
//!
//! The handler hands an opened secret to its I/O half as a
//! [`crate::credential::Secret`] (`CredentialSlots::secret_handle`) inside the
//! cell's own reconfigure variant; it prints `<sealed>`, so a `{:?}` of a
//! reconfigure frame cannot leak it.

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Default ceiling of the doubling wait between two rounds: 5 min. A lost
/// question costs at most this long once the vault is reachable again.
#[must_use]
pub fn default_credential_backoff_max_ms() -> u64 {
    300_000
}

/// The waiting schedule of one cell's credential rounds: the first round waits
/// `wait_ms`, every following one twice the last, capped at `cap_ms`.
///
/// The deadline is an absolute instant, so a reconfigure frame that wakes the
/// I/O loop in between (a params update) does not restart the wait.
#[derive(Debug, Clone)]
pub struct RoundClock {
    current_ms: u64,
    cap_ms: u64,
    deadline: Instant,
    round: u32,
}

impl RoundClock {
    /// Start round 1 now. A zero wait is lifted to 1 ms (a zero deadline would
    /// re-ask in a tight loop); a cap below the first wait is lifted to it.
    #[must_use]
    pub fn start(wait_ms: u64, cap_ms: u64) -> Self {
        let current_ms = wait_ms.max(1);
        Self {
            current_ms,
            cap_ms: cap_ms.max(current_ms),
            deadline: Instant::now() + Duration::from_millis(current_ms),
            round: 1,
        }
    }

    /// When the current round ends without a box.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The round in flight (1 = the `on_start` question).
    #[must_use]
    pub fn round(&self) -> u32 {
        self.round
    }

    /// The wait of the round in flight, in ms.
    #[must_use]
    pub fn wait_ms(&self) -> u64 {
        self.current_ms
    }

    /// The current round expired: open the next one with the doubled wait.
    /// Returns the new round's number and wait.
    pub fn next_round(&mut self) -> (u32, u64) {
        self.current_ms = self.current_ms.saturating_mul(2).min(self.cap_ms);
        self.deadline = Instant::now() + Duration::from_millis(self.current_ms);
        self.round = self.round.saturating_add(1);
        (self.round, self.current_ms)
    }
}

/// What woke an I/O half that sleeps until its credential arrives.
#[derive(Debug)]
pub enum Wake<R> {
    /// A reconfigure frame (the credential, or a params update).
    Reconfig(R),
    /// The handler is gone — the cell tears down.
    Closed,
    /// The round's deadline passed without a box. The clock already moved to
    /// the next round; `round`/`wait_ms` are the NEW round's.
    RoundExpired {
        /// Number of the round that starts now.
        round: u32,
        /// Its wait in ms.
        wait_ms: u64,
    },
}

/// Sleep until a reconfigure frame arrives or the round's deadline passes.
/// Reconfigure frames win a tie (`biased`), so a box that lands at the deadline
/// starts the cell instead of asking once more.
pub async fn next_wake<R>(rx: &mut mpsc::Receiver<R>, clock: &mut RoundClock) -> Wake<R> {
    tokio::select! {
        biased;
        frame = rx.recv() => match frame {
            Some(r) => Wake::Reconfig(r),
            None => Wake::Closed,
        },
        () = tokio::time::sleep_until(clock.deadline()) => {
            let (round, wait_ms) = clock.next_round();
            Wake::RoundExpired { round, wait_ms }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_up_to_the_cap() {
        let mut c = RoundClock::start(1_000, 5_000);
        assert_eq!((c.round(), c.wait_ms()), (1, 1_000));
        assert_eq!(c.next_round(), (2, 2_000));
        assert_eq!(c.next_round(), (3, 4_000));
        assert_eq!(c.next_round(), (4, 5_000));
        assert_eq!(c.next_round(), (5, 5_000));
    }

    #[test]
    fn a_zero_wait_and_a_low_cap_are_lifted() {
        let mut c = RoundClock::start(0, 0);
        assert_eq!(c.wait_ms(), 1);
        assert_eq!(c.next_round(), (2, 1));
    }

    #[tokio::test]
    async fn a_frame_before_the_deadline_wins_and_keeps_the_deadline() {
        let (tx, mut rx) = mpsc::channel::<u8>(2);
        let mut c = RoundClock::start(100, 800);
        let deadline = c.deadline();
        tx.send(7).await.unwrap();
        assert!(matches!(
            next_wake(&mut rx, &mut c).await,
            Wake::Reconfig(7)
        ));
        assert_eq!(c.deadline(), deadline, "a frame must not restart the wait");
        match next_wake(&mut rx, &mut c).await {
            Wake::RoundExpired { round, wait_ms } => assert_eq!((round, wait_ms), (2, 200)),
            other => panic!("expected the round to expire, got {other:?}"),
        }
        drop(tx);
        assert!(matches!(next_wake(&mut rx, &mut c).await, Wake::Closed));
    }
}
