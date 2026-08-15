//! Phase-10-B Demo. Zwei Tests:
//! 1. Fire: anti-cascade (/sink first), op:add cron */1, receipts with
//!    vollstaendigem Header-Set + monotonen iteration_n.
//! 2. Substrate shutdown with a one-shot at 2099 — the mailbox-close abort path.

use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::{ColonyHandle, topologies::phase_3a::CaptureCell};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10b_demo_fire_via_per_second_cron() {
    let h = ColonyHandle::new();
    let (recv_tx, mut recv_rx) = mpsc::channel::<Message>(32);

    // Anti-cascade: register /sink FIRST (phase-6.5 lesson).
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    // Register the timer via the factory (production path).
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(TimerCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/timer"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/timer"), spawned).await;
    // W2 (A1): /timer emission to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/timer"),
        Path::new("/sink"),
    )
    .await;

    // op:add — */1 * * * * * → fire every second.
    let sid = Uuid::now_v7();
    h.send(
        MessageBuilder::new(Path::new("/timer"))
            .body(Body::Inline(json!({
                "op": "add", "schedule_id": sid.to_string(),
                "schedule_name": "tick", "cron": "*/1 * * * * *",
                "emit_to": "/sink", "emit_body": { "messages": [] }, "emit_headers": {}
            })))
            .build(),
    )
    .await;

    // Collect for ~3.5s. Outer deadline + inner recv timeout (200ms) — no
    // blocking rx, which would risk a hang.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(3500);
    let mut hits: Vec<Message> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), recv_rx.recv()).await {
            Ok(Some(m)) => hits.push(m),
            _ => continue,
        }
    }

    // Lower bound: ≥2 receipts. NO upper bound (flake robustness).
    assert!(
        hits.len() >= 2,
        "expected >=2 fires within ~3.5s, got {}",
        hits.len()
    );

    // Header completeness + values per receipt.
    let mut iters: Vec<i64> = Vec::new();
    for (i, m) in hits.iter().enumerate() {
        let h = &m.headers.hop;
        let event_id = h
            .get("event_id")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("hit {i}: missing event_id"));
        assert!(Uuid::parse_str(event_id).is_ok(), "event_id is UUID");
        assert_eq!(
            h.get("schedule_id").and_then(|v| v.as_str()).unwrap(),
            sid.to_string()
        );
        assert_eq!(
            h.get("schedule_name").and_then(|v| v.as_str()).unwrap(),
            "tick"
        );
        let scheduled_at = h
            .get("scheduled_at")
            .and_then(|v| v.as_str())
            .expect("scheduled_at");
        let fired_at = h
            .get("fired_at")
            .and_then(|v| v.as_str())
            .expect("fired_at");
        assert!(scheduled_at.ends_with('Z'), "scheduled_at RFC-3339-Z");
        assert!(fired_at.ends_with('Z'), "fired_at RFC-3339-Z");
        // Both are SecondsFormat::Secs — fired_at may be strictly greater or
        // equal (really milliseconds later, but identical once rounded to the
        // second). The spec demands ordering, not strict
        // Differenz auf Sekunden-Aufloesung.
        assert!(
            scheduled_at <= fired_at,
            "scheduled_at <= fired_at: sched={scheduled_at} fired={fired_at}"
        );
        let iter = h
            .get("iteration_n")
            .and_then(|v| v.as_i64())
            .expect("iteration_n");
        iters.push(iter);
    }
    // Monotonic from 0 (a lower bound — no fixed sequence, flake-robust).
    assert_eq!(iters[0], 0, "first iteration_n is 0");
    for w in iters.windows(2) {
        assert!(w[1] > w[0], "iteration_n strictly increasing: {:?}", iters);
    }

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10b_demo_substrate_shutdown_with_once_in_2099() {
    use chrono::{TimeZone, Utc};
    use meclaw_cells::timer::cell::TimerCell;
    use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind};
    use meclaw_colony::{DbConn, cell_task_long_running};
    use meclaw_core::CellEmission;

    // A real TimerCell with a one-shot schedule far in the future → I/O
    // Sub-Task haengt real in `sleep_until` (Phase-10-A-Lesson, commit
    // 31c15b6: not a hollow `pending().await`).
    let active = vec![ActiveSchedule {
        schedule_id: Uuid::now_v7(),
        kind: ScheduleKind::At(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()),
    }];
    let cell = TimerCell::new(Path::new("/timer"), active, 5000);

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/timer"),
        in_rx,
        out_tx,
        64,
        cell,
        db,
        None, // peace_tx
        None, // colony_inbox_tx
        None, // stop_rx
        None, // death_ack
        None, // blob_store
        None,
    ));

    // Sanity: wait 200 ms — `run_io` really sits in `sleep_until` (target:
    // 2099). If cell_task_long_running had already terminated here, it did not
    // run through the real sleep_until path.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !join.is_finished(),
        "cell_task is running (sleep_until in 2099)"
    );

    // Mailbox-close abort path (phase-10-A lesson, the second-order trap):
    // drop(in_tx) → handler_loop's mailbox.recv() → None → break out of the
    // handler_loop → handler_join completes → outer select! in
    // cell_task_long_running aborts io_join → return. NOT an events-channel
    // close (that would be the wrong path: events_tx is run_io-internal and only
    // closes when the sender drops, which only happens on run_io's return — a
    // chicken-and-egg with the sleep_until
    // in 2099).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("substrate shutdown was not prompt")
        .unwrap();
}
