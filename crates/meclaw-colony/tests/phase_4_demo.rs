//! Phase-4 demo test 1: filesystem bootstrap from an example tree built in a
//! TempDir (no loading from examples/).
//!
//! Tree:
//!   /                   (hive, root)
//!   ├── echo-a          (cell)
//!   ├── echo-b          (cell)
//!   └── pool/           (sub-hive)
//!       ├── w1          (cell)
//!       └── w2          (cell)
//!
//! Erwartete Counts: 2 hives, 4 cells, 2 edges.

use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::HashMap;
use std::sync::Arc;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_4_boot_from_examples_tree_populates_registry_and_hive_scopes() {
    let td = TempDir::new().unwrap();
    // Root hive with two edges
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":"./echo-a","to":"./echo-b"}
          ]}}}"#,
    );
    // Two top-level cells
    write(
        td.path(),
        "main/echo-a/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/echo-b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/echo-b/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/echo-a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Sub-hive with two cells and one edge
    write(
        td.path(),
        "main/pool/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":"./w1","to":"./w2"}
          ]}}}"#,
    );
    write(
        td.path(),
        "main/pool/w1/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/pool/w2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/pool/w2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/pool/w1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    let report = bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect("clean tree must bootstrap");

    assert_eq!(report.hive_count, 2, "2 hives: root + pool");
    assert_eq!(report.cell_count, 4, "4 echo cells: a, b, w1, w2");
    assert_eq!(report.edge_count, 2, "2 edges: root + pool");

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_4_edge_overrides_cell_emitted_target() {
    use meclaw_core::Path;
    use meclaw_core::serde_json::json;
    use meclaw_testing::MessageBuilder as TestMessageBuilder;
    use std::time::Duration;

    let td = TempDir::new().unwrap();
    // Root hive declares edge /echo-a -> /echo-b.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":"/echo-a","to":"/echo-b"}
          ]}}}"#,
    );
    // echo-a's config says emitted_target=/dummy — edge should override this.
    write(
        td.path(),
        "main/echo-a/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dummy"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // echo-b is terminal (emitted_target=/sink which doesn't exist → DLQ proves edge reached it).
    write(
        td.path(),
        "main/echo-b/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect("bootstrap must succeed");

    // Source-Message an /echo-a triggers the chain.
    let src = TestMessageBuilder::new("/echo-a")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"hi"})])
        .build();
    colony.send_from(Path::new("/"), src).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Probe: drain DLQ. /echo-b forwarded its emission to /sink (unresolved),
    // so DLQ must contain at least one entry with target=/sink — that proves
    // /echo-b received and processed the message via the edge override.
    let dls = colony.drain_dead_letters().await;
    let to_sink: Vec<_> = dls
        .iter()
        .filter(|dl| dl.message.target.as_str() == "/sink")
        .collect();
    assert!(
        !to_sink.is_empty(),
        "/echo-b forwarded to /sink (proves edge reached /echo-b, not /dummy)"
    );

    // Counter-proof: /dummy must NOT have received anything.
    // We can't directly inspect a non-existent cell, but if /dummy had been
    // the route, /echo-b's tag-emitted /sink would not appear in DLQ.
    // The to_sink assertion above is sufficient.

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_4_strict_fail_collects_multiple_errors_atomically() {
    use meclaw_colony::BootstrapError;
    use meclaw_core::Path;
    use meclaw_testing::MessageBuilder as TestMessageBuilder;
    use std::time::Duration;

    let td = TempDir::new().unwrap();
    // Root hive declares an edge with a malformed CEL `condition`.
    // Phase 13.5-A1: strict-fail-on-presence is gone; only parse-failure
    // surfaces as a bootstrap error (`EdgeConditionParse`).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":"./a","to":"./b","condition":"=="}
          ]}}}"#,
    );
    // /a has invalid JSON.
    write(td.path(), "main/a/config.json", "not valid json");
    // /b has unknown cell_type.
    write(
        td.path(),
        "main/b/config.json",
        r#"{"cell":{"type":"unknown_type"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    let err = bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect_err("must collect errors atomically, not Ok");

    let items = err.items();
    assert!(
        items
            .iter()
            .any(|e| matches!(e, BootstrapError::EdgeConditionParse { .. })),
        "edge condition parse error must be collected, got: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|e| matches!(e, BootstrapError::InvalidJson { .. })),
        "invalid JSON must be collected, got: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|e| matches!(e, BootstrapError::UnknownCellType { .. })),
        "unknown cell type must be collected, got: {items:?}"
    );

    // Atomic-strict proof: no cell was spawned.
    // Probe via: send to /a (which would exist if spawning had succeeded),
    // expect DLQ (UnresolvedPath) because nothing is registered.
    let msg = TestMessageBuilder::new("/a").build();
    colony.send_from(Path::new("/"), msg).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let dls = colony.drain_dead_letters().await;
    assert!(
        dls.iter().any(|dl| dl.message.target.as_str() == "/a"),
        "no cell spawned at /a — message must DLQ as unresolved"
    );

    colony.shutdown().await;
}
