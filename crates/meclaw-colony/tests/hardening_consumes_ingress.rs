//! Slice 2 (roadmap Z.136): the substrate-side required-consumes check at the
//! delivery boundary.
//!
//! Task 2.4: a cell with `consumes.hop.h1 required:true` in its contract is NOT
//! invoked when the message does not carry the key — with `reply_to` an error
//! message (`error_code: "consumes_violation"`) goes to the sender, without
//! `reply_to` the message lands in the DLQ
//! (`DeadLetterReason::ConsumesViolation`). Satisfied consumes and cells
//! without a declaration deliver normally (positive capture receipt, DLQ empty).
//!
//! Topology pattern as in `hardening_header_locality.rs` (FS fixtures with
//! contract blocks, `bootstrap_from_filesystem`, capture receipts, DLQ drain).
//! The echo consumer `/c` runs over the `cell_task` path (reply branch); the
//! stateful consumer `/m` (multi_update) runs over `cell_task_stateful` with a
//! wired `colony_inbox_tx` (DLQ branch) — the plain `cell_task` has no DLQ path
//! by convention.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, DeadLetterReason, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::{EchoCellFactory, MultiUpdateMockCellFactory};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "multi_update".to_string(),
            Arc::new(MultiUpdateMockCellFactory) as Arc<dyn CellFactory>,
        ),
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

/// Consumer topology (boots green, pattern `hardening_header_locality`):
/// producer `/p` with `emits.hop.h1` + boot edge `p → c` satisfies the 14-B
/// fan-in check of the consumer `/c` (`consumes.hop.h1 required:true`, echoes
/// to `/sink`). `/n` is the contract-less echo consumer (vacuous).
/// `/m` is the STATEFUL consumer (multi_update, same consumes block) for the
/// DLQ branch — the boot edge `p → m` satisfies its fan-in check too
/// (a participating required consumer needs ≥1 delivering in-edge);
/// the delivery boundary checks the ACTUAL message independently of that.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/p")).unwrap();
    std::fs::create_dir_all(td.join("main/c")).unwrap();
    std::fs::create_dir_all(td.join("main/n")).unwrap();
    std::fs::create_dir_all(td.join("main/m")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./p","to":"./c"},
            {"from":"./p","to":"./m"},
            {"from":"./c","to":"/sink"},
            {"from":"./n","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/p/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"string"}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"h1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/n/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/m/config.json"),
        r#"{"cell":{"type":"multi_update"},"params":{"sink_target":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"h1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// A UBF-conformant probe (phase-6 lesson: no InvalidUbfBody DLQ) to `target`.
fn ubf_probe(target: &str) -> MessageBuilder {
    MessageBuilder::new(Path::new(target)).body(Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": "consumes-probe"}]
    })))
}

/// reply_to set + required key missing → the cell does NOT run, reply_to
/// receives an error message with hop.error_code == "consumes_violation"
/// (the colony extracts `content.header` into the hop compartment of the
/// follow-up message, pattern `emit_backstop_timeout`/`message_timeout`). The
/// error reply is the ordering anchor; afterwards a SHORT bounded window on
/// /sink proves that the target's echo receipt DOES NOT ARRIVE.
///
/// W2b DIRECT-REPLY PIN (ruling A1, ruling 2026-06-12): `/c` now carries the
/// catch-all out-edge `./c→/sink` (write_topology). The `consumes_violation`
/// error reply MUST still go DIRECTLY to `/cap` (reply_to) — via
/// route_with_log, NOT over `/c`'s out-edges. Proven doubly here: (1) `/cap`
/// receives the reply (direct delivery), (2) `/sink` receives NOTHING (the
/// catch-all edge does NOT divert the error reply) and (3) there is no
/// `no_route` DLQ entry from `/c` (the reply was delivered, not routed into the
/// void).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_required_hop_key_with_reply_to_yields_error_reply() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    // Capture-Cells VOR Bootstrap registrieren (Anti-Cascade-Lesson).
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/cap"), move || CaptureCell::new(cap_tx.clone()))
        .await;
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /c: reply_to=/cap, WITHOUT h1 in the hop compartment.
    let probe = ubf_probe("/c").reply_to(Path::new("/cap")).build();
    h.send(probe).await;

    // Error reply at reply_to (30s failure-marker convention).
    let received = tokio::time::timeout(Duration::from_secs(30), cap_rx.recv())
        .await
        .expect("/cap must receive the consumes_violation error reply within 30s")
        .expect("CaptureCell channel must deliver a message");
    assert_eq!(received.target.as_str(), "/cap");
    assert_eq!(
        received.headers.hop.get("error_code"),
        Some(&json!("consumes_violation")),
        "hop.error_code must be consumes_violation, hop: {:?}",
        received.headers.hop
    );
    assert_eq!(
        received.headers.hop.get("finish_reason"),
        Some(&json!("error")),
        "hop.finish_reason must be error, hop: {:?}",
        received.headers.hop
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /cap, got {other:?}"),
    };
    assert!(
        body.contains("required consumes.hop 'h1' missing"),
        "the error reply must carry the reason: {body}"
    );

    // The target cell receipt DOES NOT ARRIVE: the error reply is the anchor —
    // had /c run, its echo would already be in the pipeline. Short bounded window.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), sink_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "/c must NOT have processed the message (no echo to /sink), got {no_receipt:?}"
    );

    // W2b direct-reply pin: the error reply was delivered DIRECTLY — no
    // no_route DLQ entry from /c (had it been edge-routed over /c→/sink, or
    // no_route'd without a match, there would be an entry here).
    let dls = h.drain_dead_letters().await;
    assert!(
        !dls.iter()
            .any(|d| d.reason.as_code() == "no_route" && d.sender_path.as_str() == "/c"),
        "the consumes_violation error reply must NOT no_route (direct delivery), got: {:?}",
        dls.iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

/// reply_to None + required key missing → a DLQ entry ConsumesViolation
/// (as_code "consumes_violation"), the cell does not run. The target is the
/// STATEFUL consumer /m (cell_task_stateful with colony_inbox_tx → DLQ path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_required_key_without_reply_to_dead_letters() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /m: WITHOUT reply_to, WITHOUT h1.
    let probe = ubf_probe("/m").build();
    h.send(probe).await;

    // DLQ poll with a 30s deadline (the DLQ entry travels asynchronously
    // cell_task → colony inbox); the drain is destructive → accumulate.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut collected = Vec::new();
    loop {
        collected.extend(h.drain_dead_letters().await);
        if collected
            .iter()
            .any(|d| d.reason == DeadLetterReason::ConsumesViolation)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no consumes_violation dead letter within 30s, got {collected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cv_count = collected
        .iter()
        .filter(|d| d.reason == DeadLetterReason::ConsumesViolation)
        .count();
    assert_eq!(
        cv_count, 1,
        "exactly ONE consumes_violation dead letter expected, got {collected:?}"
    );
    let dl = collected
        .iter()
        .find(|d| d.reason == DeadLetterReason::ConsumesViolation)
        .unwrap();
    assert_eq!(dl.reason, DeadLetterReason::ConsumesViolation);
    assert_eq!(dl.reason.as_code(), "consumes_violation");
    assert_eq!(dl.resolved_target.as_str(), "/m");

    // The cell did not run: multi_update would have emitted to /sink. The DLQ
    // entry is the anchor; a short bounded window afterwards.
    let no_receipt = tokio::time::timeout(Duration::from_millis(300), sink_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "/m must NOT have processed the message (no emission to /sink), got {no_receipt:?}"
    );

    h.shutdown().await;
}

/// Key present + type-correct in the hop compartment → handle() runs (positive
/// capture receipt with the /c echo turn at /sink), DLQ empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn satisfied_consumes_delivers_normally() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /c WITH h1 (string) in the hop compartment.
    let mut hop = Map::new();
    hop.insert("h1".into(), json!("v1"));
    let probe = ubf_probe("/c").hop(hop).build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive the echo receipt within 30s")
        .expect("CaptureCell channel must deliver a message");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /c"),
        "the receipt must carry the /c echo turn — proves /c RECEIVED the message \
         with consumes satisfied: {body}"
    );

    // DLQ guard AFTER the flow.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}

/// A cell without a consumes declaration stays unaffected (vacuous): a message
/// without any headers delivers normally (receipt), DLQ empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_without_consumes_is_unaffected() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, factories());

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("topology must boot green");

    // Probe to /n: no headers, no contract at the target.
    let probe = ubf_probe("/n").build();
    h.send(probe).await;

    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive the echo receipt within 30s")
        .expect("CaptureCell channel must deliver a message");
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /n"),
        "the receipt must carry the /n echo turn (vacuous consumes): {body}"
    );

    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");

    h.shutdown().await;
}
