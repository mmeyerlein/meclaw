//! T23 / phase-10-A lesson: the substrate shutdown duty. Endless I/O (a real
//! `initialize` against a blackhole TCP server) leaves the cell hanging in I/O;
//! the POC A timeout is generous (5000 ms) while the test deadline is 2 s. If the
//! mailbox close does NOT take effect promptly, the test task hangs until the A
//! timeout → the test fails.
//!
//! ## Termination path audit
//!
//! Termination MUST run through the **mailbox-close abort path** via
//! `cell_task_long_running` laufen:
//!
//! 1. `drop(in_tx)` closes the mailbox.
//! 2. `handler_loop` in `cell_task_long_running`: `mailbox.recv() => None
//!    => break` → Handler-Sub-Task endet normal.
//! 3. Outer-`tokio::select!` triggert auf `handler_join`-Arm.
//! 4. The outer task calls `io_join.abort()` → the I/O future (stuck in a
//!    reqwest read against the blackhole TCP server) is hard-aborted.
//! 5. The outer task returns → `h.await` in the test task resolves.
//!
//! This path is the **mailbox-close abort path**. It is NOT the events-channel
//! close path (phase-10-A lesson, the second-order trap):
//! `mcp::io::run_io` bindet `events_tx` + `_reconfig_rx` explizit im
//! `async move` scope, so that an accidental channel drop (the old trap) does
//! NOT terminate the I/O task prematurely.
//!
//! **Timing-Diskriminator:** Prompt (~200–300 ms) = Mailbox-Close-Abort
//! took effect. Only after ~5 s = the A timeout elapsing (mailbox-close path
//! broken) would the test fail against the 2 s deadline.

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::DbConn;
use meclaw_colony::cell_task_long_running;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_core::{CellEmission, Message, Path};
use std::net::SocketAddr;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Accept TCP connections but never write back. reqwest hangs in read.
/// Client-side A-Timeout (5000 ms) is deliberately larger than the
/// test deadline (2 s) — if the Mailbox-Close-Abort path does NOT fire
/// promptly, the test task will wait until A-Timeout and then fail.
async fn start_blackhole() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substrate_shutdown_on_mailbox_close_with_endless_initialize() {
    let addr = start_blackhole().await;
    let client = McpClient::new(&format!("http://{addr}/"), None).unwrap();
    let cell = McpCell::new(
        client,
        5_000, // A timeout considerably larger than the test deadline (2 s).
        5_000,
        "main_mcp".into(),
    );

    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(64);

    let h = tokio::spawn(cell_task_long_running(
        Path::new("/mcp"),
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
        Default::default(),
    ));

    // Let initialize hang for 200 ms against the blackhole, then close the mailbox.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(in_tx);

    // Outer must terminate well under the A-Timeout. Deadline 2 s.
    let r = tokio::time::timeout(Duration::from_secs(30), h).await;
    assert!(
        r.is_ok(),
        "cell_task_long_running did not terminate within 2 s after mailbox-close \
         (mailbox-close abort path broken — the A timeout fired instead)"
    );
}
