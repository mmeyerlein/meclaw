//! Phase-16 Core-Takt (B-LOUD `colony.rs:1136`): the reboot **hive_scope**
//! hydration must HARD-FAIL on a corrupt `hive_scopes` table — symmetric to the
//! edge-hydration hard-fail 16 lines above (`read_edges` → panic on corruption,
//! `phase_13_5_durable_edges_demo.rs` steps 3+4).
//!
//! Before this slice `read_hive_scopes()` was read through an `if let Ok(...)`
//! arm: an `Err` was silently swallowed and the colony booted with an empty /
//! partial hive-scope table — exactly the silent routing corruption the edge
//! path asymmetrically prevents. This demo corrupts the persisted `hive_scopes`
//! table so the read returns `Err`, reboots via a raw `colony_task` JoinHandle,
//! and asserts the boot panics (surfacing as a `JoinError`).
//!
//! Topology mirrors the durable-edges demo: a root hive persists the hive_scope
//! `/`, `/a` is a self-echoing cell, and an `add_edges` mutation populates the
//! `edges` table so a populated colony.db classifies as `Reboot` (boot_state
//! needs all three of registry/edges/hive_scopes non-empty — corrupting the
//! *value* of the single hive_scope row, not its count, keeps the Reboot
//! classification while making the hydration read fail).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path, Uuid};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, spawn_colony_task_at};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive (persists hive_scope `/`) + `/a` self-echo source cell.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Boot 1: bootstrap (persists registry `/a` + hive_scope `/`) + add a CEL edge
/// via mutation (populates `edges`) + graceful shutdown. Leaves a populated
/// colony.db classifying as `Reboot`.
async fn boot1_populated_db(td: &TempDir) {
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "a", "to": "sink", "condition": "context.kind == 'text'"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "setup add_edges must commit, got {outcome:?}"
    );
    h.shutdown().await;
}

/// Reboot the colony_task and assert it panics with a message containing
/// `needle`. A *bounded* wait discriminates the two states: a hard-fail panics
/// near-instantly (JoinError), a silent swallow boots into the `select!` loop
/// and never returns — the timeout is that negative signal.
async fn assert_reboot_panics_with(td: &TempDir, needle: &str) {
    let join = spawn_colony_task_at(td.path(), echo_factories());
    let joined = tokio::time::timeout(Duration::from_secs(15), join).await;
    let err = match joined {
        Ok(res) => res.expect_err("corrupt hive_scopes must hard-fail the boot"),
        Err(_) => panic!(
            "boot did NOT terminate within 15s — read_hive_scopes Err was swallowed \
             (silent partial-boot instead of hard-fail)"
        ),
    };
    assert!(err.is_panic(), "boot failure must be a panic, got {err:?}");
    let msg = err.into_panic();
    let s = msg
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| msg.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(s.contains(needle), "panic msg was: {s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_16_reboot_hard_fails_on_corrupt_hive_scopes() {
    let td = TempDir::new().unwrap();
    boot1_populated_db(&td).await;

    // Corrupt the hive_scopes table so the hydration SELECT fails, while the
    // row count stays 1 (boot_state COUNT(*) still sees it → Reboot, not
    // Inconsistent). Renaming the `path` column makes `SELECT path FROM
    // hive_scopes` fail with "no such column" — a deterministic rusqlite Err.
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    conn.execute(
        "ALTER TABLE hive_scopes RENAME COLUMN path TO path_corrupt",
        [],
    )
    .unwrap();
    drop(conn);

    assert_reboot_panics_with(&td, "hive_scope hydration failed").await;
}
