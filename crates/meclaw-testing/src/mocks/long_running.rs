//! `ReceiptMockLongRunningCell` — Phase-10 substrate test fixture.
//!
//! **Test instrumentation only.** The `tokio::Mutex` over shared observation
//! state is NOT a production pattern — production long-running cells hold state
//! exclusively in the handler sub-task (phase-1 discipline: no
//! `Mutex`/`RwLock`/atomics over cell state). Long-running cell implementations
//! in 10-B/C/D may take this mock as a structural template, **but must leave the
//! mutex path out**.

use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Mutex, mpsc};

/// Provider event frame for `ReceiptMockLongRunningCell` tests.
#[derive(Debug, Clone)]
pub struct MockEvent(pub String);

/// Reconfigure hint frame for `ReceiptMockLongRunningCell` tests.
#[derive(Debug, Clone)]
pub struct MockReconfig(pub String);

/// Mock long-running cell — records handle/handle_event calls, mirrors
/// reconfig hints, supports panic injection.
///
/// **Test instrumentation only.** Cf. the module docs — the shared `Mutex`
/// over reconfigs is not a production pattern.
#[derive(Clone)]
pub struct ReceiptMockLongRunningCell {
    /// Counts `handle()` invocations (shared across clones).
    pub handle_calls: Arc<AtomicUsize>,
    /// Counts `handle_event()` invocations (shared across clones).
    pub event_calls: Arc<AtomicUsize>,
    /// All reconfig hints received in `run_io` (test-observable).
    pub reconfigs_seen: Arc<Mutex<Vec<MockReconfig>>>,
    inbound_event_rx: Arc<Mutex<Option<mpsc::Receiver<MockEvent>>>>,
    /// Test injection: if true, `run_io` panics immediately on start.
    pub panic_in_run_io: bool,
    /// Test injection: if `Some(n)`, `handle` panics after `n` successful calls.
    pub panic_in_handle_after: Option<usize>,
    /// B4-backstop demo: sleep this many ms inside `handle()` before emitting.
    /// Used to prove that LR cells are NOT subject to the `cell.message_timeout`
    /// backstop — a 400 ms handle survives a 100 ms `message_timeout` config.
    pub sleep_in_handle_ms: u64,
    /// B4-backstop demo: emit a UBF receipt to this path after the optional
    /// sleep. `None` → no emission (default, preserves existing behaviour).
    pub echo_to: Option<meclaw_core::Path>,
}

/// Owned I/O state moved into the `run_io` sub-task.
pub struct ReceiptMockIo {
    rx: mpsc::Receiver<MockEvent>,
    panic: bool,
    seen_reconfigs: Arc<Mutex<Vec<MockReconfig>>>,
}

impl ReceiptMockLongRunningCell {
    /// Returns the mock plus a sender that tests use to inject provider
    /// events (forwarded through the substrate into `handle_event`).
    pub fn new() -> (Self, mpsc::Sender<MockEvent>) {
        let (tx, rx) = mpsc::channel(8);
        let cell = Self {
            handle_calls: Arc::new(AtomicUsize::new(0)),
            event_calls: Arc::new(AtomicUsize::new(0)),
            reconfigs_seen: Arc::new(Mutex::new(Vec::new())),
            inbound_event_rx: Arc::new(Mutex::new(Some(rx))),
            panic_in_run_io: false,
            panic_in_handle_after: None,
            sleep_in_handle_ms: 0,
            echo_to: None,
        };
        (cell, tx)
    }

    /// Mark the mock so its `run_io` panics immediately.
    pub fn with_panic_in_run_io(mut self) -> Self {
        self.panic_in_run_io = true;
        self
    }

    /// Mark the mock so its `handle` panics after `n` successful calls
    /// (n=1 panics on the first call).
    pub fn with_panic_in_handle_after(mut self, n: usize) -> Self {
        self.panic_in_handle_after = Some(n);
        self
    }
}

impl LongRunningCell for ReceiptMockLongRunningCell {
    type Event = MockEvent;
    type Reconfig = MockReconfig;
    type Io = ReceiptMockIo;

    fn split_io(&mut self) -> Self::Io {
        let rx = self
            .inbound_event_rx
            .try_lock()
            .expect("split_io must run before any concurrent access")
            .take()
            .expect("split_io called twice on the same cell");
        ReceiptMockIo {
            rx,
            panic: self.panic_in_run_io,
            seen_reconfigs: self.reconfigs_seen.clone(),
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        mut io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        mut reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            if io.panic {
                panic!("ReceiptMockLongRunningCell: run_io panic (test injection)");
            }
            loop {
                tokio::select! {
                    ev = io.rx.recv() => match ev {
                        Some(e) => { if events_tx.send(e).await.is_err() { break; } }
                        None => break,
                    },
                    rc = reconfig_rx.recv() => match rc {
                        Some(r) => io.seen_reconfigs.lock().await.push(r),
                        None => break,
                    },
                }
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let n = self
                .handle_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            if Some(n) == self.panic_in_handle_after {
                panic!("ReceiptMockLongRunningCell: handle panic after {n} calls (test injection)");
            }
            // B4 backstop demo: optional in-handle sleep. LR tasks have no
            // B-backstop wrapper, so this completes regardless of any
            // `cell.message_timeout` configured on the registry entry.
            if self.sleep_in_handle_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.sleep_in_handle_ms)).await;
            }
            // Optional positive-receipt emission for B4 demo.
            if let Some(ref target) = self.echo_to {
                let _ = sink
                    .push(meclaw_core::CellOutput {
                        target: target.clone(),
                        content: meclaw_core::serde_json::json!({
                            "header": {"done": true},
                            "messages": [
                                {"origin": "assistant", "type": "text",
                                 "text": "lr-handle slept and finished"}
                            ]
                        }),
                    })
                    .await;
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        _event: Self::Event,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            self.event_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn new_returns_mock_and_event_injector() {
        let (mock, _inject_tx) = ReceiptMockLongRunningCell::new();
        assert_eq!(mock.handle_calls.load(Ordering::SeqCst), 0);
        assert_eq!(mock.event_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn builders_set_panic_flags() {
        let (m, _) = ReceiptMockLongRunningCell::new();
        let m = m.with_panic_in_run_io();
        assert!(m.panic_in_run_io);

        let (m, _) = ReceiptMockLongRunningCell::new();
        let m = m.with_panic_in_handle_after(2);
        assert_eq!(m.panic_in_handle_after, Some(2));
    }
}
