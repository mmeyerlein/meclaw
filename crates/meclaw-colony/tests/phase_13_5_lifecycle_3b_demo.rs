//! Phase-13.5 Lifecycle-3b Demo (Task 1.5): rehydration of the persisted
//! `registry.status` into `RegistryEntry.active`.
//!
//! The 3a identity overlay already delivers the persisted `status` string per
//! path. Task 1 maps it: `status == "active"` → `active == true`, anything else
//! (e.g. `"inactive"`) → `active == false`. A genuinely-new FS node (no overlay
//! entry) is `active == true`.
//!
//! Proof discipline: `active` is read from the **RAM** registry via
//! `ColonyMsg::ReadRegistry` after Boot 2 — the in-memory snapshot, not the DB.
//!
//! Topology mirrors the 3a demo for reboot mechanics: a root hive with one edge
//! (so the colony.db classifies as a Reboot) plus two echo cells `/a` and `/b`.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

/// `(name, factory)` list with `echo` registered.
fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive with one self-edge so edges + hive_scopes tables are populated;
/// `/a` and `/b` are echo cells so the registry table is too.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::create_dir_all(td.join("main/b")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"a","to":"a"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Read the in-memory (RAM) `active` flag for `path` via `ColonyMsg::ReadRegistry`.
async fn ram_active(h: &ColonyHandle, path: &str) -> bool {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("ReadRegistry ack timed out")
        .unwrap();
    reply
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path} must be registered"))
        .active
}

/// A boot where `/a` is persisted as `status='inactive'` must rehydrate `/a`
/// with `active == false`, while a still-`'active'` path `/b` rehydrates with
/// `active == true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reboot_maps_persisted_status_into_active() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1: registers /a and /b (status='active' in colony.db). ─────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    // Both fresh spawns are active.
    assert!(ram_active(&h, "/a").await, "boot1 /a must be active");
    assert!(ram_active(&h, "/b").await, "boot1 /b must be active");
    h.shutdown().await;

    // ── Flip /a to 'inactive' directly in colony.db between the two boots. ───
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        let n = conn
            .execute("UPDATE registry SET status='inactive' WHERE path='/a'", [])
            .unwrap();
        assert_eq!(n, 1, "exactly one /a row must be flipped to inactive");
    }

    // ── Boot 2 (same colony.db): /a inactive, /b active. ────────────────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");

    assert!(
        !ram_active(&h2, "/a").await,
        "/a persisted as status='inactive' must rehydrate with active == false"
    );
    assert!(
        ram_active(&h2, "/b").await,
        "/b still status='active' must rehydrate with active == true"
    );

    h2.shutdown().await;

    // ── DB round-trip: persisted status survived Boot 2's re-registration. ───
    // Boot 2 re-ran UpsertRegistry for /a and /b. If UpsertRegistry still wrote
    // `status` on conflict, /a's 'inactive' would have been clobbered back to
    // 'active'. SetRegistryStatus is the sole status-authority — so the values
    // must be exactly what we last wrote: /a inactive, /b active.
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let status_a: String = conn
        .query_row("SELECT status FROM registry WHERE path='/a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let status_b: String = conn
        .query_row("SELECT status FROM registry WHERE path='/b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        status_a, "inactive",
        "/a status must survive re-registration (UpsertRegistry must not clobber it)"
    );
    assert_eq!(status_b, "active", "/b status must stay active");
}

/// A genuinely-new FS node added before a REBOOT is REPORTED, not adopted (A5b,
/// Phase-16 W1b — announced regression: pre-A5b this pinned the new node
/// rehydrating `active == true`; the ratified ruling reports unknown reboot
/// nodes and never registers them, so it is absent from the registry).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_fs_node_without_overlay_entry_is_reported_not_adopted_on_reboot() {
    let td = TempDir::new().unwrap();
    // Only /a exists on the first boot.
    std::fs::create_dir_all(td.path().join("main/a")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"a","to":"a"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    h.shutdown().await;

    // Add a genuinely-new /c after Boot 1 — no overlay entry exists for it.
    std::fs::create_dir_all(td.path().join("main/c")).unwrap();
    std::fs::write(
        td.path().join("main/c/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");

    // A5b: /c is a new dir found at REBOOT → reported, not adopted → not in the
    // registry at all (so neither active nor inactive — simply unregistered).
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h2.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    assert!(
        !reply.entries.iter().any(|e| e.path == "/c"),
        "A5b: a new FS node found at reboot must be reported, not registered"
    );
    h2.shutdown().await;
}
