//! Phase-10 Slice A — RespawnFn-Round-Trip through the real
//! `handle_cell_died` corridor. Proves that the Factory-RespawnFn
//! (which re-invokes `cell_task_long_running`) restarts BOTH sub-tasks
//! together (one_for_one) and that the post-restart Cell-Instanz is
//! GESUND — post-restart messages reach the fresh handler, post-restart
//! events reach the fresh I/O sub-task. Harness mirrors Phase-5 Q8/Q9
//! (tests/phase/phase_5_quiescence_routing.rs T31) — same Colony+Factory+
//! wait_for_spawn_count pattern.

use meclaw_colony::factory::CellFactory;
use meclaw_core::{MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::LongRunningReceiptFactory;
use meclaw_testing::mocks::MockEvent;
use meclaw_testing::wait::wait_for_spawn_count;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10_a_round_trip_panic_restart_dispatches_to_fresh_handler_and_io() {
    let h = ColonyHandle::new();
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("lr");
    std::fs::create_dir(&cell_dir).unwrap();

    let factory = Arc::new(LongRunningReceiptFactory::new_with_panic_after_first_handle());
    let spawn_count = factory.spawn_count.clone();
    let handle_calls = factory.handle_calls.clone();
    let event_calls = factory.event_calls.clone();

    // Spawn via CellFactory trait + register via Phase-5-Harness.
    let spawned = factory
        .clone()
        .spawn_cell(
            Path::new("/lr"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/lr"), spawned).await;
    wait_for_spawn_count(&spawn_count, 1).await;

    // First message → first cell instance panics in handle() (factory arms
    // ONLY the first instance via prior==0 check in spawn_cell).
    h.send(MessageBuilder::new(Path::new("/lr")).build()).await;

    // Restart barrier: spawn_count == 2 (Supervisor called RespawnFn,
    // BOTH sub-tasks freshly spawned).
    wait_for_spawn_count(&spawn_count, 2).await;

    // Post-restart: second cell instance is HEALTHY (no arm), second
    // message reaches fresh handler.
    let pre_handle = handle_calls.load(Ordering::SeqCst);
    h.send(MessageBuilder::new(Path::new("/lr")).build()).await;
    meclaw_testing::wait::until_with(
        "the fresh handler picks up the post-restart message",
        || std::future::ready(handle_calls.load(Ordering::SeqCst) > pre_handle),
        || (pre_handle, handle_calls.load(Ordering::SeqCst)),
    )
    .await;

    // Fresh I/O sub-task: post-restart inject_tx (old is dead, factory
    // published the fresh one via latest_inject).
    let pre_event = event_calls.load(Ordering::SeqCst);
    let inject = factory
        .latest_inject_tx()
        .expect("post-restart inject_tx must exist");
    inject
        .send(MockEvent("post_restart".into()))
        .await
        .expect("inject");
    meclaw_testing::wait::until_with(
        "the fresh I/O sub-task forwards the post-restart event",
        || std::future::ready(event_calls.load(Ordering::SeqCst) > pre_event),
        || (pre_event, event_calls.load(Ordering::SeqCst)),
    )
    .await;

    // Final assertion: spawn_count EXACTLY 2 (no restart loop — arm fired only
    // on first instance, post-restart healthy). >2 = panic_in_handle_after
    // armed on rebuild = BUG.
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        2,
        "spawn_count MUST be exactly 2 (initial spawn + one restart); >2 indicates a restart loop"
    );

    h.shutdown().await;
}
