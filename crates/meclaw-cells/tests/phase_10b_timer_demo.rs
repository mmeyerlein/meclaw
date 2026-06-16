//! Phase-10-B Demo. Zwei Tests:
//! 1. Fire: Anti-Cascade (/sink zuerst), op:add cron */1, Receipts mit
//!    vollstaendigem Header-Set + monotonen iteration_n.
//! 2. Substrat-Shutdown mit once 2099 — Mailbox-Close-Abort-Pfad.

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

    // Anti-Cascade: /sink ZUERST registrieren (Phase-6.5-Lesson).
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    // Timer registrieren via Factory (Production-Pfad).
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

    // op:add — */1 * * * * * → jede Sekunde feuern.
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

    // ~3.5s sammeln. Outer-Deadline + Inner-Recv-Timeout (200ms) — kein
    // Block-rx, sonst Hang-Risiko.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(3500);
    let mut hits: Vec<Message> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), recv_rx.recv()).await {
            Ok(Some(m)) => hits.push(m),
            _ => continue,
        }
    }

    // Untere Schranke: ≥2 Receipts. KEIN Upper-Bound (flake-Robustheit).
    assert!(
        hits.len() >= 2,
        "expected >=2 fires within ~3.5s, got {}",
        hits.len()
    );

    // Header-Vollstaendigkeit + Werte pro Receipt.
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
        // Beide sind SecondsFormat::Secs — fired_at kann strikt > oder
        // gleich sein (real ms-spaeter, aber auf Sekunde gerundet
        // identisch). Spec verlangt Reihenfolge, nicht strikte
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
    // Monotonie ab 0 (untere Schranke — keine feste Sequenz, flake-robust).
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

    // Echte TimerCell mit einem once-Schedule weit in der Zukunft → I/O-
    // Sub-Task haengt real in `sleep_until` (Phase-10-A-Lesson, commit
    // 31c15b6: kein hohler `pending().await`).
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

    // Sanity: 200 ms warten — `run_io` steht REAL in `sleep_until`
    // (Ziel: 2099). Wenn cell_task_long_running hier schon terminiert
    // waere, lief es nicht ueber den echten sleep_until-Pfad.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !join.is_finished(),
        "cell_task laeuft (sleep_until in 2099)"
    );

    // Mailbox-Close-Abort-Pfad (Phase-10-A-Lesson, Second-Order-Trap):
    // drop(in_tx) → handler_loop's mailbox.recv() → None → break aus dem
    // handler_loop → handler_join completes → outer select! in
    // cell_task_long_running abortet io_join → return. KEIN
    // events-Channel-Close (das waere der falsche Pfad: events_tx ist
    // run_io-internal und schliesst erst beim Drop des Senders, der nur
    // beim run_io-Return passiert — also Henne-Ei mit dem sleep_until
    // in 2099).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("Substrat-Shutdown nicht prompt")
        .unwrap();
}
