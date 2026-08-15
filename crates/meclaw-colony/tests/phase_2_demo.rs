//! Phase-2 demo: the three assertions from PROGRESS.md row "Phase 2".
//!
//! Each test stands on its own (separate topology, separate colony) so a
//! failure cleanly identifies which spec invariant broke.

use meclaw_colony::DeadLetterReason;
use meclaw_core::Path;
use meclaw_testing::MessageBuilder;
use meclaw_testing::topologies::phase_2::build_phase_2_topology;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_2_routes_absolute_path_to_registered_cell() {
    let mut topo = build_phase_2_topology().await;
    topo.colony
        .send_from(Path::new("/"), MessageBuilder::new("/a/b/c").build())
        .await;
    let p = topo.tap_rx.recv().await.expect("/a/b/c must tap");
    assert_eq!(p.as_str(), "/a/b/c");
    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_2_routes_relative_dotdot_target_via_central_route() {
    let mut topo = build_phase_2_topology().await;
    // Inject at /a/b/d → forwarder emits target "../c" → colony resolves to /a/b/c.
    topo.colony
        .send_from(Path::new("/"), MessageBuilder::new("/a/b/d").build())
        .await;
    let p1 = topo.tap_rx.recv().await.unwrap();
    let p2 = topo.tap_rx.recv().await.unwrap();
    assert_eq!(
        [p1.as_str(), p2.as_str()],
        ["/a/b/d", "/a/b/c"],
        "tap order proves the forwarder ran first and ../c resolved to /a/b/c"
    );
    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_2_unresolved_target_lands_in_dead_letters() {
    let topo = build_phase_2_topology().await;
    topo.colony
        .send_from(Path::new("/"), MessageBuilder::new("/missing").build())
        .await;
    let dls = topo.colony.drain_dead_letters().await;
    assert_eq!(dls.len(), 1, "exactly one dead-letter expected");
    assert_eq!(dls[0].resolved_target.as_str(), "/missing");
    assert_eq!(dls[0].original_target.as_str(), "/missing");
    assert_eq!(dls[0].sender_path.as_str(), "/");
    assert_eq!(dls[0].reason, DeadLetterReason::UnresolvedPath);
    assert!(matches!(dls[0].message.body, meclaw_core::Body::Inline(_)));
    topo.colony.shutdown().await;
}
