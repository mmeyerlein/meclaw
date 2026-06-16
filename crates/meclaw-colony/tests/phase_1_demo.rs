//! Phase-1 demo integration tests.
//!
//! Mirror the PROGRESS.md acceptance criterion: 2 echo cells round-trip
//! through colony's routing; supervisor restarts a panicking cell with a
//! fresh mpsc pair.

use meclaw_testing::MessageBuilder;
use meclaw_testing::topologies::phase_1::build_phase_1_topology;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_echo_cells_chain_through_colony_routing() {
    let mut topo = build_phase_1_topology().await;

    topo.colony
        .send(MessageBuilder::new("/echo-a").build())
        .await;

    let p1 = tokio::time::timeout(Duration::from_secs(30), topo.tap_rx.recv())
        .await
        .expect("tap p1 timeout")
        .expect("tap p1 channel closed");
    let p2 = tokio::time::timeout(Duration::from_secs(30), topo.tap_rx.recv())
        .await
        .expect("tap p2 timeout")
        .expect("tap p2 channel closed");

    assert_eq!([p1.as_str(), p2.as_str()], ["/echo-a", "/echo-b"]);

    topo.colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn supervisor_restarts_panicking_cell_with_fresh_mpsc_pair() {
    use std::sync::atomic::Ordering;

    let mut topo = build_phase_1_topology().await;

    topo.colony
        .send(MessageBuilder::new("/flaky").build())
        .await;
    let p1 = tokio::time::timeout(Duration::from_secs(30), topo.tap_rx.recv())
        .await
        .expect("first tap timeout")
        .expect("first tap channel closed");
    assert_eq!(p1.as_str(), "/flaky");

    tokio::time::sleep(Duration::from_millis(200)).await;

    topo.colony
        .send(MessageBuilder::new("/flaky").build())
        .await;
    let p2 = tokio::time::timeout(Duration::from_secs(30), topo.tap_rx.recv())
        .await
        .expect("second tap timeout")
        .expect("second tap channel closed");
    assert_eq!(p2.as_str(), "/flaky");

    assert_eq!(topo.global_panic_calls.load(Ordering::SeqCst), 2);

    topo.colony.shutdown().await;
}
