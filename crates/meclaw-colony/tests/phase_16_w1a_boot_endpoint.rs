//! Phase-16 W1a A8 (Ruling 2026-06-12): the boot endpoint-existence check.
//!
//! The flipped `bootstrap_commits_with_dangling_endpoint_no_reject` (case e):
//! the APPLY path now resolves `params.graph` edge endpoints against the LIVE
//! colony — plan cells/hives ∪ already-live registry paths ∪ `/colony/*`.
//!
//! - A registry-only endpoint (a sink spawned via `h.spawn` BEFORE bootstrap)
//!   is resolved → the boot COMMITS (pins the runtime-sink pattern that 5
//!   examples + the 14b harness rely on).
//! - A genuinely-unregistered endpoint (a typo) resolves to nothing → the boot
//!   FAILS LOUD with `BootstrapError::DanglingEndpoint` naming the edge + path.

use meclaw_colony::{BootstrapError, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::Path;
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tempfile::TempDir;

/// Write a root hive whose `params.graph` declares a single edge `. → <to>`.
fn write_root_edge_to(td: &std::path::Path, to: &str) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":[{{"from":".","to":"{to}"}}]}}}}}}"#
        ),
    )
    .unwrap();
}

/// Case e (commit): the edge targets `/sink`, a registry-only cell spawned
/// before bootstrap. The endpoint resolves against the live registry → boot
/// COMMITS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_commits_with_registry_only_endpoint() {
    let td = TempDir::new().unwrap();
    write_root_edge_to(td.path(), "/sink");

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    // Registry-only sink, spawned BEFORE bootstrap (the h.spawn pattern).
    h.spawn(Path::new("/sink"), || EchoMockCell::new(Path::new("/sink")))
        .await;

    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("boot must commit: /sink resolves against the live registry");
    h.shutdown().await;
}

/// Case e (fail): the edge targets `/bogus`, which is neither in the FS plan
/// nor the live registry nor a `/colony/*` endpoint → LOUD boot fail naming the
/// dangling endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_fails_loud_on_unregistered_endpoint() {
    let td = TempDir::new().unwrap();
    write_root_edge_to(td.path(), "/bogus");

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    let err = bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect_err("boot must fail: /bogus resolves to nothing the colony knows");
    assert!(
        err.items().iter().any(|e| matches!(
            e,
            BootstrapError::DanglingEndpoint { endpoint, .. } if endpoint.as_str() == "/bogus"
        )),
        "expected DanglingEndpoint for /bogus, got {err:?}"
    );
    h.shutdown().await;
}
