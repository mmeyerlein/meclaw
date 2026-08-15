//! AUDIT-PRE14-001 regression: a Long-Running cell whose `handle()` panics MUST
//! surface that panic on the outer task's `JoinHandle` (and thus to the
//! supervisor as `was_panic = true` → restart, NOT graceful removal).
//!
//! Root cause (pre-fix): the outer `tokio::select!` in `cell_task_long_running`
//! was unbiased and each arm aborted+discarded the peer `JoinHandle`. A handler
//! panic also closes `run_io` (the unwinding handler drops `reconfig_tx`), so
//! `io_join` becomes ready `Ok(())` concurrently; when the `io_join` arm won, the
//! handler panic was swallowed and the outer task returned `Ok(())`.
//!
//! These tests are AMPLIFIED (many concurrent runs) so the load-induced
//! interleaving is hit reliably. No sleeps; a single 30s timeout backstops a hang
//! (failure-marker convention). Pre-fix they fire (some runs lose the panic);
//! post-fix they are 0/N deterministically.

use meclaw_colony::{ColonyMsg, DbConn, cell_task_long_running, spawn_watcher};
use meclaw_core::{CellEmission, MessageBuilder, Path};
use meclaw_testing::mocks::ReceiptMockLongRunningCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// One scenario run: spawn a long-running cell whose handler panics on its first
/// message; send one message; return `true` iff the outer `JoinHandle` returned
/// `Ok(())` (i.e. the panic was LOST).
async fn outer_join_lost_panic() -> bool {
    let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
    let cell = cell.with_panic_in_handle_after(1);
    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);
    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None, // death_ack
        None, // blob_store
        None,
    ));
    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();
    // Hold the io-event sender so `run_io` can only end via the handler unwind
    // dropping `reconfig_tx` (the exact race), not via its own input closing.
    let r = tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("outer join did not settle within 30s (hang backstop)");
    drop(inject_tx);
    match r {
        Ok(()) => true, // panic swallowed — the bug
        Err(e) => {
            assert!(e.is_panic(), "unexpected non-panic JoinError: {e:?}");
            false
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handler_panic_always_propagates_to_outer_join() {
    const BATCHES: usize = 50;
    const CONCURRENCY: usize = 64;
    let lost = Arc::new(AtomicUsize::new(0));
    for _ in 0..BATCHES {
        let mut handles = Vec::with_capacity(CONCURRENCY);
        for _ in 0..CONCURRENCY {
            let lost = lost.clone();
            handles.push(tokio::spawn(async move {
                if outer_join_lost_panic().await {
                    lost.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    }
    let n = lost.load(Ordering::SeqCst);
    assert_eq!(
        n,
        0,
        "{n}/{} runs LOST the handler panic (outer JoinHandle returned Ok(()) instead of \
         Err(is_panic)) — AUDIT-PRE14-001 regression",
        BATCHES * CONCURRENCY
    );
}

/// One end-to-end run through `spawn_watcher`: a real panicking long-running cell
/// wired exactly like production (peace_tx held by the cell, peace_rx by the
/// watcher). On a handler panic the watcher MUST emit
/// `CellDied { death_kind: Panic }` (which `handle_cell_died` routes to the
/// restart path) — NOT `Normal`/`Backstop` misclassification. AUDIT-PRE14-001
/// amplified (P3-B-restart): even though `backstop_rx` is now plumbed, a panic
/// must classify `Panic` (panic priority). Returns `true` iff classification was
/// WRONG (not `Panic`).
async fn watcher_misclassified_panic() -> bool {
    let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
    let cell = cell.with_panic_in_handle_after(1);
    let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);
    // Production wiring: cell owns peace_tx, watcher owns peace_rx.
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(cell_task_long_running(
        Path::new("/lr"),
        in_rx,
        out_tx,
        16,
        cell,
        db,
        Some(peace_tx),
        None,
        None,
        None,
        None,
        None,
    ));
    let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
    spawn_watcher(&inbox_tx, Path::new("/lr"), join, peace_rx, backstop_rx);
    in_tx
        .send(MessageBuilder::new(Path::new("/lr")).build())
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(30), inbox_rx.recv())
        .await
        .expect("watcher did not emit within 30s (hang backstop)")
        .expect("watcher inbox closed without a message");
    drop(inject_tx);
    match msg {
        // true == misclassified (anything other than Panic is WRONG for a panic).
        ColonyMsg::CellDied { death_kind, .. } => death_kind != meclaw_colony::DeathKind::Panic,
        _ => panic!("expected ColonyMsg::CellDied from the watcher"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watcher_classifies_handler_panic_as_was_panic_true() {
    const BATCHES: usize = 25;
    const CONCURRENCY: usize = 40;
    let wrong = Arc::new(AtomicUsize::new(0));
    for _ in 0..BATCHES {
        let mut handles = Vec::with_capacity(CONCURRENCY);
        for _ in 0..CONCURRENCY {
            let wrong = wrong.clone();
            handles.push(tokio::spawn(async move {
                if watcher_misclassified_panic().await {
                    wrong.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    }
    let n = wrong.load(Ordering::SeqCst);
    assert_eq!(
        n,
        0,
        "{n}/{} runs misclassified a handler panic as was_panic=false (cell would be REMOVED, \
         not restarted) — AUDIT-PRE14-001 watcher-contract regression",
        BATCHES * CONCURRENCY
    );
}
