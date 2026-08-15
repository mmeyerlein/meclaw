//! Phase-10-C T14 / W10: the substrate shutdown duty. Endless I/O (a real
//! `getUpdates` against a mock server that accepts the connection but never
//! closes it → the reqwest future hangs; or a server returning 401, which puts
//! the I/O task into `BackoffState::Permanent` (a 5 min sleep)).
//! Both paths must terminate promptly on mailbox close — through the
//! **mailbox-close abort path** in `cell_task_long_running`, NOT via a client
//! timeout or the backoff sleep expiring (phase-10-A lesson: the events-channel
//! close path is constructionally unreachable here).

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, cell_task_long_running};
use meclaw_core::{CellEmission, Message, Path};
use std::time::Duration;
use tokio::sync::mpsc;

/// Test helper: a TCP listener that accepts connections but NEVER writes an HTTP
/// response. The reqwest client hangs in the read — and its client timeout
/// (`long_poll_timeout_ms = 5000`) is considerably longer than the test deadline
/// (2 s). If the mailbox close does NOT take effect promptly, the test task hangs
/// until the client timeout → the test fails.
async fn start_blackhole_server() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            tokio::spawn(async move {
                // Read & drop bytes, never write. Connection stays open.
                let mut buf = [0u8; 1024];
                loop {
                    use tokio::io::AsyncReadExt;
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
async fn substrate_shutdown_on_mailbox_close_with_endless_long_poll() {
    let addr = start_blackhole_server().await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        5000,
        1,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );

    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/p"),
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

    // Sanity: no immediate hang (the long poll hangs on the blackhole server).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Mailbox close → handler.mailbox.recv() = None → break → outer
    // aborts I/O sub-task (die im reqwest-Read haengt). Termination via
    // Mailbox-close abort path — NOT via the client timeout (5 s > 2 s
    // Deadline).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("substrate did not shut down promptly (mailbox-close path)")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substrate_shutdown_during_backoff_sleep_terminates_promptly() {
    // Mock liefert 401 → BackoffState::Permanent (5min Sleep). Mailbox-
    // The close must abort the I/O task during the sleep, NOT only after 5 min.
    use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
    let resp = MockResponse {
        status: 401,
        body: b"unauthorized".to_vec(),
        content_type: "text/plain".into(),
        delay: None,
    };
    // One 401 response suffices — after that the task enters the 5 min permanent
    // sleep. If the I/O task made a second request the mock would panic — which
    // additionally proves here that the sleep really took effect.
    let (addr, _j, _c) = start_mock_server_capturing(vec![resp]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        2000,
        1,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let (in_tx, in_rx) = mpsc::channel::<Message>(8);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = DbConn::wrap(conn, None);

    let join = tokio::spawn(cell_task_long_running(
        Path::new("/p"),
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

    // Wait until the I/O task is in the permanent sleep (1st request sent,
    // 401 empfangen, Sleep gestartet). 300 ms reichen ueblicherweise.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Mailbox close → abort the I/O sub-task (which sits in the 5 min sleep).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect(
            "abort during the backoff sleep did NOT take effect promptly — W8 condition violated",
        )
        .unwrap();
}
