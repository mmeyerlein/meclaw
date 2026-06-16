//! Cell that panics on the N-th `handle()` call (1-based), allowing
//! deterministic supervisor-restart tests.
//!
//! Internal counter survives the cell instance, so a fresh instance
//! created by the supervisor on restart resets to 1. The N-th call
//! check uses an injected `AtomicU32` shared via factory closure for
//! tests that need to observe the global call count across restarts.

use meclaw_core::{Cell, Message, OutputSink, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::mpsc;

/// Mock cell that panics deterministically on the N-th `handle()` call.
pub struct FailOnDemandMockCell {
    own_path: Path,
    panic_at_call: u32,
    local_calls: u32,
    global_calls: Arc<AtomicU32>,
    tap_to: Option<mpsc::Sender<Path>>,
}

impl FailOnDemandMockCell {
    /// Create a new cell that panics on the `panic_at_call`-th invocation.
    pub fn new(own_path: Path, panic_at_call: u32, global_calls: Arc<AtomicU32>) -> Self {
        Self {
            own_path,
            panic_at_call,
            local_calls: 0,
            global_calls,
            tap_to: None,
        }
    }

    /// Attach a tap sender; the cell's own path is sent before each handle call.
    pub fn tap_to(mut self, tap: mpsc::Sender<Path>) -> Self {
        self.tap_to = Some(tap);
        self
    }
}

impl Cell for FailOnDemandMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        _msg: Message,
        _sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        async move {
            self.local_calls += 1;
            let global = self.global_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(tap) = &self.tap_to {
                let _ = tap.send(self.own_path.clone()).await;
            }
            if self.local_calls == self.panic_at_call {
                panic!(
                    "FailOnDemandMockCell panic at local call {} (global {})",
                    self.local_calls, global
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{CellEmission, MessageBuilder, OutputSink, Path, Uuid};

    #[tokio::test]
    async fn fail_on_demand_panics_at_nth_call() {
        let calls = Arc::new(AtomicU32::new(0));
        let cell_calls = calls.clone();
        let join = tokio::spawn(async move {
            let mut cell = FailOnDemandMockCell::new(Path::new("/f"), 2, cell_calls);
            let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(4);
            let sink = OutputSink::new(
                out_tx,
                Path::new("/f"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                10,
                meclaw_core::Headers::new(),
                None,
            );
            let msg = MessageBuilder::new(Path::new("/f")).build();
            cell.handle(msg.clone(), &sink).await;
            cell.handle(msg, &sink).await;
        });
        let res = join.await;
        assert!(res.is_err(), "task must panic");
        assert!(res.unwrap_err().is_panic());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
