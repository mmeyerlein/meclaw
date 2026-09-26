//! Uniform actor handle: cheap-clone wrapper around an mpsc sender plus the path.

use crate::message::Message;
use crate::path::Path;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ActorHandle {
    pub path: Path,
    sender: mpsc::Sender<Message>,
}

impl ActorHandle {
    pub fn new(path: Path, sender: mpsc::Sender<Message>) -> Self {
        Self { path, sender }
    }

    /// Send a message to this cell's mailbox. Backpressure applies via
    /// `mpsc::Sender::send`.
    ///
    /// The error is BOXED (GH #406). `SendError<T>` is large precisely because
    /// it hands the undelivered value back, and `Message` is one of the biggest
    /// types in the substrate — an unboxed `Err` variant would make every
    /// success on this path carry the failure's footprint, which is what
    /// `clippy::result_large_err` objects to. Handing the message back is worth
    /// keeping (a full or gone mailbox is exactly where a caller may want to
    /// see what did not arrive), so the value survives and the allocation moves
    /// into the failure branch: `Box::new` runs only when the send has already
    /// failed, i.e. when a mailbox is gone and one allocation is the least of
    /// it. Nothing on the wire changes — this is a crate-internal signature.
    pub async fn send(&self, msg: Message) -> Result<(), Box<mpsc::error::SendError<Message>>> {
        self.sender.send(msg).await.map_err(Box::new)
    }

    /// Put a message into this cell's mailbox only if there is room RIGHT NOW.
    ///
    /// GH #850: the colony's routing loop must never wait on a mailbox, so the
    /// one place outside the router that used to `send().await` into a mailbox
    /// (the hand-over of a rescued mailbox to a respawned cell) now tries and,
    /// on a full mailbox, hands the rest to the cell's overflow instead. The
    /// error is boxed for the same reason as in [`Self::send`] (GH #406) and
    /// hands the message back, so nothing is lost on `Full` or `Closed`.
    pub fn try_send(&self, msg: Message) -> Result<(), Box<mpsc::error::TrySendError<Message>>> {
        self.sender.try_send(msg).map_err(Box::new)
    }

    /// A second sender into this cell's mailbox.
    ///
    /// GH #850: only the overflow of a cell uses it. Its drain task owns the
    /// cell's overflow queue and waits on `Sender::reserve` — outside the routing
    /// loop, so the loop never does. Every other producer goes through the
    /// registry entry that owns this handle; handing a clone to anything else
    /// would break the "one producer per mailbox" argument the overflow's
    /// ordering rests on.
    pub fn sender_clone(&self) -> mpsc::Sender<Message> {
        self.sender.clone()
    }

    /// Whether `other` feeds the same mailbox as this handle.
    ///
    /// GH #850: a cell's overflow remembers the mailbox its drain task delivers
    /// into. When the handle registered at the path no longer feeds that
    /// mailbox, the cell was displaced (a `replace_nodes` lift, a disconnect or
    /// a failure parking it on a fresh channel) — and the overflow is the
    /// displaced cell's, never the newcomer's.
    pub fn same_mailbox(&self, other: &mpsc::Sender<Message>) -> bool {
        self.sender.same_channel(other)
    }

    /// Configured bounded-mpsc capacity of this cell's mailbox (the value passed to `channel()`).
    pub fn max_capacity(&self) -> usize {
        self.sender.max_capacity()
    }

    /// Free slots in this cell's mailbox right now. `0` means the next
    /// [`Self::send`] will WAIT.
    ///
    /// GH #162: the routing loop is what waits, so a full mailbox stops the whole
    /// colony — and until this existed there was no way to say which mailbox that
    /// was without a SQLite client on `colony.db`. Read-only and cheap; the value
    /// is a snapshot and the caller must not treat it as a reservation.
    pub fn free_capacity(&self) -> usize {
        self.sender.capacity()
    }

    /// True once the mailbox behind this handle has been closed — its receiver
    /// dropped, or [`tokio::sync::mpsc::Receiver::close`] called on it.
    ///
    /// GH #682: the colony uses this to tell WHOSE mailbox a returned receiver
    /// is. A cell reports its stop under the path it was born with, and after
    /// a `replace_nodes` that path belongs to a different cell: a handle whose
    /// mailbox the returned receiver did not close is not the one that stopped.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_builder::MessageBuilder;

    #[tokio::test]
    async fn handle_max_capacity_returns_channel_size() {
        let (tx, _rx) = mpsc::channel(7);
        let h = ActorHandle::new(Path::new("/cell"), tx);
        assert_eq!(h.max_capacity(), 7);
    }

    /// GH #850: `try_send` fills a mailbox and then hands the message back
    /// instead of waiting; `sender_clone` reaches the same mailbox.
    #[tokio::test]
    async fn try_send_hands_the_message_back_when_the_mailbox_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let h = ActorHandle::new(Path::new("/cell"), tx);
        let first = MessageBuilder::new(Path::new("/cell")).build();
        let second = MessageBuilder::new(Path::new("/cell")).build();
        let second_id = second.id;
        h.try_send(first).unwrap();
        let back = h.try_send(second).unwrap_err();
        match *back {
            mpsc::error::TrySendError::Full(m) => assert_eq!(m.id, second_id),
            other => panic!("a full mailbox is Full, not {other:?}"),
        }
        rx.recv().await.unwrap();
        let third = MessageBuilder::new(Path::new("/cell")).build();
        let third_id = third.id;
        h.sender_clone().send(third).await.unwrap();
        assert_eq!(rx.recv().await.unwrap().id, third_id);
        assert!(
            h.same_mailbox(&h.sender_clone()),
            "a clone feeds the same mailbox"
        );
        let (other, _other_rx) = mpsc::channel::<Message>(1);
        assert!(!h.same_mailbox(&other), "another channel does not");
    }

    #[tokio::test]
    async fn handle_sends_message_to_receiver() {
        let (tx, mut rx) = mpsc::channel(4);
        let handle = ActorHandle::new(Path::new("/cell"), tx);
        let msg = MessageBuilder::new(Path::new("/cell")).build();
        handle.send(msg).await.unwrap();
        let m = rx.recv().await.unwrap();
        assert_eq!(m.target.as_str(), "/cell");
        assert!(matches!(m.body, crate::body::Body::Inline(_)));
    }
}
