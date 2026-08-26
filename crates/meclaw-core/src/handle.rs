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
