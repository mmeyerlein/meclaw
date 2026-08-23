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

/// GH #285 — a hive at `/h` declaring `params.ports` as given and wiring itself
/// to `./gen`, which has no directory. `main/` is the root scope `/`.
fn write_slot_hive(td: &std::path::Path, ports: &str) {
    std::fs::create_dir_all(td.join("main/h")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/config.json"),
        format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{"ports":{ports},
                 "graph":{{"edges":[{{"from":".","to":"./gen"}}]}}}}}}"#
        ),
    )
    .unwrap();
}

/// GH #285, the boot half of the exemption, END-TO-END through the real apply
/// path: a hive that DECLARED a slot invited this edge, so the boot commits
/// even though nothing stands at `/h/gen`.
///
/// The unit tests call the check with a slot set they derive themselves; this
/// one only plants a tree and boots, so it is what goes red if the boot call
/// site ever stops deriving one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_commits_on_a_declared_slot_endpoint() {
    let td = TempDir::new().unwrap();
    write_slot_hive(
        td.path(),
        r#"[{"name":"gen","slot":true,"unbound":"park"}]"#,
    );

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("boot must commit: /h/gen is a declared slot — an address that may stand empty");
    h.shutdown().await;
}

/// GH #285, the typo case at the boot: the SAME tree without the declaration
/// fails LOUD again. The exemption is bought by the declaration and by nothing
/// else — without this half, deleting the check entirely would stay green.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_fails_loud_without_the_slot_declaration() {
    let td = TempDir::new().unwrap();
    write_slot_hive(td.path(), "[]");

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);
    let err = bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect_err("boot must fail: nothing declared /h/gen");
    assert!(
        err.items().iter().any(|e| matches!(
            e,
            BootstrapError::DanglingEndpoint { endpoint, .. } if endpoint.as_str() == "/h/gen"
        )),
        "expected DanglingEndpoint for /h/gen, got {err:?}"
    );
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
