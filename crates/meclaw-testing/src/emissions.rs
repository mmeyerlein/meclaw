//! GH #1015: reading a cell's raw output channel without its consume marks.
//!
//! Every cell task, with or without a colony inbox, closes every handled
//! delivery with a consume mark (`CellEmission::consumed_mark_id().is_some()`) on its OUTPUT
//! channel. The colony consumes the mark itself; a test that reads the raw
//! channel sees it as an extra emission. These helpers return the next
//! emission that is NOT a mark, so an assertion keeps saying the same thing it
//! said before the marks existed.

use meclaw_core::CellEmission;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

/// Mark-skipping reads on a cell's output channel.
#[allow(async_fn_in_trait)]
pub trait EmissionsExt {
    /// The next emission that is not a consume mark; `None` when the channel closed.
    async fn recv_answer(&mut self) -> Option<CellEmission>;
    /// The next already-queued emission that is not a consume mark.
    fn try_recv_answer(&mut self) -> Result<CellEmission, TryRecvError>;
}

impl EmissionsExt for mpsc::Receiver<CellEmission> {
    async fn recv_answer(&mut self) -> Option<CellEmission> {
        loop {
            let em = self.recv().await?;
            if em.consumed_mark_id().is_none() {
                return Some(em);
            }
        }
    }

    fn try_recv_answer(&mut self) -> Result<CellEmission, TryRecvError> {
        loop {
            let em = self.try_recv()?;
            if em.consumed_mark_id().is_none() {
                return Ok(em);
            }
        }
    }
}
