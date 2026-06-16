//! RAII drop-guard that signals cell-task termination via a oneshot channel.
//!
//! The guard is bound one `let` deep inside a cell-task frame so it drops
//! **after** the frame's `DbConn` (Rust drops locals in reverse declaration
//! order: declare the guard *before* the `DbConn`, so the guard's `Drop` runs
//! *after* the connection's `Drop` → after `sqlite3_close()`). That ordering
//! guarantees a consumer awaiting the `death_ack` only observes it once the
//! `cell.db` connection is closed.
//!
//! Phase-13.5 Lifecycle-3b Task 3 (F2): paired with the colony-initiated
//! peace-stop. The colony fires `stop_tx`, the task finishes its in-flight
//! `handle()`, fires `peace_tx`, drops its frame (closing `cell.db`), and the
//! `TermAckGuard::drop` then fires `death_ack`.

use tokio::sync::oneshot;

/// RAII guard that fires a `oneshot::Sender<()>` on `Drop`.
///
/// Bind it **before** the `DbConn` in a cell-task frame so it drops afterwards
/// (reverse-declaration drop order) — the ack then signals "task frame torn
/// down, `cell.db` closed".
pub struct TermAckGuard {
    /// The death-ack sender. `Option` so `Drop` can `take()` it and send
    /// exactly once; `None` makes the guard a no-op.
    tx: Option<oneshot::Sender<()>>,
}

impl TermAckGuard {
    /// Create a guard that fires `tx` on drop. `None` → inert guard (no send).
    pub fn new(tx: Option<oneshot::Sender<()>>) -> Self {
        Self { tx }
    }
}

impl Drop for TermAckGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            // Receiver may already be gone (caller dropped it) — ignore.
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    /// 3.1: a guard in scope with a (mock) conn fires exactly one `()` on
    /// scope-end, and the ack arrives only after the conn-binding has dropped.
    /// Drop order: `_guard` declared before `conn` → `_guard` drops *after*
    /// `conn`, so the ack observably follows the conn teardown.
    #[test]
    fn guard_fires_ack_after_conn_drop_at_scope_end() {
        let (tx, mut rx) = oneshot::channel::<()>();
        {
            // Declare guard FIRST so it drops LAST (after the conn).
            let _guard = TermAckGuard::new(Some(tx));
            // A stand-in for the `DbConn` whose `Drop` (sqlite3_close) must run
            // before the ack fires. Its drop runs first (declared after guard).
            let conn = String::from("cell.db connection");
            // While both are alive, no ack yet.
            assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty));
            drop(conn);
        }
        // Scope end: guard dropped → exactly one `()` received.
        assert_eq!(rx.try_recv(), Ok(()));
        // And no second value.
        assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
    }

    /// `None`-guard is inert: drop must not panic and the receiver sees a
    /// dropped sender (no value).
    #[test]
    fn none_guard_is_noop_no_panic() {
        let g = TermAckGuard::new(None);
        drop(g);
    }

    /// A dropped receiver before the guard fires must be silently tolerated.
    #[test]
    fn send_failure_silenced_when_receiver_gone() {
        let (tx, rx) = oneshot::channel::<()>();
        drop(rx);
        let g = TermAckGuard::new(Some(tx));
        drop(g);
    }
}
