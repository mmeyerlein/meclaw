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
//!
//! **A write behind the close (GH #1068, #1209).** A sender that took its
//! permit before the close can still write after the drain, and it sees `Ok`.
//! The guard cannot wait for that in `Drop` (see
//! `MailboxGuard::wait_for_the_late_writes`); a short task holding only the
//! closed receiver forwards such writes as
//! [`crate::ColonyMsg::MailboxRescuedLate`], before or after the `CellDied`.
//! The guard announces that task from `Drop` -- so, like the rescue, strictly
//! before the `CellDied` -- under a [`LateTicket`] that names this one dying
//! mailbox. The colony settles per ticket at the `CellDied` where the late
//! writes go ([`LateRescues`]): behind the rescue while the death is pending,
//! afterwards to the successor the death restarted, else to the DLQ. A write
//! of an old incarnation therefore never reaches a newer cell at the same
//! path (GH #688), and none is buffered for a cell that does not run.

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
        // `Disconnected` after a close means "empty AND no permit out"
        // (tokio: `rx_closed && semaphore.is_idle()`); `Empty` means a sender
        // still holds a permit it took before the close.
        let settled = loop {
            match rx.try_recv() {
                Ok(msg) => rescued.push(msg),
                Err(mpsc::error::TryRecvError::Disconnected) => break true,
                Err(mpsc::error::TryRecvError::Empty) => break false,
            }
        };
        if !rescued.is_empty() {
            self.hand_over(tx, rescued);
        }
        if !settled {
            self.wait_for_the_late_writes(rx, tx);
        }
    }
}

impl MailboxGuard {
    /// GH #1068 (residual, #1209): a permit is still out, so a write can
    /// still land in this (closed) queue — and a sender that writes through
    /// a permit sees `Ok`, so nobody else parks or dead-letters that message.
    ///
    /// `Drop` must not WAIT for it: the holder may be a task whose permit was
    /// assigned by the cell's last read and that has not run since (it sits
    /// in this very worker's LIFO slot, which no other worker steals), so a
    /// spin here would stall it and the drop forever. The closed receiver
    /// goes to a short task instead, which forwards every late write to the
    /// colony as [`crate::ColonyMsg::MailboxRescuedLate`] the moment it lands
    /// (a written permit wakes the receiver). It holds only the receiver and
    /// a sender clone — no state shared with anyone.
    ///
    /// It ends on `recv() == None`: the queue empty and no permit out, or the
    /// last mailbox sender gone. No mailbox permit is held across an `.await`
    /// (`Sender::send` writes synchronously after `reserve`; the overflow
    /// drain sends or drops its permits synchronously, GH #850 M-1), so the
    /// first comes promptly when a permit is written or dropped. A woken
    /// waiter whose permit the last read had assigned gets `Err` from the
    /// closed semaphore and keeps its message; its permit goes back without
    /// waking the receiver (tokio's `Acquire` drop), so then the task ends
    /// with the dead mailbox's last sender — the registry handle replaced by
    /// the respawn, or removed with the entry.
    fn wait_for_the_late_writes(
        &self,
        mut rx: mpsc::Receiver<Message>,
        tx: &mpsc::Sender<crate::ColonyMsg>,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // Outside a runtime there is no task to wait in. A cell task dies
            // on a runtime thread (unwind or abort), so this is a test path.
            tracing::error!(
                path = %self.own_path.as_str(),
                "cell died while a mailbox permit was out and no runtime can wait for \
                 the write — a message written through it is lost"
            );
            return;
        };
        let path = self.own_path.clone();
        let ticket = LateTicket::new();
        // GH #1209 (re-check M1): announced from `Drop`, i.e. strictly before
        // the join handle resolves and the watcher sends `CellDied` -- the
        // same ordering the rescue relies on. Without the announcement the
        // colony could not tell this mailbox's late writes from those of a
        // newer cell at the same path; it dead-letters unannounced ones.
        if tx
            .try_send(crate::ColonyMsg::MailboxRescuedLate(LateReport {
                path: path.clone(),
                ticket: ticket.clone(),
                event: LateEvent::Opened,
            }))
            .is_err()
        {
            tracing::error!(
                path = %path.as_str(),
                "the colony inbox would not take the announcement of a late mailbox \
                 rescue -- writes through the outstanding permit will be dead-lettered"
            );
        }
        let tx = tx.clone();
        drop(runtime.spawn(async move {
            while let Some(first) = rx.recv().await {
                let mut late = vec![first];
                while let Ok(msg) = rx.try_recv() {
                    late.push(msg);
                }
                let count = late.len();
                let sent = tx
                    .send(crate::ColonyMsg::MailboxRescuedLate(LateReport {
                        path: path.clone(),
                        ticket: ticket.clone(),
                        event: LateEvent::Writes(late),
                    }))
                    .await;
                match sent {
                    Ok(()) => tracing::warn!(
                        path = %path.as_str(),
                        count,
                        "a write through a permit taken before the cell died landed after \
                         the rescue — handed to the colony behind it"
                    ),
                    Err(_) => {
                        tracing::error!(
                            path = %path.as_str(),
                            count,
                            "a write through a permit taken before the cell died landed \
                             after the rescue and the colony is gone — messages lost"
                        );
                        return;
                    }
                }
            }
            // The colony forgets the ticket's route: nothing can follow.
            let _ = tx
                .send(crate::ColonyMsg::MailboxRescuedLate(LateReport {
                    path,
                    ticket,
                    event: LateEvent::Closed,
                }))
                .await;
        }));
    }

    /// Hand the drained remainder to the colony (see `Drop`).
    fn hand_over(&self, tx: &mpsc::Sender<crate::ColonyMsg>, rescued: Vec<Message>) {
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

/// GH #1068 (#1209, re-check M1): names ONE dying mailbox's late task, so the
/// colony can tell its writes from those of any later cell at the same path.
/// Compared by identity only; the `Arc` is never shared state, it carries no
/// data and nobody mutates through it.
#[derive(Clone, Debug)]
pub(crate) struct LateTicket(std::sync::Arc<()>);

impl LateTicket {
    pub(crate) fn new() -> Self {
        Self(std::sync::Arc::new(()))
    }

    fn is(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a dying cell's late-write task reports to the colony
/// ([`crate::ColonyMsg::MailboxRescuedLate`]). Opaque outside this crate.
#[derive(Debug)]
pub struct LateReport {
    pub(crate) path: Path,
    pub(crate) ticket: LateTicket,
    pub(crate) event: LateEvent,
}

/// The three things a late task says, in this order.
#[derive(Debug)]
pub(crate) enum LateEvent {
    /// Sent from the guard's `Drop`, before the `CellDied`.
    Opened,
    /// Writes that landed in the closed mailbox, oldest first.
    Writes(Vec<Message>),
    /// The task ended: no permit is out any more.
    Closed,
}

/// Where the writes of one announced late task go.
#[derive(Debug)]
pub(crate) enum LateRoute {
    /// The `CellDied` of the dying cell is still to come: behind its rescue.
    AwaitingDeath,
    /// The death restarted the cell in place: this successor's mailbox, and
    /// only while it is still the one registered and running. Weak, so the
    /// route never keeps a mailbox open; it is upgraded for the comparison.
    Successor(mpsc::WeakSender<Message>),
    /// The death left no successor, or was a parked twin's (GH #688).
    DeadLetter,
}

/// The colony loop's record of announced late tasks (GH #1209), keyed by
/// ticket. Owned by the loop like the rescue map; an entry lives from the
/// announcement to the task's `Closed`, which is microseconds in practice.
#[derive(Debug, Default)]
pub(crate) struct LateRescues {
    open: Vec<(LateTicket, Path, LateRoute)>,
}

impl LateRescues {
    /// A late task for the mailbox that dies at `path` was announced.
    pub(crate) fn opened(&mut self, ticket: LateTicket, path: Path) {
        self.open.push((ticket, path, LateRoute::AwaitingDeath));
    }

    /// The `CellDied` for `path` was handled: every task still awaiting that
    /// death learns where its later writes go.
    pub(crate) fn settle(&mut self, path: &Path, successor: Option<&mpsc::Sender<Message>>) {
        for (_, at, route) in &mut self.open {
            if at == path && matches!(route, LateRoute::AwaitingDeath) {
                *route = match successor {
                    Some(sender) => LateRoute::Successor(sender.downgrade()),
                    None => LateRoute::DeadLetter,
                };
            }
        }
    }

    /// The route of `ticket`, or `None` when it was never announced.
    pub(crate) fn route(&self, ticket: &LateTicket) -> Option<&LateRoute> {
        self.open
            .iter()
            .find(|(t, ..)| t.is(ticket))
            .map(|(.., route)| route)
    }

    /// The task behind `ticket` ended.
    pub(crate) fn closed(&mut self, ticket: &LateTicket) {
        self.open.retain(|(t, ..)| !t.is(ticket));
    }

    /// Late tasks still open: each may yet deliver a write it owes (re-check
    /// H1) -- work the drain counts like a message waiting in a mailbox.
    pub(crate) fn pending(&self) -> usize {
        self.open.len()
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

    /// Every rescued message id, from both rescue parts, until the last
    /// colony sender (the guard's and its late task's) is gone -- an event,
    /// not a clock.
    async fn rescued_ids(
        colony_rx: &mut mpsc::Receiver<crate::ColonyMsg>,
    ) -> Vec<meclaw_core::Uuid> {
        let mut ids = Vec::new();
        while let Some(msg) = colony_rx.recv().await {
            match msg {
                crate::ColonyMsg::MailboxRescued { path, messages } => {
                    assert_eq!(path, Path::new("/c"));
                    ids.extend(messages.iter().map(|m| m.id));
                }
                crate::ColonyMsg::MailboxRescuedLate(report) => {
                    assert_eq!(report.path, Path::new("/c"));
                    if let LateEvent::Writes(messages) = report.event {
                        ids.extend(messages.iter().map(|m| m.id));
                    }
                }
                _ => panic!("only rescues travel from a guard to the colony"),
            }
        }
        ids
    }

    /// GH #1068 (residual, #1209): a sender that took its permit BEFORE the
    /// guard closed the mailbox and writes AFTER it -- the colony's thread
    /// descheduled between `reserve` and the write inside `Sender::send` --
    /// still lands its message in the queue. The sender saw `Ok`, so nobody
    /// parks or dead-letters it; a guard that stopped at the first empty
    /// `try_recv` dropped it with the receiver (gh18: counter 3 of 4). The
    /// guard waits for that write (in a task, not in `Drop`), so the rescue
    /// carries it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_write_through_a_permit_taken_before_the_close_is_rescued() {
        let (tx, rx) = mpsc::channel::<Message>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        let late = MessageBuilder::new(Path::new("/c")).build();
        let late_id = late.id;
        let permit = tx
            .clone()
            .reserve_owned()
            .await
            .expect("an open mailbox hands out a permit");
        let guard = MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx));
        // The cell's task dies on another runtime thread while the permit is
        // out (a cell task always dies on one: unwind or abort).
        let runtime = tokio::runtime::Handle::current();
        let dropper = std::thread::spawn(move || {
            let _in_runtime = runtime.enter();
            drop(guard)
        });
        // Event, not a clock: the guard has closed the mailbox.
        tx.closed().await;
        permit.send(late);
        dropper.join().expect("the guard's drop must not panic");
        assert_eq!(
            rescued_ids(&mut colony_rx).await,
            vec![late_id],
            "GH #1068: the rescue carries the message written through the early permit"
        );
    }

    /// GH #1068 (#1209): the guard never waits for an outstanding permit
    /// inside `Drop`. The cell's last read assigns the freed slot to a queued
    /// sender, which has not run since -- here it is this very task, as it
    /// would be the task in the dying worker's LIFO slot. A drop that spun
    /// until the permit came back would never return; it must, and the woken
    /// sender gets the message back (`Err`) to park or dead-letter itself.
    #[tokio::test]
    async fn a_sender_queued_when_the_cell_dies_never_stalls_the_drop() {
        use std::future::Future;
        let (tx, rx) = mpsc::channel::<Message>(1);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        tx.send(MessageBuilder::new(Path::new("/c")).build())
            .await
            .unwrap();
        {
            let mut reserve = std::pin::pin!(tx.reserve());
            let queued = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(reserve.as_mut().poll(cx).is_pending())
            })
            .await;
            assert!(queued, "the full mailbox queues the sender");
            let mut guard = MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx));
            assert!(
                guard.recv().await.is_some(),
                "the read frees the slot the queued sender is assigned"
            );
            drop(guard);
            assert!(
                reserve.await.is_err(),
                "the closed mailbox refuses the woken sender, which keeps its message"
            );
        }
        // The dead mailbox's last sender goes (the respawn replaces the
        // registry handle): the late task has nothing more to wait for.
        drop(tx);
        assert!(
            rescued_ids(&mut colony_rx).await.is_empty(),
            "nothing was written, nothing is rescued"
        );
    }

    /// GH #1209 (re-check M1): the late task is announced from `Drop` itself
    /// -- before the drop returns, so before any `CellDied` can follow -- and
    /// all three of its reports carry the same ticket: `Opened`, the writes,
    /// then `Closed` once no permit is out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_late_task_is_announced_by_the_drop_and_closes_under_one_ticket() {
        let (tx, rx) = mpsc::channel::<Message>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(4);
        let late = MessageBuilder::new(Path::new("/c")).build();
        let late_id = late.id;
        let permit = tx
            .clone()
            .reserve_owned()
            .await
            .expect("an open mailbox hands out a permit");
        drop(MailboxGuard::new(Path::new("/c"), rx, Some(colony_tx)));
        let Ok(crate::ColonyMsg::MailboxRescuedLate(opened)) = colony_rx.try_recv() else {
            panic!("the drop announces the late task before it returns");
        };
        assert!(matches!(opened.event, LateEvent::Opened));
        permit.send(late);
        drop(tx);
        let mut events = Vec::new();
        while let Some(msg) = colony_rx.recv().await {
            let crate::ColonyMsg::MailboxRescuedLate(report) = msg else {
                panic!("only the late task reports now");
            };
            assert!(report.ticket.is(&opened.ticket), "one ticket throughout");
            events.push(report.event);
        }
        assert!(
            matches!(events.as_slice(), [LateEvent::Writes(w), LateEvent::Closed]
                if w.iter().map(|m| m.id).eq([late_id])),
            "the write, then the end: {events:?}"
        );
    }

    /// GH #1209: the record settles per path and per ticket -- only tasks
    /// still awaiting that path's death learn the route, and a closed ticket
    /// is forgotten.
    #[test]
    fn late_rescues_settle_per_path_and_forget_closed_tickets() {
        let (succ, _rx) = mpsc::channel::<Message>(1);
        let mut late = LateRescues::default();
        let (a, b) = (LateTicket::new(), LateTicket::new());
        late.opened(a.clone(), Path::new("/a"));
        late.opened(b.clone(), Path::new("/b"));
        late.settle(&Path::new("/a"), Some(&succ));
        late.settle(&Path::new("/a"), None);
        assert!(
            matches!(late.route(&a), Some(LateRoute::Successor(_))),
            "settled once"
        );
        assert!(matches!(late.route(&b), Some(LateRoute::AwaitingDeath)));
        late.settle(&Path::new("/b"), None);
        assert!(matches!(late.route(&b), Some(LateRoute::DeadLetter)));
        assert_eq!(late.pending(), 2, "both tasks still owe their writes");
        late.closed(&a);
        assert!(late.route(&a).is_none() && late.route(&b).is_some());
        assert_eq!(late.pending(), 1);
        assert!(late.route(&LateTicket::new()).is_none(), "unannounced");
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
