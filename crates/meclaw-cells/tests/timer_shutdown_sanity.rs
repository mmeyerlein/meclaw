//! Phase-10-B T10: Substrat-Shutdown-Sanity (Phase-10-A-Lesson Z.290–333).
//!
//! Endlos-I/O (real `sleep_until` auf once in 2099) muss bei Mailbox-Close
//! prompt terminieren. Beweis, dass der **Mailbox-Close-Abort-Pfad** durch
//! `cell_task_long_running` greift — NICHT der events-Channel-Close-Pfad:
//!
//! - I/O-Sub-Task steht in `sleep_until(2099)` — kein `events_tx.send` →
//!   events-Channel-Close-Pfad ist konstruktiv unerreichbar.
//! - Test sendet bewusst KEINE Message (`handle()` ist noch
//!   `unimplemented!()`-Stub aus T7). Damit ist die einzige Termination-
//!   Quelle der `mailbox.recv() == None`-Arm im `handler_loop`.
//! - `drop(in_tx)` schliesst die Mailbox → `handler_loop` `break` →
//!   `handler_join` returnt → outer `select!` in `cell_task_long_running`
//!   aborted `io_join` → `join.await` terminiert vor 5 s.

use chrono::{TimeZone, Utc};
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind};
use meclaw_colony::{DbConn, cell_task_long_running};
use meclaw_core::{CellEmission, Path, Uuid};
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substrate_shutdown_on_mailbox_close_with_endless_sleep_until() {
    let active = vec![ActiveSchedule {
        schedule_id: Uuid::now_v7(),
        kind: ScheduleKind::At(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()),
    }];
    let cell = TimerCell::new(Path::new("/timer"), active, 5000);

    let (in_tx, in_rx) = mpsc::channel(8);
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

    // Sanity: nichts feuert innerhalb 200 ms (sleep_until liegt in 2099).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Mailbox close → handler.mailbox.recv() returns None → break →
    // outer aborts I/O sub-task. Termination via Mailbox-Close-Abort-Pfad.
    // (handle() ist noch unimplemented!() — Test sendet BEWUSST KEINE
    //  Message, sonst panickt der Handler statt sauber zu schliessen.)
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("Substrat hat nicht prompt geshutdownt")
        .unwrap();
}
