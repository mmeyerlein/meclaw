//! T15/T17 — mailbox_capacity wiring for stateless-dispatcher and long-running
//! factory archetypes.
//!
//! Proves end-to-end: `spawn_cell(... mailbox_capacity: N ...)` produces a
//! `SpawnedCellKind` whose `sender.max_capacity()` equals N.
//!
//! Two archetypes covered (byte-identical wiring per archetype; no redundant
//! copies needed per spec):
//!   - **Stateless-Dispatcher** via `web_fetch` (T15 representative).
//!   - **Long-Running** via `timer` (T17).

// ── Stateless-Dispatcher: web_fetch ─────────────────────────────────────────

use meclaw_cells::web_fetch::WebFetchCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::{Path, serde_json::json};
use std::sync::Arc;

/// Helper: spawn a `web_fetch` factory with the given `mailbox_capacity` and
/// return the `Active` sender without driving any I/O.
fn spawn_web_fetch(mailbox_capacity: usize) -> SpawnedCellKind {
    let params = json!({
        "external_timeout_ms": 5000
    });
    let (otx, _orx) = tokio::sync::mpsc::channel(8);
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let td = tempfile::TempDir::new().unwrap();
    Arc::new(WebFetchCellFactory)
        .spawn_cell(
            Path::new("/wf"),
            params,
            otx,
            td.path().to_path_buf(),
            ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            mailbox_capacity,
        )
        .expect("web_fetch spawn_cell must succeed with valid params")
}

/// T15 — Stateless-Dispatcher archetype: mailbox capacity override is reflected
/// in the Active sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_fetch_factory_mailbox_capacity_override() {
    let kind = spawn_web_fetch(9);
    let sender = match kind {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => panic!("expected Active for stateless factory"),
    };
    assert_eq!(
        sender.max_capacity(),
        9,
        "web_fetch factory must use mailbox_capacity param (9), not hardcoded 1000"
    );
}

// ── Long-Running: timer ──────────────────────────────────────────────────────

use meclaw_cells::timer::TimerCellFactory;

/// Helper: spawn a `timer` factory with the given `mailbox_capacity` and
/// return the `Active` sender without driving any scheduling logic.
fn spawn_timer(cell_dir: std::path::PathBuf, mailbox_capacity: usize) -> SpawnedCellKind {
    // Minimal valid params: empty schedules list.
    let params = json!({ "schedules": [] });
    let (otx, _orx) = tokio::sync::mpsc::channel(8);
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/tmr"),
            params,
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
        .expect("timer spawn_cell must succeed with valid params")
}

/// T17 — Long-Running archetype: mailbox capacity override is reflected in the
/// Active sender.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_factory_mailbox_capacity_override() {
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();

    let kind = spawn_timer(cell_dir, 11);
    let sender = match kind {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => panic!("expected Active for long-running factory"),
    };
    assert_eq!(
        sender.max_capacity(),
        11,
        "timer factory must use mailbox_capacity param (11), not hardcoded 1000"
    );
}
