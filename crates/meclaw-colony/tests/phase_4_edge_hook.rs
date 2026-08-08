//! Phase-4 edge hook: the cell target is overridden by the edge.

use meclaw_core::{Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3b::CaptureCell;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_overrides_cell_emitted_target() {
    let colony = ColonyHandle::new();

    let (recv_tx, mut recv_rx) = mpsc::channel(16);
    colony
        .spawn(Path::new("/receiver"), move || {
            CaptureCell::new(recv_tx.clone())
        })
        .await;

    let (dummy_tx, mut dummy_rx) = mpsc::channel(16);
    colony
        .spawn(Path::new("/dummy"), move || {
            CaptureCell::new(dummy_tx.clone())
        })
        .await;

    colony
        .spawn(Path::new("/src"), move || {
            EchoMockCell::new(Path::new("/src")).echo_to(Path::new("/dummy"))
        })
        .await;

    // Edge /src -> /receiver overrides /src's echo_to=/dummy.
    colony
        .add_edge(Uuid::now_v7(), Path::new("/src"), Path::new("/receiver"))
        .await;

    let src = TestMessageBuilder::new("/src")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"hi"})])
        .build();
    colony.send_from(Path::new("/"), src).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut got_receiver = Vec::new();
    while let Ok(m) = recv_rx.try_recv() {
        got_receiver.push(m);
    }
    let mut got_dummy = Vec::new();
    while let Ok(m) = dummy_rx.try_recv() {
        got_dummy.push(m);
    }

    assert_eq!(
        got_receiver.len(),
        1,
        "edge target /receiver must get the message"
    );
    assert!(
        got_dummy.is_empty(),
        "cell-target /dummy must NOT receive — edge overrides"
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_edge_dead_letters_as_no_route() {
    // Ruling A1 (Phase-16 W2): without a matching out-edge, the cell's emission
    // no longer identity-routes to its emitted target — it dead-letters as
    // `no_route`. /dst must NOT receive (the former identity-fallback is gone).
    let colony = ColonyHandle::new();

    let (recv_tx, mut recv_rx) = mpsc::channel(16);
    colony
        .spawn(Path::new("/dst"), move || CaptureCell::new(recv_tx.clone()))
        .await;

    colony
        .spawn(Path::new("/src"), move || {
            EchoMockCell::new(Path::new("/src")).echo_to(Path::new("/dst"))
        })
        .await;

    let src = TestMessageBuilder::new("/src")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"hi"})])
        .build();
    colony.send_from(Path::new("/"), src).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut got = Vec::new();
    while let Ok(m) = recv_rx.try_recv() {
        got.push(m);
    }
    assert!(
        got.is_empty(),
        "without edge, the emission dead-letters as no_route — /dst must not receive"
    );

    let dls = colony.drain_dead_letters().await;
    let no_route: Vec<_> = dls
        .iter()
        .filter(|dl| dl.reason.as_code() == "no_route" && dl.sender_path.as_str() == "/src")
        .collect();
    assert_eq!(
        no_route.len(),
        1,
        "exactly one no_route DLQ entry from /src for the unrouted emission, got: {:?}",
        dls.iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        no_route[0].original_target.as_str(),
        "/dst",
        "dying edge target"
    );

    colony.shutdown().await;
}
