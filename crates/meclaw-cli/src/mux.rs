//! The colony's one listener, with the HTTP API as its fallback.
//!
//! The peek-and-hand-off core lives in [`meclaw_colony::surfaces::listener`]:
//! it reads the first request line of every accepted connection and hands the
//! stream, unread, to the cell that mounted the segment it names (ADR-0031).
//! What is left here is the answer for everything else — this process serves
//! the HTTP API on the same socket — and the graceful end that answer needs.
//!
//! # Why the fallback carries its own signal
//!
//! [`meclaw_colony::surfaces::listener::Fallback`] is handed a stream and
//! nothing else, deliberately: the core knows when a connection is nobody's
//! mount, not what to say to it, and not what "finish politely" means for
//! whoever says it. Hyper's graceful end is the API's own business, so
//! [`ApiFallback`] holds the `watch` sender that carries it and [`serve`] drops
//! the matching receiver the moment the process's shutdown fires. The core's
//! drain then waits for the connections that were still being answered — and
//! not for the ones a cell was handed, which belong to the cell.
//!
//! **A connection is decided once, for its whole life** (ruling O-639-6). The
//! tests below pin that, and the busy refusal, and the drain, through this
//! wiring rather than through the core alone: this is the shape that ships.

use meclaw_api::axum::Router;
use meclaw_colony::SurfaceRegistry;
use meclaw_colony::surfaces::{BoxFuture, listener};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::watch;

/// Serves the HTTP API on every connection whose first path segment names no
/// mount, and ends those connections when the process does.
struct ApiFallback {
    /// The API router, cloned per connection the way `axum::serve` clones it.
    api: Router,
    /// Resolves once [`serve`] has dropped the matching receiver, which is what
    /// asks hyper to finish the request in flight and then end.
    signal: Arc<watch::Sender<()>>,
}

impl listener::Fallback for ApiFallback {
    fn serve(&self, stream: TcpStream, _peer: SocketAddr) -> BoxFuture<'static, ()> {
        let api = self.api.clone();
        let signal = Arc::clone(&self.signal);
        Box::pin(async move {
            meclaw_api::serve_handed_until(stream, api, async move { signal.closed().await }).await;
        })
    }
}

/// Accept on `listener` until `shutdown` resolves; a mounted first segment is
/// handed to its cell, everything else is served the API router.
///
/// The whole of the behaviour — the peek budget, the `503 surface busy` a full
/// mount answers, the drain and its cap — is
/// [`meclaw_colony::surfaces::listener::serve`]; this call is the API half of
/// it.
pub async fn serve<F>(
    listener: tokio::net::TcpListener,
    api: Router,
    surfaces: Arc<SurfaceRegistry>,
    shutdown: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    // The drain signal of the API side. The receiver lives in the shutdown
    // future below and nowhere else, so `signal.closed()` in every API
    // connection resolves exactly when the process asked the listener to stop.
    let (drain_tx, drain_rx) = watch::channel(());
    let fallback = Arc::new(ApiFallback {
        api,
        signal: Arc::new(drain_tx),
    });
    listener::serve(listener, surfaces, fallback, async move {
        shutdown.await;
        drop(drain_rx);
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_api::axum::routing::get;
    use meclaw_colony::surfaces::listener::DRAIN_CAP;
    use meclaw_colony::{HandedConnection, SurfaceEntry};
    use meclaw_core::Path;
    use meclaw_testing::deadline;

    /// The listener's own debug line for a connection it dropped at the
    /// shutdown before it had asked for anything (`listener::decide`).
    const DROPPED_SILENT: &str = "this connection had asked for nothing";
    /// The listener's warning when the drain ran out at [`DRAIN_CAP`]
    /// (`listener::serve`).
    const DRAIN_RAN_OUT: &str = "a connection did not finish inside the drain";

    /// Every line the process's tracing recorder holds at `level` for `peer`
    /// that contains `needle`.
    fn lines_for(level: tracing::Level, peer: std::net::SocketAddr, needle: &str) -> Vec<String> {
        meclaw_testing::log_recorder::global().lines(level, &[&format!("peer={peer}"), needle])
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mounted_segment_is_handed_over_and_everything_else_reaches_the_api() {
        let surfaces = Arc::new(SurfaceRegistry::new());
        let (mut rx, _held) = surfaces
            .register(
                "voice",
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new("/v"),
                    links: None,
                },
            )
            .await
            .expect("the mount is free");
        // The mounted side answers with a one-line router of its own, on the
        // routes a mounted cell serves: `/<mount>/…`.
        tokio::spawn(async move {
            while let Some(handed) = rx.recv().await {
                tokio::spawn(meclaw_api::serve_handed(
                    handed.stream,
                    Router::new().route("/voice/info", get(|| async { "voice here" })),
                ));
            }
        });
        let api = Router::new().route("/health", get(|| async { "api here" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        assert_eq!(
            reqwest::get(format!("http://{addr}/voice/info"))
                .await
                .expect("the mount answers")
                .text()
                .await
                .expect("text"),
            "voice here"
        );
        assert_eq!(
            reqwest::get(format!("http://{addr}/health"))
                .await
                .expect("the API answers")
                .text()
                .await
                .expect("text"),
            "api here"
        );
        assert_eq!(
            reqwest::get(format!("http://{addr}/nothing"))
                .await
                .expect("an unmounted segment reaches the API")
                .status(),
            404
        );

        let _ = stop_tx.send(());
        deadline::hang("the accept loop ends on shutdown", server)
            .await
            .expect("no panic");
    }

    /// Reads one HTTP/1.1 answer off `stream`: its status code and its body,
    /// framed by `content-length`. `None` when the connection ends, or fails,
    /// before the answer is whole.
    async fn read_answer(stream: &mut TcpStream) -> Option<(u16, String)> {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let head_end = loop {
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break end;
            }
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = std::str::from_utf8(&buf[..head_end]).ok()?;
        let status = head.split(' ').nth(1)?.parse().ok()?;
        let length: usize = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })?;
        let body_start = head_end + 4;
        while buf.len() < body_start + length {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8(buf[body_start..body_start + length].to_vec())
            .ok()
            .map(|body| (status, body))
    }

    /// Ruling O-639-6, pinned: the first request line decides the whole life of a
    /// connection, so the second request on a reused one reaches the first one's
    /// backend.
    ///
    /// **Why the connection is provably reused:** both requests are written on
    /// one raw socket, the second only after the first answer was read whole.
    /// There is no client pool that could open a second connection behind the
    /// test's back (a pooled client puts its connection back asynchronously, and
    /// under load the second request went out on a fresh one and met the API).
    /// The answer to `/health` is then positively the voice cell's: its router's
    /// own fallback body, which the API never sends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reused_connection_keeps_the_backend_of_its_first_request() {
        use meclaw_api::axum::http::StatusCode;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;

        const VOICE_HAS_NO_ROUTE: &str = "the voice cell has no such route";
        let surfaces = Arc::new(SurfaceRegistry::new());
        let (mut rx, _held) = surfaces
            .register(
                "voice",
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new("/v"),
                    links: None,
                },
            )
            .await
            .expect("the mount is free");
        let handed = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&handed);
        tokio::spawn(async move {
            while let Some(connection) = rx.recv().await {
                counted.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(meclaw_api::serve_handed(
                    connection.stream,
                    Router::new()
                        .route("/voice/info", get(|| async { "voice here" }))
                        .fallback(|| async { (StatusCode::NOT_FOUND, VOICE_HAS_NO_ROUTE) }),
                ));
            }
        });
        let api = Router::new().route("/health", get(|| async { "api here" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        let (info, health) = deadline::hang("both answers on one connection", async {
            let mut stream = TcpStream::connect(addr).await.expect("connect");
            stream
                .write_all(b"GET /voice/info HTTP/1.1\r\nHost: mux\r\n\r\n")
                .await
                .expect("write the first request");
            let info = read_answer(&mut stream)
                .await
                .expect("the mount answers the first request");
            stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: mux\r\n\r\n")
                .await
                .expect("write the second request on the same connection");
            let health = read_answer(&mut stream)
                .await
                .expect("the second request is answered on the same connection");
            (info, health)
        })
        .await;

        assert_eq!(info, (200, "voice here".to_string()));
        assert_eq!(
            health,
            (404, VOICE_HAS_NO_ROUTE.to_string()),
            "the voice cell's router answers it, and it has no /health: a client that \
             wants the API on this connection has already lost"
        );
        assert_eq!(
            handed.load(Ordering::Relaxed),
            1,
            "one connection, one handoff"
        );

        let _ = stop_tx.send(());
        deadline::hang("the accept loop ends on shutdown", server)
            .await
            .expect("no panic");
    }

    /// A socket that never asked for anything does not hold the shutdown up.
    ///
    /// A browser preconnect is exactly this connection, and it is the everyday
    /// case rather than the exotic one: the peek budget of such a socket must not
    /// become the process's exit latency.
    ///
    /// **Why the connection is provably accepted** before the shutdown fires: the
    /// silent socket connects FIRST, and a listener accepts in the order the
    /// kernel queued. The `/health` answer that follows can only have been served
    /// by a later accept, so by the time it arrives the silent one is already in
    /// its peek loop with its five-second budget running. Without that ordering
    /// the test would pass on a connection the loop never saw.
    ///
    /// **Why it was not waited for**, without a clock: the listener says which
    /// end the peek took. Its line for this peer is the shutdown arm ("asked for
    /// nothing"), so the connection's task ended with the signal and the drain
    /// had nothing of it to wait for -- the peek budget's arm would have logged
    /// "no request line within the peek budget" instead.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_shutdown_drops_a_connection_that_asked_for_nothing() {
        // The recorder listens from here on; the listener's lines are the verdict.
        meclaw_testing::log_recorder::global();
        use tokio::io::AsyncReadExt;

        let surfaces = Arc::new(SurfaceRegistry::new());
        let api = Router::new().route("/health", get(|| async { "api here" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        let mut silent = tokio::net::TcpStream::connect(addr)
            .await
            .expect("the silent socket connects");
        let silent_peer = silent.local_addr().expect("the silent socket's address");
        assert_eq!(
            reqwest::get(format!("http://{addr}/health"))
                .await
                .expect("the API answers a later connection")
                .text()
                .await
                .expect("text"),
            "api here",
            "the answer proves the accept loop got past the silent connection"
        );

        let _ = stop_tx.send(());
        deadline::hang("serve returns after the shutdown", server)
            .await
            .expect("no panic");
        let dropped = lines_for(tracing::Level::DEBUG, silent_peer, DROPPED_SILENT);
        assert_eq!(
            dropped.len(),
            1,
            "a connection that asked for nothing is dropped by the shutdown, not waited \
             for until its peek budget runs out; lines for it: {dropped:?}"
        );

        // And the socket is closed rather than merely un-awaited: a read on it
        // ends, which is what the client of a shut-down listener should see.
        let mut byte = [0u8; 1];
        let read = deadline::hang(
            "the read on the dropped socket ends",
            silent.read(&mut byte),
        )
        .await
        .expect("the socket reads");
        assert_eq!(read, 0, "the listener closed the connection it dropped");
    }

    /// The drain: a request that is being answered when the shutdown fires is
    /// answered, and `serve` returns after it rather than before it.
    ///
    /// The handler is held until the shutdown has reached the listener (its
    /// shutdown future has resolved), then released. The order of the two ends
    /// is the discriminator, not a duration: a listener that only stops
    /// accepting returns at once and before the held answer, one that drains
    /// cannot return before the handler it interrupted has finished.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_shutdown_waits_for_the_answer_a_client_is_already_owed() {
        // The recorder listens from here on; the listener's lines are the verdict.
        meclaw_testing::log_recorder::global();
        use std::sync::Mutex;

        let surfaces = Arc::new(SurfaceRegistry::new());
        // What ended, in the order it ended: the handler, and `serve`.
        let ends: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        // The handler says when it has started, so the shutdown fires while the
        // request really is in flight, and waits for the test to release it.
        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel::<()>(1);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let handler_ends = Arc::clone(&ends);
        let api = Router::new().route(
            "/slow",
            get(move || {
                let started = started_tx.clone();
                let release = Arc::clone(&release_rx);
                let ends = Arc::clone(&handler_ends);
                async move {
                    let _ = started.send(()).await;
                    if let Some(rx) = release.lock().await.take() {
                        let _ = rx.await;
                    }
                    ends.lock().expect("ends").push("answered");
                    "answered"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel::<()>();
        let serve_ends = Arc::clone(&ends);
        let server = tokio::spawn(async move {
            serve(listener, api, surfaces, async move {
                let _ = stop_rx.await;
                let _ = stopping_tx.send(());
            })
            .await;
            serve_ends.lock().expect("ends").push("serve returned");
        });

        let request =
            tokio::spawn(async move { reqwest::get(format!("http://{addr}/slow")).await });
        deadline::hang("the handler starts", started_rx.recv())
            .await
            .expect("the handler reports its start");

        let _ = stop_tx.send(());
        deadline::hang("the shutdown reaches the listener", stopping_rx)
            .await
            .expect("the shutdown future resolves");
        let _ = release_tx.send(());

        deadline::hang("serve returns once the drain is over", server)
            .await
            .expect("no panic");
        assert_eq!(
            *ends.lock().expect("ends"),
            ["answered", "serve returned"],
            "serve returned before the answer it still owed"
        );
        let ran_out =
            meclaw_testing::log_recorder::global().lines(tracing::Level::WARN, &[DRAIN_RAN_OUT]);
        assert!(
            ran_out.is_empty(),
            "the drain ended with the connection, not at its {DRAIN_CAP:?} cap: {ran_out:?}"
        );

        let answer = deadline::hang("the owed answer arrives", request)
            .await
            .expect("no panic in the client task")
            .expect("the request the client was owed is answered, not cut");
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.text().await.expect("text"), "answered");
    }

    /// Sends `GET /voice/info` on a raw socket and reads the answer; returns the
    /// client's own address (the listener's `peer`) and the answer.
    async fn ask_voice(
        addr: std::net::SocketAddr,
    ) -> (std::net::SocketAddr, Option<(u16, String)>) {
        use tokio::io::AsyncWriteExt;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let peer = stream.local_addr().expect("the client's address");
        stream
            .write_all(b"GET /voice/info HTTP/1.1\r\nHost: mux\r\n\r\n")
            .await
            .expect("write the request");
        (peer, read_answer(&mut stream).await)
    }

    /// The `reason` fields the listener logged for `peer`'s refusal.
    fn refusal_reasons(peer: std::net::SocketAddr) -> Vec<&'static str> {
        ["reader_gone", "handoff_timeout"]
            .into_iter()
            .filter(|reason| {
                !lines_for(tracing::Level::DEBUG, peer, &format!("reason={reason}")).is_empty()
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mount_whose_reader_is_gone_answers_that_it_is_busy() {
        // The recorder listens from here on; the listener's lines are the verdict.
        meclaw_testing::log_recorder::global();
        // GH #660: the sentence both long-running cells' teardown relies on.
        // A cell that is going drops the receiving end of its handoff channel
        // BEFORE it gives the mount back, so there is a window in which the
        // name is still on the table and nothing reads it. A connection
        // arriving in that window has to be refused — the alternative is a
        // `try_send` that lands in a queue nobody will ever empty, and a client
        // that waits for an answer no one owes it any more.
        let surfaces = Arc::new(SurfaceRegistry::new());
        let (rx, _held) = surfaces
            .register(
                "voice",
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new("/v"),
                    links: None,
                },
            )
            .await
            .expect("the mount is free");
        // The I/O half ends: its receiver falls, its registration does not.
        // That is the window, held open for the length of this test.
        drop(rx);
        assert!(
            surfaces.table().await.iter().any(|r| r.mount == "voice"),
            "the window is only a window while the name is still registered"
        );

        let api = Router::new().route("/health", get(|| async { "api here" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        let (peer, answer) = deadline::hang(
            "a mount nobody reads answers instead of swallowing the client",
            ask_voice(addr),
        )
        .await;
        // GH #851: a FULL queue is waited on, a CLOSED one is not — the
        // teardown depends on hearing the refusal at once. The listener's
        // `reason` names the arm that refused: the closed queue's, never the
        // end of the wait for a slot.
        assert_eq!(
            refusal_reasons(peer),
            ["reader_gone"],
            "a mount whose reader is gone refuses at once, not after the wait for a slot"
        );
        assert_eq!(
            answer,
            Some((503, "surface busy\n".to_string())),
            "and it is the mount's refusal, not the API fallback's 404"
        );

        let _ = stop_tx.send(());
        deadline::hang("the accept loop ends on shutdown", server)
            .await
            .expect("no panic");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mount_that_reads_nothing_answers_that_it_is_busy() {
        // The recorder listens from here on; the listener's lines are the verdict.
        meclaw_testing::log_recorder::global();
        let surfaces = Arc::new(SurfaceRegistry::new());
        // The receiver is held and never read: the handoff channel fills up, and
        // the connection after that has to be refused rather than parked.
        let (_rx, _held) = surfaces
            .register(
                "voice",
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new("/v"),
                    links: None,
                },
            )
            .await
            .expect("the mount is free");
        // Fill the handoff channel from the side, so the one client below is
        // certain to meet a full one. Going through the listener for this would
        // park a client per slot and pay a failure marker for each.
        let filler = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the filler");
        let filler_addr = filler.local_addr().expect("the filler address");
        let handoff = surfaces.take_handoff("voice").await.expect("mounted");
        let mut parked = Vec::new();
        for _ in 0..meclaw_colony::surfaces::HANDOFF_QUEUE {
            let client = tokio::net::TcpStream::connect(filler_addr)
                .await
                .expect("connect to the filler");
            let (stream, peer) = filler.accept().await.expect("accept on the filler");
            handoff
                .try_send(HandedConnection { stream, peer })
                .expect("a free slot takes it");
            parked.push(client);
        }

        let api = Router::new().route("/health", get(|| async { "api here" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        let (peer, answer) = deadline::hang(
            "a full mount answers instead of parking the client",
            ask_voice(addr),
        )
        .await;
        // GH #851: a full queue whose reader is still there is given the wait
        // for a slot before it is refused. The listener's `reason` names the
        // arm that refused: the end of that wait, never the closed queue's.
        assert_eq!(
            refusal_reasons(peer),
            ["handoff_timeout"],
            "a full mount refuses only after the wait for a slot"
        );
        assert_eq!(
            answer,
            Some((503, "surface busy\n".to_string())),
            "the refusal says what happened"
        );

        let _ = stop_tx.send(());
        deadline::hang("the accept loop ends on shutdown", server)
            .await
            .expect("no panic");
    }
}
