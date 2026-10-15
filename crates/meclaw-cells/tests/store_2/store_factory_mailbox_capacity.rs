//! T9 (Pilot) — store factory honours `mailbox_capacity`.
//!
//! Proves the end-to-end wiring: `spawn_cell(... mailbox_capacity: N ...)` must
//! produce a `Dormant` variant whose `sender.max_capacity()` equals N.
//!
//! Two cases:
//!   1. override (N=7)  — the mandatory assertion.
//!   2. default  (N=1000) — regression guard: existing callers are unaffected.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::Path;
use std::sync::Arc;

fn make_raw_params() -> meclaw_core::JsonValue {
    meclaw_core::serde_json::json!({
        "schema": { "items": { "id": "int", "name": "text" } }
    })
}

/// Helper: spawn a `store` factory and return the `Dormant` sender without
/// driving any I/O.  The cell dir is a fresh temp dir so the factory finds no
/// existing `cell.db`.
fn spawn_store(cell_dir: std::path::PathBuf, mailbox_capacity: usize) -> SpawnedCellKind {
    let (otx, _orx) = tokio::sync::mpsc::channel(8);
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    Arc::new(StoreCellFactory)
        .spawn_cell(
            Path::new("/store"),
            make_raw_params(),
            otx,
            cell_dir,
            ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            mailbox_capacity,
        )
        .expect("spawn_cell must succeed with valid params")
}

/// Mandatory: mailbox capacity override is reflected in the Dormant sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_factory_mailbox_capacity_override() {
    let td = tempfile::TempDir::new().unwrap();
    let kind = spawn_store(td.path().to_path_buf(), 7);
    let sender = match kind {
        SpawnedCellKind::Dormant { sender, .. } => sender,
        SpawnedCellKind::Active { .. } => panic!("expected Dormant"),
    };
    assert_eq!(
        sender.max_capacity(),
        7,
        "store factory must use mailbox_capacity param (7), not the hardcoded 1000"
    );
}

/// Regression: default capacity (1000) must still work for existing callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_factory_mailbox_capacity_default() {
    let td = tempfile::TempDir::new().unwrap();
    let kind = spawn_store(td.path().to_path_buf(), 1000);
    let sender = match kind {
        SpawnedCellKind::Dormant { sender, .. } => sender,
        SpawnedCellKind::Active { .. } => panic!("expected Dormant"),
    };
    assert_eq!(
        sender.max_capacity(),
        1000,
        "default capacity (1000) must be preserved"
    );
}
