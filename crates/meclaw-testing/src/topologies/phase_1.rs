//! Phase-1 demo topology builder.
//!
//! Provisions:
//!   - `/echo-a` → forwards to `/echo-b`, taps own path
//!   - `/echo-b` → terminal (no forward), taps own path
//!   - `/flaky` → panics on first call, taps own path; respawn-capable
//!
//! Returns the colony handle plus the tap receiver. Tests inject one
//! Route message at `/echo-a` and observe the tap sequence [/echo-a, /echo-b]
//! for the round-trip case, or [/flaky, /flaky] for the restart case.

use crate::ColonyHandle;
use crate::mocks::{EchoMockCell, FailOnDemandMockCell};
use meclaw_core::{Path, Uuid};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::mpsc;

/// Returned by [`build_phase_1_topology`]; holds all test-observable handles.
pub struct Phase1Topology {
    /// Running colony with the three registered cells.
    pub colony: ColonyHandle,
    /// Receives the `Path` of each cell that processes a message, in order.
    pub tap_rx: mpsc::Receiver<Path>,
    /// Cumulative call counter across all `/flaky` instances (including restarts).
    pub global_panic_calls: Arc<AtomicU32>,
}

/// Build and return a running phase-1 topology.
pub async fn build_phase_1_topology() -> Phase1Topology {
    let colony = ColonyHandle::new();
    let (tap_tx, tap_rx) = mpsc::channel(64);
    let panic_calls = Arc::new(AtomicU32::new(0));

    // /echo-a forwards to /echo-b
    let tap_a = tap_tx.clone();
    colony
        .spawn(Path::new("/echo-a"), move || {
            EchoMockCell::new(Path::new("/echo-a"))
                .emitted_target(Path::new("/echo-b"))
                .tap_to(tap_a.clone())
        })
        .await;

    // /echo-b is terminal
    let tap_b = tap_tx.clone();
    colony
        .spawn(Path::new("/echo-b"), move || {
            EchoMockCell::new(Path::new("/echo-b")).tap_to(tap_b.clone())
        })
        .await;

    // /flaky panics on first call per instance — fresh instance on respawn resets local counter
    let tap_f = tap_tx.clone();
    let pc = panic_calls.clone();
    colony
        .spawn(Path::new("/flaky"), move || {
            FailOnDemandMockCell::new(Path::new("/flaky"), 1, pc.clone()).tap_to(tap_f.clone())
        })
        .await;

    // Wire /echo-a -> /echo-b as an unconditional catch-all out-edge. This is the
    // settable default edge that replaces the former implicit identity-fallback
    // delivery (Phase-16 W2 / Ruling A1: a cell emission without a matching out-edge
    // now dead-letters as `no_route` instead of being delivered to its echo target).
    // /echo-a's only emission is the echo to /echo-b, so the catch-all does not
    // re-route any other normal emission.
    colony
        .add_edge(Uuid::now_v7(), Path::new("/echo-a"), Path::new("/echo-b"))
        .await;

    Phase1Topology {
        colony,
        tap_rx,
        global_panic_calls: panic_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageBuilder;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn topology_routes_echo_chain() {
        let mut topo = build_phase_1_topology().await;
        topo.colony
            .send(MessageBuilder::new("/echo-a").build())
            .await;
        let p1 = topo.tap_rx.recv().await.unwrap();
        let p2 = topo.tap_rx.recv().await.unwrap();
        assert_eq!([p1.as_str(), p2.as_str()], ["/echo-a", "/echo-b"]);
        topo.colony.shutdown().await;
    }
}
