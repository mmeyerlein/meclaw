//! Phase-2 demo topology builder.
//!
//! Provisions:
//!   - `/a/b/c` → terminal echo, taps own path
//!   - `/a/b/d` → forwarder echo with relative target `../c`, taps own path
//!
//! Used by the phase-2 integration tests to assert that:
//!   - direct route to `/a/b/c` works (absolute case)
//!   - emit from `/a/b/d` with target `../c` resolves to `/a/b/c` (relative case)
//!   - route to `/missing` lands in `/colony/dead_letters` (cascade case)

use crate::ColonyHandle;
use crate::mocks::EchoMockCell;
use meclaw_core::{Path, Uuid};
use tokio::sync::mpsc;

pub struct Phase2Topology {
    pub colony: ColonyHandle,
    pub tap_rx: mpsc::Receiver<Path>,
}

pub async fn build_phase_2_topology() -> Phase2Topology {
    let colony = ColonyHandle::new();
    let (tap_tx, tap_rx) = mpsc::channel(64);

    // /a/b/c — terminal
    let tap_c = tap_tx.clone();
    colony
        .spawn(Path::new("/a/b/c"), move || {
            EchoMockCell::new(Path::new("/a/b/c")).tap_to(tap_c.clone())
        })
        .await;

    // /a/b/d — forwarder to ../c
    let tap_d = tap_tx.clone();
    colony
        .spawn(Path::new("/a/b/d"), move || {
            EchoMockCell::new(Path::new("/a/b/d"))
                .echo_to(Path::new("../c"))
                .tap_to(tap_d.clone())
        })
        .await;

    // Wire /a/b/d -> ../c as the unconditional catch-all out-edge. Ruling A1
    // removed the implicit identity-fallback that used to centrally resolve a
    // cell's relative emission target. Keeping the edge target RELATIVE (`../c`)
    // preserves exactly what this topology proves: the outputs-arm routes the
    // edge target through route()'s central relative-resolution (`../c` from
    // sender /a/b/d resolves to /a/b/c) — now via the edge model instead of the
    // fallback. The forwarder's only emission is this echo, so nothing else is
    // rerouted.
    colony
        .add_edge(Uuid::now_v7(), Path::new("/a/b/d"), Path::new("../c"))
        .await;

    Phase2Topology { colony, tap_rx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageBuilder;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn topology_routes_relative_emit_to_target_via_dotdot() {
        let mut topo = build_phase_2_topology().await;
        topo.colony
            .send_from(Path::new("/"), MessageBuilder::new("/a/b/d").build())
            .await;
        let p1 = topo.tap_rx.recv().await.unwrap();
        let p2 = topo.tap_rx.recv().await.unwrap();
        assert_eq!([p1.as_str(), p2.as_str()], ["/a/b/d", "/a/b/c"]);
        topo.colony.shutdown().await;
    }
}
