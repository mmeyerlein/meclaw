//! Paket-3 P3-B-restart (sanctioned `handle_cell_died` corridor break,
//! the spec owner 2026-06-07): the `cell.message_timeout` B-backstop now triggers a
//! one_for_one **RESTART** (via `DeathKind::Backstop`), not removal.
//!
//! Full-topology proof through the REAL corridor: a `hang` cell (woken by a
//! probe) hangs in `handle()` past its small `cell.message_timeout`; the
//! substrate backstop fires the `backstop` oneshot and ends the task; the
//! watcher classifies `DeathKind::Backstop`; `handle_cell_died` restarts the
//! cell with a fresh mpsc pair. Positive restart signal: the
//! `HangCellFactory.spawn_count` increments (Wake = 1 → backstop-restart = 2).
//!
//! Stop-wiring restored (renotify): after the backstop-restart re-notifies the
//! colony with the fresh `(stop_tx, death_ack_rx)` pair, a SUBSEQUENT disconnect
//! of the cell COMMITS (it can be peace-stopped) instead of rejecting with
//! `stop_wiring_unavailable`.

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::Value;
use meclaw_core::{MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::{EchoCellFactory, HangCellFactory};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
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

/// Poll `spawn_count` until it reaches `target` (or time out). Returns the last
/// observed value.
async fn wait_for_spawn_count(counter: &Arc<AtomicU32>, target: u32) -> u32 {
    for _ in 0..300 {
        let n = counter.load(Ordering::Relaxed);
        if n >= target {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    counter.load(Ordering::Relaxed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backstop_triggers_restart_and_restores_stop_wiring() {
    let td = tempfile::TempDir::new().unwrap();

    // Root hive (the `main/` dir maps to `/`); cells `/anchor` and `/hang` are
    // its direct children (parent = root → always active once connected).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Anchor echo cell — provides the inbound edge that activates the hang cell.
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // The hang cell: hangs forever in handle(), small message_timeout so the
    // B-backstop fires quickly.
    write(
        td.path(),
        "main/hang/config.json",
        r#"{"cell":{"type":"hang","message_timeout":300},"params":{"hang_forever":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawn_count = Arc::new(AtomicU32::new(0));
    let hang_factory: Arc<dyn CellFactory> = Arc::new(HangCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let echo_factory: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("hang".to_string(), hang_factory.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    let mut registry = CellFactoryRegistry::new();
    registry.insert("hang".to_string(), hang_factory);
    registry.insert("echo".to_string(), echo_factory);

    let report = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    assert_eq!(report.cell_count, 2, "anchor + hang boot");

    // Activate `/hang` via an edge anchor->hang (edge-derived activity); the
    // bootstrap edge set was empty so the cell boots inactive.
    let act = send_mutation(
        &h,
        json!({
            "diff": { "add_edges": [{"from": "anchor", "to": "hang"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->hang must commit, got {act:?}"
    );

    // Probe the hang cell → wakes it (Dormant → Awake) with the message_timeout
    // wired. spawn_count: 0 → 1 (initial Wake build).
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;
    let after_wake = wait_for_spawn_count(&spawn_count, 1).await;
    assert!(after_wake >= 1, "hang cell must wake (spawn_count >= 1)");

    // handle() hangs forever → after ~300ms the B-backstop fires → the watcher
    // sees DeathKind::Backstop → handle_cell_died RESTARTS (NOT removes) → the
    // RespawnFn rebuilds → spawn_count bumps to >= 2. This is the positive
    // restart receipt that the sanctioned corridor change produces.
    let after_restart = wait_for_spawn_count(&spawn_count, 2).await;
    assert!(
        after_restart >= 2,
        "B-backstop must RESTART the hang cell (spawn_count >= 2, was {after_restart}) — \
         the P3-B-restart corridor change; before it the cell would have been REMOVED"
    );

    // Stop-wiring restored: a disconnect of the (restarted) hang cell must COMMIT
    // — the backstop-restart re-notified the colony with a fresh live stop pair,
    // so the cell can be peace-stopped (no `stop_wiring_unavailable` reject).
    let outcome = send_mutation(
        &h,
        json!({
            "diff": { "remove_nodes": [{"match": {"name": "hang"}}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect of the backstop-restarted cell must COMMIT (stop-wiring restored), got {outcome:?}"
    );

    h.shutdown().await;
}
