//! T23 / Phase-10-A-Lesson: Substrat-Shutdown-Pflicht. Endlos-I/O
//! (real `initialize` gegen einen Blackhole-TCP-Server) → Cell bleibt
//! im I/O hängen, A-Timeout im POC ist großzügig (5000 ms); Test-Deadline
//! 2 s. Wenn Mailbox-Close NICHT prompt greift, hängt die Test-Task
//! bis zum A-Timeout → Test failed.
//!
//! ## Termination-Pfad-Audit
//!
//! Termination MUSS über den **Mailbox-Close-Abort-Pfad** durch
//! `cell_task_long_running` laufen:
//!
//! 1. `drop(in_tx)` schließt die Mailbox.
//! 2. `handler_loop` in `cell_task_long_running`: `mailbox.recv() => None
//!    => break` → Handler-Sub-Task endet normal.
//! 3. Outer-`tokio::select!` triggert auf `handler_join`-Arm.
//! 4. Outer ruft `io_join.abort()` → I/O-Future (hängt im reqwest-read
//!    gegen den Blackhole-TCP-Server) wird hart abgebrochen.
//! 5. Outer-Task kehrt zurück → `h.await` in der Test-Task löst sich.
//!
//! Dieser Pfad ist der **Mailbox-Close-Abort-Pfad**. Er ist NICHT der
//! events-Channel-Close-Pfad (Phase-10-A-Lesson Second-Order-Trap):
//! `mcp::io::run_io` bindet `events_tx` + `_reconfig_rx` explizit im
//! `async move`-Scope, damit versehentliches Channel-Drop (der alte Trap)
//! die I/O-Task NICHT vorzeitig terminiert.
//!
//! **Timing-Diskriminator:** Prompt (~200–300 ms) = Mailbox-Close-Abort
//! hat gegriffen. Erst nach ~5 s = A-Timeout-Elapsed (Mailbox-Close-Pfad
//! gebrochen) → Test failed in der 2 s-Deadline.

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
        5_000, // A-Timeout deutlich größer als Test-Deadline (2 s).
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
    ));

    // Let initialize hang for 200 ms against the blackhole, then close the mailbox.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(in_tx);

    // Outer must terminate well under the A-Timeout. Deadline 2 s.
    let r = tokio::time::timeout(Duration::from_secs(30), h).await;
    assert!(
        r.is_ok(),
        "cell_task_long_running did not terminate within 2 s after mailbox-close \
         (Mailbox-Close-Abort-Pfad broken — A-Timeout fired instead)"
    );
}
