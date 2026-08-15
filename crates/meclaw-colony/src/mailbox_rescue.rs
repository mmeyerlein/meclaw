//! Mailbox preservation across cell death (GH #18).
//!
//! A cell task owns the receiving end of its mailbox in its own frame. Every
//! exit that is NOT one of the deliberate, peaceful ones drops that receiver
//! inside the dying task: a panic unwinds it, and the outer `select!` of a
//! long-running cell aborts the surviving handler task — mailbox and all —
//! when the I/O side ends first. Whatever was buffered at that moment used to
//! disappear without a trace and without a signal.
//!
//! [`MailboxGuard`] closes that hole from OUTSIDE the byte-frozen corridors:
//! it owns the receiver for the whole life of the task, and its `Drop` — which
//! runs on an unwind and on a task abort alike — drains what is left and hands
//! it to the colony as [`crate::ColonyMsg::MailboxRescued`]. The colony
//! delivers it to the successor after the respawn, or dead-letters it when the
//! death left no successor.
//!
//! The peaceful exits are unaffected: peace-stop, idle-sleep and one-shot hand
//! the whole `Receiver` over themselves (`ColonyMsg::Stopped` / `Sleep`) and
//! disarm the guard via [`MailboxGuard::release`] while doing so.
//!
//! **Ordering.** The guard hands over while the task is being dropped, which is
//! strictly before its `JoinHandle` resolves — so the rescue is enqueued on the
//! colony inbox before the watcher can send `CellDied` for the same cell. That
//! is what lets the colony hold the messages for the successor instead of
//! having to guess.

use meclaw_core::{Message, Path};
use tokio::sync::mpsc;

/// Owns a cell's mailbox receiver and preserves its unread remainder when the
/// cell task dies without handing the mailbox over itself.
pub(crate) struct MailboxGuard {
    own_path: Path,
    /// `None` once the receiver was handed on by a peaceful exit — a released
    /// guard is inert.
    inner: Option<mpsc::Receiver<Message>>,
    /// `None` on the plain test paths without a colony; then nothing can be
    /// rescued and the guard is inert by construction.
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
}

impl MailboxGuard {
    /// Wrap a cell's mailbox receiver for the lifetime of its task.
    pub(crate) fn new(
        own_path: Path,
        mailbox: mpsc::Receiver<Message>,
        colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    ) -> Self {
        Self {
            own_path,
            inner: Some(mailbox),
            colony_inbox_tx,
        }
    }

    /// Receive the next mailbox message.
    ///
    /// Cancel-safe: it forwards to `mpsc::Receiver::recv`, so a `select!` that
    /// drops this future loses no message. A released guard parks forever —
    /// every release site returns from its loop immediately afterwards.
    pub(crate) async fn recv(&mut self) -> Option<Message> {
        match self.inner.as_mut() {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Is the mailbox free of buffered messages? A released guard counts as
    /// empty (it no longer owns a queue).
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.as_ref().is_none_or(|rx| rx.is_empty())
    }

    /// Hand the receiver on (peace-stop, idle-sleep, one-shot) and disarm the
    /// guard — those paths route the remainder themselves.
    pub(crate) fn release(&mut self) -> Option<mpsc::Receiver<Message>> {
        self.inner.take()
    }
}

impl Drop for MailboxGuard {
    fn drop(&mut self) {
        let (Some(mut rx), Some(tx)) = (self.inner.take(), self.colony_inbox_tx.as_ref()) else {
            return;
        };
        // Close first (as the disconnect drain does) so no sender can slip a
        // message in behind the drain and have it vanish with the receiver.
        rx.close();
        let mut rescued = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            rescued.push(msg);
        }
        if rescued.is_empty() {
            return;
        }
        let count = rescued.len();
        // `Drop` cannot await, so this is a `try_send` — the same fire-and-
        // forget shape `renotify_stop_wiring` uses from the await-free respawn
        // corridor. A full colony inbox is the one case that still loses the
        // messages, and it says so loudly instead of failing silently.
        match tx.try_send(crate::ColonyMsg::MailboxRescued {
            path: self.own_path.clone(),
            messages: rescued,
        }) {
            Ok(()) => tracing::warn!(
                path = %self.own_path.as_str(),
                count,
                "cell died with buffered mailbox messages — handed to the colony for the successor"
            ),
            Err(_) => tracing::error!(
                path = %self.own_path.as_str(),
                count,
                "cell died with buffered mailbox messages and the colony inbox would not take them — messages lost"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::MessageBuilder;

    #[tokio::test]
    async fn a_dropped_guard_hands_the_remainder_to_the_colony() {
        let (tx, rx) = mpsc::channel::<Message>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        tx.send(MessageBuilder::new(Path::new("/c")).build())
            .await
            .unwrap();
        let guard = MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx));
        drop(guard);
        match colony_rx.try_recv() {
            Ok(crate::ColonyMsg::MailboxRescued { path, messages }) => {
                assert_eq!(path, Path::new("/c"));
                assert_eq!(messages.len(), 1);
            }
            _ => panic!("the guard must hand the remainder over"),
        }
    }

    #[tokio::test]
    async fn a_released_guard_stays_silent() {
        let (tx, rx) = mpsc::channel::<Message>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        tx.send(MessageBuilder::new(Path::new("/c")).build())
            .await
            .unwrap();
        let mut guard = MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx));
        let handed_on = guard.release();
        assert!(handed_on.is_some(), "release yields the receiver");
        drop(guard);
        assert!(
            colony_rx.try_recv().is_err(),
            "a released guard must not duplicate the remainder"
        );
    }

    #[tokio::test]
    async fn an_empty_mailbox_produces_no_rescue() {
        let (_tx, rx) = mpsc::channel::<Message>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        let guard = MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx));
        assert!(guard.is_empty());
        drop(guard);
        assert!(
            colony_rx.try_recv().is_err(),
            "nothing buffered, nothing to rescue"
        );
    }
}
