//! One listener for every surface a test fixture boots.
//!
//! A `web` or `voice` cell has no port of its own: it registers a mount and is
//! reached at `/<mount>/…` on the colony's one listener. A test that wants to
//! talk to such a cell therefore needs that listener, and it is the same core
//! the CLI runs — [`meclaw_colony::surfaces::listener::serve`] — with a
//! fallback of two lines instead of the HTTP API.
//!
//! The fallback answers `404 not found` to every request whose first path
//! segment names no mount. That is the whole difference to the shipped
//! listener, and it is what makes the helper honest: a test that asks for the
//! wrong mount reads a refusal rather than hanging on a socket nobody serves.

use meclaw_colony::surfaces::{BoxFuture, SurfaceRegistry, listener};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

/// What the fallback answers, verbatim on the wire. `Connection: close`, because
/// there is nothing behind it to keep alive; the body is ten bytes long and the
/// header says so.
const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 10\r\nConnection: close\r\n\r\nnot found\n";

/// How long the refusal may take to leave and the connection to end. A client
/// that will not read it is not worth a test's patience (hard rule 12: every
/// I/O here carries its own deadline).
const REFUSAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The fallback of a test's listener: a fixed `404`, no router, no crate above
/// this one.
struct NotFound;

impl listener::Fallback for NotFound {
    fn serve(&self, mut stream: TcpStream, _peer: SocketAddr) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = tokio::time::timeout(REFUSAL_TIMEOUT, answer_404(&mut stream)).await;
        })
    }
}

/// Write the refusal and end the connection so the client can read it.
///
/// The two lines after the write are not ceremony. The listener **peeks** the
/// request line and hands the stream over unread, so the client's request is
/// still sitting in this socket's receive buffer — and a socket closed with
/// unread data sends an RST, which throws away the answer that was already on
/// the wire. `shutdown` sends the FIN first, the drain then takes the request
/// nobody parsed, and the close after it is orderly.
async fn answer_404(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(NOT_FOUND).await?;
    stream.shutdown().await?;
    let mut unread = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(stream, &mut unread).await?;
    Ok(())
}

/// Bind one listener for `registry` and serve it until the returned handle is
/// aborted.
///
/// The address it returns is where every mount in `registry` is reachable:
/// `http://<addr>/<mount>/…`. The registry is told about it
/// ([`SurfaceRegistry::set_listener`]), the way the CLI tells it, so a fixture
/// that reads `/colony/surfaces` sees the same thing a colony would publish.
///
/// **Ending it.** The accept loop runs until the task is aborted —
/// `handle.abort()`, or the end of the `#[tokio::test]` runtime, which drops
/// every task on it. There is no graceful drain to wait for: a test's listener
/// owes nobody an answer after the test.
///
/// What `abort()` ends is the ACCEPT LOOP, not the connections already handed
/// over: those run on tasks of their own, held by the mount's own cell, and
/// they end with that cell or with the test's runtime. A test that aborts the
/// listener and waits for an open socket to die waits for the runtime. Let the
/// cell end instead — that is what closes its sockets.
///
/// # Panics
///
/// If the port it was given cannot be bound — which means the machine is not one
/// a test can serve on.
pub async fn surface_listener(registry: Arc<SurfaceRegistry>) -> (SocketAddr, JoinHandle<()>) {
    // `free_port` leaves one window open, by its own account: two test
    // PROCESSES probing the same port out of ten thousand in the same
    // microseconds. One fixture per test binary hit that about never; a
    // fixture per surface cell hits it often enough to be seen (measured on a
    // box running two gates at once), so the answer is to ask again rather
    // than to fail the test that lost the race.
    let mut last = None;
    for _ in 0..BIND_ATTEMPTS {
        let port = crate::ports::free_port();
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(bound) => return serve_on(bound, port, registry),
            Err(e) => last = Some((port, e)),
        }
    }
    match last {
        Some((port, e)) => panic!("no test listener could be bound; {port} answered {e}"),
        None => panic!("BIND_ATTEMPTS must be at least one"),
    }
}

/// How many ports a listener asks for before it gives up. Each one is a fresh
/// `free_port`, so an attempt never retries the port that just lost.
const BIND_ATTEMPTS: usize = 8;

/// Publish `bound` as the registry's listener and serve it.
fn serve_on(
    bound: tokio::net::TcpListener,
    port: u16,
    registry: Arc<SurfaceRegistry>,
) -> (SocketAddr, JoinHandle<()>) {
    // The port was named rather than asked for (`free_port`, not `:0`), so the
    // address is known without asking the socket for it.
    let addr = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    registry.set_listener(addr);
    let handle = tokio::spawn(listener::serve(
        bound,
        registry,
        Arc::new(NotFound),
        std::future::pending::<()>(),
    ));
    (addr, handle)
}

/// How long a fixture waits for a cell to put its name on the mount table.
///
/// The repo's 30 s failure-marker convention: only ever reached when the cell
/// under test never registered at all, which is the defect, not the wait.
const MOUNT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Wait until `mount` is on `registry`'s table.
///
/// A surface cell registers at the top of its I/O half, which runs after the
/// spawn returns — so a fixture that spawns a cell and immediately asks the
/// listener for `/<mount>/` races that registration and reads the fallback's
/// `404`. This is the positive signal to wait on instead: the name is there,
/// and the next request reaches the cell.
///
/// # Panics
///
/// If the name never appears within the failure-marker window.
pub async fn wait_for_mount(registry: &SurfaceRegistry, mount: &str) {
    let deadline = std::time::Instant::now() + MOUNT_WAIT;
    loop {
        if registry.take_handoff(mount).await.is_some() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no cell registered the mount {mount:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::{HandedConnection, SurfaceEntry};
    use meclaw_core::Path;
    use tokio::io::AsyncReadExt;

    /// What a client reads back, as one string, for one request on a fresh
    /// connection.
    async fn ask(addr: SocketAddr, target: &str) -> String {
        let mut c = TcpStream::connect(addr).await.expect("connect");
        c.write_all(
            format!("GET {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write the request");
        let mut answer = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            c.read_to_end(&mut answer),
        )
        .await
        .expect("the answer arrives within the failure marker")
        .expect("read");
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_listener_answers_404_for_an_unmounted_segment_and_hands_off_a_mount() {
        let registry = Arc::new(SurfaceRegistry::new());
        let (mut handed, _held) = registry
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
        // The mounted side: one fixed line per handed stream, written straight
        // on the socket. That is all a mount owes this test — the point is that
        // the stream got here at all, unread.
        tokio::spawn(async move {
            while let Some(HandedConnection { mut stream, .. }) = handed.recv().await {
                tokio::spawn(async move {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nvoice here\n",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                    // The request is still unread — the stream arrived peeked,
                    // not parsed — and closing on top of it would RST the
                    // answer away. Same reason as [`answer_404`].
                    let mut unread = Vec::new();
                    let _ = stream.read_to_end(&mut unread).await;
                });
            }
        });

        let (addr, listener_task) = surface_listener(Arc::clone(&registry)).await;
        assert_eq!(
            registry.listener(),
            Some(addr),
            "the helper publishes its address the way the colony's own listener does"
        );

        let mounted = ask(addr, "/voice/x").await;
        assert!(
            mounted.contains("200 OK") && mounted.ends_with("voice here\n"),
            "the mount was handed the stream: {mounted:?}"
        );

        let unmounted = ask(addr, "/nothing").await;
        assert!(
            unmounted.starts_with("HTTP/1.1 404 Not Found\r\n")
                && unmounted.ends_with("not found\n"),
            "an unmounted segment reads the fallback's refusal: {unmounted:?}"
        );

        listener_task.abort();
    }
}
