//! The colony's one listener: the first request line says who serves the
//! connection.
//!
//! One socket is bound for the whole process. What arrives on it is either a
//! client of a surface cell — a WebSocket, a phone edge, a poller — or a client
//! of whatever else the process serves on that socket, and the two are told
//! apart by the first path segment of the request line: a segment that names a
//! mount in the process's mount table (ADR-0031) belongs to the cell that
//! registered it, everything else to the [`Fallback`]. The stream is handed over
//! unread, so the cell serves its own protocol on its own routes; nothing here
//! parses a body, and no request is proxied.
//!
//! The fallback is a parameter because the core has two callers with two
//! answers: the CLI serves the HTTP API on it, a test serves `404 not found`
//! (`meclaw_testing::surface_listener`). Neither answer belongs in this module,
//! and the HTTP API cannot: it lives a crate above this one.
//!
//! **A connection is decided once, for its whole life** (ruling O-639-6). A
//! keep-alive connection is not re-inspected, so a later request on it reaches
//! the backend the first request chose: ask for `/voice/info` and then `/health`
//! on one connection and the second answer comes from the voice cell's router,
//! which does not know that route, rather than from the API. A client that wants
//! both doors uses two connections, and the clients this listener is for do:
//! a browser pools per origin and gets a fresh connection for the other door, a
//! WebSocket is one connection for one door for its whole life, and a proxy in
//! front opens what its own rules say.
//! `a_reused_connection_keeps_the_backend_of_its_first_request` in
//! `meclaw-cli`'s `mux` pins the behaviour rather than the hope.

use super::{BoxFuture, HandedConnection, SurfaceRegistry};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// How long a connection may take to send its request line before it is dropped.
pub(crate) const PEEK_TIMEOUT: Duration = Duration::from_secs(5);

/// The most bytes peeked for the request line.
pub(crate) const PEEK_MAX: usize = 4096;

/// How long the peek loop waits before looking at an incomplete request line
/// again.
///
/// A peek leaves the bytes where they are, and a socket that holds half a
/// request line stays readable for as long as that half sits there — asking
/// again at once would spin a core until [`PEEK_TIMEOUT`]. The pause is the
/// price of not consuming a byte, and not consuming one is the whole point: the
/// cell is handed a stream with its request still in it.
const PEEK_INTERVAL: Duration = Duration::from_millis(5);

/// How long the refusal below may take to leave. A client that will not read it
/// is not worth a task.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(1);

/// What a mount that cannot take another connection answers, verbatim on the
/// wire. `Connection: close` because there is nothing behind it to keep alive.
const SURFACE_BUSY: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 13\r\nConnection: close\r\n\r\nsurface busy\n";

/// How long the accept loop waits after an accept error of the resource class.
///
/// An exhausted file-descriptor table fails every accept at once, and a loop
/// that retried immediately would burn a core for as long as the table stays
/// full without any connection being served by it. One second is what
/// `axum::serve` waits, and it is deliberately long: the errors of the other
/// class, the ones that belong to a single connection, never reach it (see
/// [`is_connection_error`]).
pub(crate) const ACCEPT_BACKOFF: Duration = Duration::from_secs(1);

/// How long a shutdown waits for the fallback connections that are still in
/// flight.
///
/// A request that is being answered when the signal arrives is answered; the
/// cap is there so a client that never reads its answer cannot hold the process
/// open. Connections a cell was handed are NOT waited for: the stream belongs to
/// the cell, and the cell's own life decides it. Neither is a connection that had
/// asked for nothing — it is dropped instead, so this cap and `PEEK_TIMEOUT`
/// never add up; that both happen to be five seconds means nothing, and one moves
/// without the other.
pub const DRAIN_CAP: Duration = Duration::from_secs(5);

/// Who serves a connection whose first path segment names no mount.
///
/// The core knows when a connection is nobody's mount; it does not know what to
/// answer, and deliberately so. The CLI answers with the HTTP API router, a test
/// with a fixed `404`.
pub trait Fallback: Send + Sync + 'static {
    /// Serve a connection whose first path segment is no mount.
    ///
    /// The stream arrives unread, the same way a mounted cell gets it. The
    /// future is awaited by the connection's task, so whatever it waits for is
    /// what the drain waits for.
    fn serve(&self, stream: TcpStream, peer: SocketAddr) -> BoxFuture<'static, ()>;
}

/// True for the accept errors that belong to one connection rather than to the
/// process.
///
/// A client that connects and resets at once fails the accept of exactly its own
/// connection, so the loop goes straight back to accepting: no log line, no
/// pause. Paying the resource-class pause here would let one such client hold
/// the whole accept loop down, and it would write a line per attempt.
/// `axum::serve` splits the two classes the same way.
pub(crate) fn is_connection_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}

/// Accept one connection. `None` means "try again", and it has said whatever
/// there was to say about why.
async fn accept(listener: &TcpListener) -> Option<(TcpStream, SocketAddr)> {
    match listener.accept().await {
        Ok(accepted) => Some(accepted),
        Err(e) if is_connection_error(&e) => None,
        Err(e) => {
            tracing::error!(error = %e, "accept error");
            tokio::time::sleep(ACCEPT_BACKOFF).await;
            None
        }
    }
}

/// Position of the first CRLF in `head`, if there is one.
fn find_crlf(head: &[u8]) -> Option<usize> {
    head.windows(2).position(|pair| pair == b"\r\n")
}

/// The first path segment of an HTTP/1 request line, or `None` when there is no
/// complete line in `head`.
///
/// `None` also covers a line that is not a rooted-path request (an absolute-URI
/// target, a `CONNECT`, anything unparseable): the caller sends those to the
/// fallback, which answers them the way it answers any other request it does not
/// like. A request for `/` has an empty first segment, which is a segment no
/// mount may be called.
pub(crate) fn first_segment(head: &[u8]) -> Option<&str> {
    let line_end = find_crlf(head)?;
    let line = std::str::from_utf8(&head[..line_end]).ok()?;
    let mut words = line.split(' ');
    let _method = words.next()?;
    let target = words.next()?;
    let path = target.strip_prefix('/')?;
    path.split(['/', '?', '#']).next()
}

/// Peek until the request line is complete. `None` means the connection is not
/// worth serving: it said nothing in time, it hung up, or its request line is
/// longer than [`PEEK_MAX`].
async fn peek_request_line(stream: &TcpStream, buf: &mut [u8]) -> Option<usize> {
    let deadline = tokio::time::Instant::now() + PEEK_TIMEOUT;
    let mut seen = 0usize;
    loop {
        if find_crlf(&buf[..seen]).is_some() {
            return Some(seen);
        }
        if seen == buf.len() {
            return None;
        }
        let peeked = tokio::time::timeout_at(deadline, stream.peek(buf)).await;
        let grown = match peeked {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => n,
        };
        if grown == seen {
            // Nothing new arrived; wait rather than ask again at once.
            if tokio::time::timeout_at(deadline, tokio::time::sleep(PEEK_INTERVAL))
                .await
                .is_err()
            {
                return None;
            }
        }
        seen = grown;
    }
}

/// Tell a client that the mount it asked for cannot take it, and close.
async fn refuse_busy(mut stream: TcpStream, peer: SocketAddr, mount: &str) {
    tracing::warn!(%peer, mount, "the surface cannot take another connection");
    let written = tokio::time::timeout(REFUSAL_TIMEOUT, stream.write_all(SURFACE_BUSY)).await;
    if let Ok(Err(e)) = written {
        tracing::debug!(%peer, error = %e, "the refusal did not reach the client");
    }
}

/// Decide one connection and give it to whoever serves it.
///
/// `signal` is the listener's shutdown, and it reaches this in one place: before
/// the decision it ends the wait, because a connection that has asked for
/// nothing yet is dropped where it stands — there is nothing to finish and
/// nobody is owed an answer. After the decision the fallback's own graceful end
/// takes over, and the mounted side has none: that stream belongs to its cell.
async fn decide(
    stream: TcpStream,
    peer: SocketAddr,
    fallback: Arc<dyn Fallback>,
    surfaces: Arc<SurfaceRegistry>,
    signal: Arc<watch::Sender<()>>,
) {
    if let Err(e) = stream.set_nodelay(true) {
        // Not fatal: Nagle costs latency, it does not break a request.
        tracing::debug!(%peer, error = %e, "TCP_NODELAY was refused");
    }
    let mut head = vec![0u8; PEEK_MAX];
    // The peek races the shutdown. Without this arm a silent socket — a browser
    // preconnect is the everyday one — would sit out its whole peek budget and
    // hold the drain for it, for a connection that never wanted anything.
    let peeked = tokio::select! {
        peeked = peek_request_line(&stream, &mut head) => peeked,
        _ = signal.closed() => {
            tracing::debug!(%peer, "the listener is shutting down and this connection had asked for nothing; dropping it");
            return;
        }
    };
    let Some(len) = peeked else {
        tracing::debug!(%peer, "no request line within the peek budget; dropping");
        return;
    };
    let segment = first_segment(&head[..len]).map(str::to_string);
    let handoff = match &segment {
        Some(mount) => surfaces.take_handoff(mount).await,
        None => None,
    };
    let Some(handoff) = handoff else {
        fallback.serve(stream, peer).await;
        return;
    };
    // The mount is expected to be reading; a full or closed channel is the cell
    // saying so, and the client hears it rather than waiting.
    let mount = segment.unwrap_or_default();
    if let Err(refused) = handoff.try_send(HandedConnection { stream, peer }) {
        refuse_busy(refused.into_inner().stream, peer, &mount).await;
    }
}

/// Accept until `shutdown` resolves, then drain the fallback side; every
/// connection is decided by its first request line.
///
/// The shutdown does four things, in this order: the accept loop stops, every
/// connection that has not sent a request line yet is dropped where it stands,
/// every connection on the fallback is asked to finish the request it is serving
/// and then end, and this call waits for those to be over (at most
/// [`DRAIN_CAP`]). That is what `axum::serve(..).with_graceful_shutdown(..)` did
/// for this socket, down to the silent connection: such a socket is idle in
/// hyper's sense, and `graceful_shutdown` ends an idle connection at once rather
/// than waiting for a request that may never come. And it is why the colony
/// shutdown that runs after this call does not run against a `POST /messages`
/// that is still in a cell.
///
/// **Connections handed to a cell are deliberately not waited for.** The stream
/// belongs to the cell from the moment it is handed over: a call in progress on a
/// `voice` mount is the cell's to end, and the listener has no business closing
/// it. What the drain covers is the fallback's own work and nothing else.
///
/// Asking the fallback to finish is the **fallback's** business: it is built with
/// whatever signal it needs (the CLI drops a `watch` receiver when this call's
/// `shutdown` fires), because the trait hands it a stream and nothing else.
///
/// Two `watch` channels carry the rest, the shape `axum::serve` uses: dropping
/// the receiver of the first tells every connection task that had asked for
/// nothing to stop waiting, and the sender of the second resolves once the last
/// task has dropped its receiver.
pub async fn serve<F>(
    listener: TcpListener,
    registry: Arc<SurfaceRegistry>,
    fallback: Arc<dyn Fallback>,
    shutdown: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    // `signal_rx` is dropped when the accept loop ends; every task holds a
    // sender, and `closed()` on it resolves exactly then.
    let (signal_tx, signal_rx) = watch::channel(());
    let signal_tx = Arc::new(signal_tx);
    // Every task holds a `close_rx`; `close_tx.closed()` resolves when the last
    // of them is gone. A task that handed its stream to a cell drops its
    // receiver at once, so a mount never holds the drain.
    let (close_tx, close_rx) = watch::channel(());

    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::debug!("the listener stops accepting");
                break;
            }
            accepted = accept(&listener) => {
                let Some((stream, peer)) = accepted else { continue };
                let fallback = Arc::clone(&fallback);
                let surfaces = Arc::clone(&registry);
                let signal = Arc::clone(&signal_tx);
                let close = close_rx.clone();
                tokio::spawn(async move {
                    decide(stream, peer, fallback, surfaces, signal).await;
                    drop(close);
                });
            }
        }
    }

    drop(listener);
    // Both of these are what the tasks are waiting on: the first asks them to
    // finish, the second is this loop's own share of the count.
    drop(signal_rx);
    drop(close_rx);
    if tokio::time::timeout(DRAIN_CAP, close_tx.closed())
        .await
        .is_err()
    {
        tracing::warn!(
            drain_cap_secs = DRAIN_CAP.as_secs(),
            "a connection did not finish inside the drain; it ends with the process"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_segment_is_read_off_the_request_line() {
        assert_eq!(
            first_segment(b"GET /voice/ws?session=1 HTTP/1.1\r\nHost: x\r\n"),
            Some("voice")
        );
        assert_eq!(
            first_segment(b"GET /colony/graph HTTP/1.1\r\n"),
            Some("colony")
        );
        assert_eq!(first_segment(b"GET / HTTP/1.1\r\n"), Some(""));
        assert_eq!(first_segment(b"GET /voice"), None, "no line yet");
        assert_eq!(first_segment(b"\r\n"), None, "not a request line");
        assert_eq!(
            first_segment(b"CONNECT example.com:443 HTTP/1.1\r\n"),
            None,
            "a target that is not a rooted path belongs to the fallback's own refusal"
        );
    }

    #[test]
    fn an_accept_error_is_one_connection_or_the_whole_process() {
        use std::io::{Error, ErrorKind};
        for kind in [
            ErrorKind::ConnectionRefused,
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
        ] {
            assert!(
                is_connection_error(&Error::new(kind, "the client went away")),
                "{kind:?} costs its own connection and nothing else"
            );
        }
        // The resource class, and the classes that are neither: both pay the
        // pause, because neither is a client that hung up.
        for kind in [
            ErrorKind::Other,
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidInput,
        ] {
            assert!(
                !is_connection_error(&Error::new(kind, "the table is full")),
                "{kind:?} is not one connection's business"
            );
        }
    }
}
