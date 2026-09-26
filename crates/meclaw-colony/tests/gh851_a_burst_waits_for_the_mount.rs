//! GH #851 — a burst at a mount is served when a slot frees within the wait.
//!
//! The listener hands a connection to its mount over a bounded channel. It used
//! to hand over with `try_send` and answer `503 surface busy` the moment the
//! channel was full. Measured: 20 parallel POSTs through a proxy without
//! keep-alive met 1–3 refusals in 500 frames in about one run of four —
//! scheduling jitter between two `recv` of a consumer that drains into its own
//! task, not a dead consumer. The listener now waits a bounded time for a slot
//! (`surfaces::HANDOFF_WAIT`) before it refuses.
//!
//! Measured at the receiver: the client reads the MOUNT's answer, not the
//! listener's refusal.

use meclaw_colony::surfaces::{HANDOFF_QUEUE, HANDOFF_WAIT};
use meclaw_colony::{HandedConnection, SurfaceEntry, SurfaceRegistry};
use meclaw_core::Path;
use meclaw_testing::surface_listener::surface_listener;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Generous outer failure marker.
const FAILURE_MARKER: Duration = Duration::from_secs(30);

/// What the mount answers every connection it is handed.
const SERVED: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nserved\n";

/// Answer one handed connection and end it the orderly way: FIN first, then take
/// the request nobody parsed, so the close does not turn into an RST that would
/// throw the answer away.
async fn serve(mut conn: HandedConnection) {
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        conn.stream.write_all(SERVED).await?;
        conn.stream.shutdown().await?;
        let mut unread = Vec::new();
        conn.stream.read_to_end(&mut unread).await?;
        Ok::<(), std::io::Error>(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_beyond_a_full_queue_is_served_when_a_slot_frees() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (mut rx, _held) = surfaces
        .register(
            "burst",
            SurfaceEntry {
                kind: "web",
                cell_path: Path::new("/w"),
                links: None,
            },
        )
        .await
        .expect("the mount is free");

    // Fill the handoff queue from the side, so the client below is certain to
    // meet a full one — the burst, compressed to its worst moment.
    let filler = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the filler");
    let filler_addr = filler.local_addr().expect("the filler address");
    let handoff = surfaces.take_handoff("burst").await.expect("mounted");
    let mut parked = Vec::new();
    for _ in 0..HANDOFF_QUEUE {
        let client = tokio::net::TcpStream::connect(filler_addr)
            .await
            .expect("connect to the filler");
        let (stream, peer) = filler.accept().await.expect("accept on the filler");
        handoff
            .try_send(HandedConnection { stream, peer })
            .expect("a free slot takes it");
        parked.push(client);
    }
    drop(handoff);

    let (addr, listener) = surface_listener(surfaces.clone()).await;

    // The consumer is late, not gone: it starts draining well inside the wait.
    let late_by = HANDOFF_WAIT / 3;
    let consumer = tokio::spawn(async move {
        tokio::time::sleep(late_by).await;
        while let Some(conn) = rx.recv().await {
            tokio::spawn(serve(conn));
        }
    });

    let started = std::time::Instant::now();
    let text = tokio::time::timeout(FAILURE_MARKER, async move {
        let mut s = tokio::net::TcpStream::connect(addr).await?;
        s.write_all(b"GET /burst/info HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    })
    .await
    .expect("the client is answered within the failure marker")
    // A refusal that closes over the unread request arrives as a reset; it is
    // reported as what the client read rather than as a panic of its own.
    .map(|b| String::from_utf8_lossy(&b).into_owned())
    .unwrap_or_else(|e| format!("<io error: {e}>"));
    let took = started.elapsed();

    assert!(
        text.starts_with("HTTP/1.1 200") && text.ends_with("served\n"),
        "a connection that meets a full queue is served once a slot frees within \
         {HANDOFF_WAIT:?}, not refused; the client read {text:?} after {took:?}"
    );
    assert!(
        took >= late_by,
        "it was served after the consumer came back, not around it: {took:?}"
    );

    listener.abort();
    consumer.abort();
    drop(parked);
}
