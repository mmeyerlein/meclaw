//! Phase-16 W2 (Rulings A1 + A2): the implicit identity-fallback in the
//! outputs-arm is gone. A cell emission that matches no out-edge dead-letters
//! as `no_route` (self-locating: trace_id + created_at + sender + dying edge);
//! a wired catch-all out-edge delivers it instead (no DLQ).

use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn factories() -> CellFactoryRegistry {
    let mut r: CellFactoryRegistry = HashMap::new();
    let arc: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    r.insert("echo".to_string(), arc);
    r
}

/// (a) A cell emission that matches NO out-edge dead-letters as `no_route` —
/// NOT the old identity-fallback (which delivered to the cell's own emission
/// target). The entry is self-locating: sender + dying edge (sender→target) +
/// trace_id + a non-zero created_at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_emission_without_matching_edge_dead_letters_as_no_route() {
    let td = TempDir::new().unwrap();
    // Root hive with NO edges — /emitter's echo target is unrouted.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // /emitter emits to /downstream; there is no out-edge from /emitter.
    // GH #224: this is exactly the shape a first-time colony test writes by
    // accident. `emitted_target` names the field on the emission, not a route —
    // without the out-edge the emission dies here rather than reaching
    // /downstream. The param is deliberately not called `echo_to` any more.
    write(
        td.path(),
        "main/emitter/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/downstream"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect("bootstrap must succeed");

    let src = TestMessageBuilder::new("/emitter")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"hi"})])
        .build();
    colony.send_from(Path::new("/"), src).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let dls = colony.drain_dead_letters().await;
    let no_route: Vec<_> = dls
        .iter()
        .filter(|dl| dl.reason.as_code() == "no_route")
        .collect();
    assert_eq!(
        no_route.len(),
        1,
        "exactly one no_route DLQ entry for the unrouted emission, got: {:?}",
        dls.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
    );
    let dl = no_route[0];
    // Self-locating four fields.
    assert_eq!(dl.sender_path.as_str(), "/emitter", "sender");
    assert_eq!(
        dl.original_target.as_str(),
        "/downstream",
        "dying edge start"
    );
    assert_eq!(dl.resolved_target.as_str(), "/downstream", "dying edge end");
    assert!(!dl.message.trace_id.is_nil(), "trace_id present");
    assert!(dl.message.created_at > 0, "created_at stamped");

    colony.shutdown().await;
}

/// (b) The SAME unrouted emission, but with a wired unconditional catch-all
/// out-edge from /emitter, is delivered via the edge — NO dead letter. The
/// catch-all edge is the settable "default edge" (Ruling A1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catch_all_out_edge_delivers_emission_without_dead_letter() {
    let td = TempDir::new().unwrap();
    // Root hive wires an unconditional catch-all /emitter -> /sink.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":"./emitter","to":"./sink"}
          ]}}}"#,
    );
    write(
        td.path(),
        "main/emitter/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/nowhere"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /sink is terminal: echoes to a non-existent /void so its receipt is
    // observable as a /void DLQ entry (proves the catch-all reached /sink).
    write(
        td.path(),
        "main/sink/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/void"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect("bootstrap must succeed");

    let src = TestMessageBuilder::new("/emitter")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"hi"})])
        .build();
    colony.send_from(Path::new("/"), src).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let dls = colony.drain_dead_letters().await;
    // The catch-all delivered /emitter -> /sink, so /emitter's emission did NOT
    // no_route. (/sink itself then no_routes its own /void emission — a separate
    // sender — which is expected and not what this pin asserts.)
    assert!(
        !dls.iter()
            .any(|dl| dl.reason.as_code() == "no_route" && dl.sender_path.as_str() == "/emitter"),
        "catch-all edge delivers /emitter's emission — no no_route from /emitter; got: {:?}",
        dls.iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    // /sink received it (it forwarded to /void), proving the catch-all delivery.
    assert!(
        dls.iter().any(|dl| dl.message.target.as_str() == "/void"),
        "the catch-all reached /sink (it forwarded to /void)"
    );

    colony.shutdown().await;
}
