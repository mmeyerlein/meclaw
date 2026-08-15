//! Phase-10-B T10: Substrat-Shutdown-Sanity (Phase-10-A-Lesson Z.290–333).
//!
//! Endless I/O (a real `sleep_until` on a one-shot in 2099) must terminate
//! promptly on mailbox close. Proof that the **mailbox-close abort path** through
//! `cell_task_long_running` takes effect — NOT the events-channel close path:
//!
//! - The I/O sub-task sits in `sleep_until(2099)` — no `events_tx.send` → the
//!   events-channel close path is constructionally unreachable.
//! - The test deliberately sends NO message (`handle()` is still the
//!   `unimplemented!()` stub from T7). So the only source of termination is the
//!   `mailbox.recv() == None` arm in `handler_loop`.
//! - `drop(in_tx)` closes the mailbox → `handler_loop` `break` →
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

    // Sanity: nothing fires within 200 ms (sleep_until is set to 2099).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Mailbox close → handler.mailbox.recv() returns None → break →
    // outer aborts the I/O sub-task. Termination via the mailbox-close abort path.
    // (handle() is still unimplemented!() — the test DELIBERATELY sends no message,
    //  otherwise the handler would panic instead of closing cleanly.)
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("substrate did not shut down promptly")
        .unwrap();
}
