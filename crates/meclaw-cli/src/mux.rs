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
    use std::time::Duration;

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
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("the accept loop ends on shutdown")
            .expect("no panic");
    }

    /// Ruling O-639-6, pinned: the first request line decides the whole life of a
    /// connection, so the second request on a reused one reaches the first one's
    /// backend. The count of handoffs is the receipt that the connection really
    /// was reused — a second connection would have been a second handoff, and
    /// then `/health` would have answered `200`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reused_connection_keeps_the_backend_of_its_first_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};

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

        // One client, one idle connection kept: the body of the first answer is
        // read to the end, which is what puts the connection back in the pool.
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(1)
            .build()
            .expect("build the client");
        let info = client
            .get(format!("http://{addr}/voice/info"))
            .send()
            .await
            .expect("the mount answers");
        assert_eq!(info.status(), 200);
        assert_eq!(info.text().await.expect("text"), "voice here");

        let health = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .expect("the second request is answered by somebody");
        assert_eq!(
            handed.load(Ordering::Relaxed),
            1,
            "the second request rode the first connection, so nothing was handed over twice"
        );
        assert_eq!(
            health.status(),
            404,
            "the voice cell's router answers it, and it has no /health: a client that \
             wants the API on this connection has already lost"
        );

        let _ = stop_tx.send(());
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("the accept loop ends on shutdown")
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_shutdown_drops_a_connection_that_asked_for_nothing() {
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

        let signalled = std::time::Instant::now();
        let _ = stop_tx.send(());
        tokio::time::timeout(DRAIN_CAP + Duration::from_secs(5), server)
            .await
            .expect("serve returns")
            .expect("no panic");
        let served_after = signalled.elapsed();
        assert!(
            served_after < Duration::from_secs(1),
            "a connection that asked for nothing must not be waited for; serve took \
             {served_after:?} of its {DRAIN_CAP:?} cap"
        );

        // And the socket is closed rather than merely un-awaited: a read on it
        // ends at once, which is what the client of a shut-down listener should
        // see.
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(30), silent.read(&mut byte))
            .await
            .expect("the read ends within the failure marker")
            .expect("the socket reads");
        assert_eq!(read, 0, "the listener closed the connection it dropped");
    }

    /// The drain: a request that is being answered when the shutdown fires is
    /// answered, and `serve` returns after it rather than before it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_shutdown_waits_for_the_answer_a_client_is_already_owed() {
        /// How long the slow handler takes. Long enough that the shutdown lands
        /// inside it for certain, short enough to be a fraction of [`DRAIN_CAP`].
        const HANDLER_WORK: Duration = Duration::from_millis(300);

        let surfaces = Arc::new(SurfaceRegistry::new());
        // The handler says when it has started, so the shutdown fires while the
        // request really is in flight rather than after a hopeful sleep.
        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel::<()>(1);
        let api = Router::new().route(
            "/slow",
            get(move || {
                let started = started_tx.clone();
                async move {
                    let _ = started.send(()).await;
                    tokio::time::sleep(HANDLER_WORK).await;
                    "answered"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a throwaway port");
        let addr = listener.local_addr().expect("the bound address");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(listener, api, surfaces, async move {
            let _ = stop_rx.await;
        }));

        let request =
            tokio::spawn(async move { reqwest::get(format!("http://{addr}/slow")).await });
        tokio::time::timeout(Duration::from_secs(30), started_rx.recv())
            .await
            .expect("the handler starts within the failure marker")
            .expect("the handler reports its start");

        let signalled = std::time::Instant::now();
        let _ = stop_tx.send(());

        // `serve` is awaited FIRST, and how long it took is the discriminator: a
        // listener that only stops accepting returns within a millisecond or two
        // of the signal, while one that drains cannot return before the handler
        // it interrupted has finished. Half of the handler's work is the floor,
        // and the whole point is that the colony shutdown behind this call runs
        // after the answer rather than against it.
        tokio::time::timeout(DRAIN_CAP + Duration::from_secs(5), server)
            .await
            .expect("serve returns once the drain is over")
            .expect("no panic");
        let served_after = signalled.elapsed();
        assert!(
            served_after >= HANDLER_WORK / 2,
            "serve returned {served_after:?} after the signal, so it did not wait for the \
             answer it still owed"
        );
        assert!(
            served_after < DRAIN_CAP,
            "the drain ended with the connection, not at the cap; took {served_after:?}"
        );

        let answer = tokio::time::timeout(Duration::from_secs(30), request)
            .await
            .expect("the answer arrives within the failure marker")
            .expect("no panic in the client task")
            .expect("the request the client was owed is answered, not cut");
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.text().await.expect("text"), "answered");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mount_that_reads_nothing_answers_that_it_is_busy() {
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

        let answer = tokio::time::timeout(
            Duration::from_secs(30),
            reqwest::get(format!("http://{addr}/voice/info")),
        )
        .await
        .expect("a full mount answers instead of parking the client")
        .expect("the request itself went through");
        assert_eq!(answer.status(), 503);
        assert_eq!(
            answer.text().await.expect("text"),
            "surface busy\n",
            "the refusal says what happened"
        );

        let _ = stop_tx.send(());
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("the accept loop ends on shutdown")
            .expect("no panic");
    }
}
