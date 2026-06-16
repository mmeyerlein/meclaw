//! Phase-13.5 Lifecycle-3a Demo (Task 2.1): cell_id stability across a reboot
//! via the colony.db **identity overlay**.
//!
//! The bug (G5): `plan_bootstrap` assigns `Uuid::now_v7()` to every cell on
//! EVERY boot. `colony.db` persists the original cell_id (the registry UPSERT
//! does NOT overwrite cell_id on `ON CONFLICT(path)`), so the DB is stable —
//! but the in-memory (RAM) registry drifts: a known path gets a fresh RAM
//! cell_id after a reboot.
//!
//! The fix: on reboot, `bootstrap_from_filesystem` consults the persisted
//! registry as an identity overlay (path → cell_id + status). A known path
//! reuses the persisted cell_id instead of minting a fresh one.
//!
//! Proof discipline: we read the **RAM** cell_id via `ColonyMsg::ReadRegistry`
//! (the in-memory registry snapshot), not the DB — reading the DB would always
//! show a stable cell_id and would NOT prove the RAM-drift is fixed.
//!
//! Topology (mirrors phase_13_5_durable_edges_demo for reboot mechanics):
//!   td/main/config.json   — root hive with one edge (so the edges +
//!                           hive_scopes tables are non-empty → boot_state
//!                           classifies a populated colony.db as Reboot).
//!   td/main/a/config.json — `/a`: echo cell (lands in the registry table on
//!                           Register, giving boot_state a non-empty registry).

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

/// `(name, factory)` list with `echo` registered — a rebooted colony opens the
/// same `colony.db` with the same factory registry as the first boot.
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

/// Root hive declares one self-edge `/a -> /a` so the edges + hive_scopes
/// tables are populated; `/a` is an echo cell so the registry table is too.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
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
}

/// Read the in-memory (RAM) cell_id for `path` via `ColonyMsg::ReadRegistry`.
async fn ram_cell_id(h: &ColonyHandle, path: &str) -> String {
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
    let reply = ack_rx.await.unwrap();
    reply
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path} must be registered"))
        .cell_id
}

/// True iff `path` is present in the RAM registry (via `ColonyMsg::ReadRegistry`).
/// Used by the A5b reboot tests to assert a new dir is reported, NOT adopted.
async fn ram_has_path(h: &ColonyHandle, path: &str) -> bool {
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
    let reply = ack_rx.await.unwrap();
    reply.entries.iter().any(|e| e.path == path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_lifecycle_3a_cell_id_survives_reboot() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1 ──────────────────────────────────────────────────────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    let cell_id_x = ram_cell_id(&h, "/a").await;
    h.shutdown().await;

    // ── Boot 2 (reboot on the SAME colony.db) ───────────────────────────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");
    let cell_id_after = ram_cell_id(&h2, "/a").await;
    h2.shutdown().await;

    assert_eq!(
        cell_id_after, cell_id_x,
        "RAM cell_id for /a must be stable across reboot (identity overlay); \
         was {cell_id_x}, became {cell_id_after}"
    );
}

/// Demo (d) — Rehydration-only-new (Auflage A-rehydration-d):
/// A reboot on a mixed tree (known FS paths + a genuinely new FS directory
/// added between the two boots) must:
///   - keep the cell_id of already-known paths **unchanged** (identity overlay),
///   - **report, NOT adopt** the new path (A5b): registration is
///     instantiation/mutation-only, so a manually-added dir found at reboot is
///     never registered — it does not appear in the RAM registry.
///
/// **Regression mit Ansage (Phase-16 W1b, Ruling A5b 2026-06-12):** pre-A5b
/// this test pinned the old auto-adoption ("a new dir on reboot gets a fresh
/// cell_id"). The ratified A5b ruling reverses that — the reboot walk reports
/// unknown nodes (WARN / `--validate` listing) and never adopts them. Only the
/// known-path identity-overlay reuse is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reboot_mixed_tree_known_keeps_id_new_is_reported_not_adopted() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1 ──────────────────────────────────────────────────────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    // Note the RAM cell_id for the known path /a.
    let cell_id_known = ram_cell_id(&h, "/a").await;
    h.shutdown().await;

    // ── Add a genuinely-new FS cell directory /b between the two boots. ─────
    // It exists on FS but has NO colony.db registry row → at a REBOOT A5b
    // reports it as an unregistered node and does NOT adopt it.
    std::fs::create_dir_all(td.path().join("main/b")).unwrap();
    std::fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // ── Boot 2 (same colony.db, extended tree: /a known + /b new) ───────────
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed");
    let cell_id_known_after = ram_cell_id(&h2, "/a").await;
    let b_registered = ram_has_path(&h2, "/b").await;
    h2.shutdown().await;

    // Known path /a keeps its original cell_id (identity overlay reused).
    assert_eq!(
        cell_id_known_after, cell_id_known,
        "RAM cell_id for /a (known path) must survive the reboot unchanged; \
         was {cell_id_known}, became {cell_id_known_after}"
    );

    // A5b: the new path /b is REPORTED, never adopted — absent from the registry.
    assert!(
        !b_registered,
        "A5b: a new dir found at reboot must be reported, not registered (no adoption)"
    );
}

/// Orphan reconciliation (Auflage A4): an overlay entry whose FS directory is
/// gone must NOT crash the boot, must NOT re-mint a cell_id, and must NOT
/// resurrect the cell. The FS walk never reaches the orphan path, so it simply
/// stays in the DB (no-delete) and is not re-spawned. A genuinely-new FS path
/// added at the same reboot is REPORTED, not adopted (A5b, Phase-16 W1b —
/// Regression mit Ansage: pre-A5b this pinned the new path getting a fresh
/// cell_id; the ratified ruling reports unknown reboot nodes instead).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_lifecycle_3a_orphan_overlay_entry_does_not_crash_or_remint() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1: registers /a (persisted to colony.db registry) ──────────────
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot1 bootstrap must succeed");
    let _ = ram_cell_id(&h, "/a").await; // confirm /a was registered
    h.shutdown().await;

    // ── Make /a an orphan: drop its FS directory, keep the colony.db row. ────
    // Add a genuinely-new FS cell /b at the same reboot.
    std::fs::remove_dir_all(td.path().join("main/a")).unwrap();
    std::fs::create_dir_all(td.path().join("main/b")).unwrap();
    std::fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // ── Boot 2: orphan /a row present in overlay but no FS dir → no crash. ───
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 bootstrap must succeed despite the orphan /a overlay entry");

    // A5b: /b is a new dir found at reboot → REPORTED, not adopted → absent from
    // the RAM registry (no crash, overlay miss is reported not registered).
    assert!(
        !ram_has_path(&h2, "/b").await,
        "A5b: a new dir found at reboot must be reported, not registered"
    );

    // /a is an orphan: not in the live FS, so not re-spawned → absent from RAM.
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
        !reply.entries.iter().any(|e| e.path == "/a"),
        "orphan /a (no FS dir) must NOT be resurrected into the RAM registry"
    );

    h2.shutdown().await;
}
