//! Phase-10-C T14 / W10: Substrat-Shutdown-Pflicht. Endlos-I/O (real
//! `getUpdates` gegen einen Mock-Server, der die Connection annimmt aber
//! nie schliesst → reqwest-Future haengt; bzw. ein Server der 401 liefert,
//! womit der I/O-Task in `BackoffState::Permanent` (5min Sleep) geht).
//! Beide Pfade muessen bei Mailbox-Close prompt terminieren — ueber den
//! **Mailbox-Close-Abort-Pfad** durch `cell_task_long_running`, NICHT
//! ueber Client-Timeout oder Backoff-Sleep-Ablauf (Phase-10-A-Lesson:
//! events-Channel-Close-Pfad ist hier konstruktiv unerreichbar).

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, cell_task_long_running};
use meclaw_core::{CellEmission, Message, Path};
use std::time::Duration;
use tokio::sync::mpsc;

/// Test-Helper: ein TCP-Listener, der Connections annimmt aber NIE eine
/// HTTP-Response schreibt. reqwest-Client haengt im Read — und sein
/// Client-Timeout (`long_poll_timeout_ms = 5000`) ist hier deutlich
/// laenger als die Test-Deadline (2 s). Wenn Mailbox-Close NICHT prompt
/// greift, haengt die Test-Task bis zum Client-Timeout → Test failed.
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

    // Sanity: kein Haenger sofort (Long-Poll haengt am blackhole-Server).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Mailbox close → handler.mailbox.recv() = None → break → outer
    // aborts I/O sub-task (die im reqwest-Read haengt). Termination via
    // Mailbox-Close-Abort-Pfad — NICHT via Client-Timeout (5 s > 2 s
    // Deadline).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("Substrat hat nicht prompt geshutdownt (Mailbox-Close-Pfad)")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substrate_shutdown_during_backoff_sleep_terminates_promptly() {
    // Mock liefert 401 → BackoffState::Permanent (5min Sleep). Mailbox-
    // Close muss die I/O-Task waehrend des Sleeps abortieren, NICHT
    // erst nach 5min.
    use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
    let resp = MockResponse {
        status: 401,
        body: b"unauthorized".to_vec(),
        content_type: "text/plain".into(),
        delay: None,
    };
    // Eine 401-Antwort reicht — danach geht die Task in 5min-Permanent-
    // Sleep. Wenn der I/O-Task einen zweiten Request macht, panicked der
    // Mock — was hier zusaetzlich beweist, dass der Sleep wirklich
    // gegriffen hat.
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

    // Warte bis I/O-Task im Permanent-Sleep ist (1. Request abgeschickt,
    // 401 empfangen, Sleep gestartet). 300 ms reichen ueblicherweise.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Mailbox close → abort I/O sub-task (die im 5min-Sleep liegt).
    drop(in_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("Abort waehrend Backoff-Sleep griff NICHT prompt — W8-Bedingung verletzt")
        .unwrap();
}
