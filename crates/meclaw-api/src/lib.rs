//! HTTP API + operator web UI for meclaw (phase 12).
//!
//! Symmetry with the /colony/* endpoints is a data-plane statement: the same
//! typed ColonyMsg inbox variant + oneshot ack, NOT literally route(). See
//! docs/meclaw-overview.md § `/colony` as a virtual endpoint.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod handlers;
pub mod router;
pub mod ui;

pub use router::AppState;

/// Re-export of `axum` for consumers that need `serve` + `with_graceful_shutdown`
/// without carrying axum as their own top-level dep (e.g. `meclaw-cli`).
/// Consistent with the plan's rule that "meclaw-api is the HTTP layer".
pub use axum;

/// Serve one already-accepted connection with the API router until it ends.
///
/// The CLI's one listener decides a connection by its first request line and
/// serves the API on whatever no cell mounted (ADR-0031). From there on it is an
/// ordinary HTTP/1.1 connection with upgrades — exactly what `axum::serve`
/// builds per accept, spelled out here because the accept happened somewhere
/// else. Upgrades are on because `/colony/events` is a WebSocket door.
///
/// `meclaw_cells::handed::serve_handed` is the same hyper call for the mounted
/// half of that listener. The duplication is deliberate: a cell may not depend
/// on the HTTP API (GH #381), and two hyper calls are the cheaper of the two
/// prices. Only this copy has the shutdown door below, because only this side is
/// drained by the listener: a connection a cell was handed belongs to the cell.
///
/// This connection is served until the client or the server ends it. Nothing
/// interrupts it — the caller that wants a graceful end on a signal calls
/// [`serve_handed_until`].
pub async fn serve_handed(stream: tokio::net::TcpStream, router: axum::Router) {
    serve_handed_until(stream, router, std::future::pending::<()>()).await;
}

/// [`serve_handed`], plus a signal that asks the connection to finish.
///
/// When `shutdown` resolves, the connection is told to stop reading new requests
/// and to end once the request it is serving is answered — hyper's
/// `graceful_shutdown`, which is what `axum::serve(..).with_graceful_shutdown(..)`
/// says to each of its connections. The call returns when the connection is
/// over, not when the signal arrives, so a caller that awaits it drains rather
/// than cuts.
///
/// Errors are logged and never returned: a client that hangs up mid-request is
/// the ordinary end of a connection, and there is nobody above this call to
/// tell.
pub async fn serve_handed_until<F>(stream: tokio::net::TcpStream, router: axum::Router, shutdown: F)
where
    F: std::future::Future<Output = ()>,
{
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    let service = TowerToHyperService::new(router);
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    let mut conn = std::pin::pin!(conn);
    let mut shutdown = std::pin::pin!(shutdown);
    // The guard is what a `fuse()` would be: the signal future is polled at most
    // once to completion, and asking for a graceful end twice is neither needed
    // nor allowed.
    let mut asked = false;
    loop {
        tokio::select! {
            served = conn.as_mut() => {
                if let Err(e) = served {
                    tracing::debug!(error = %e, "handed connection ended with an error");
                }
                return;
            }
            _ = &mut shutdown, if !asked => {
                asked = true;
                tracing::debug!("a handed connection was asked to finish");
                conn.as_mut().graceful_shutdown();
            }
        }
    }
}

use meclaw_colony::ColonyMsg;

/// Handle for HTTP handlers to send into colony's inbox.
/// `inbox` is Send+Sync (mpsc::Sender), so it may live in an Arc<ColonyHandle>.
///
/// `templates_root` is needed for `POST /colony/templates/rescan` (phase 12-B
/// T8.5), which passes the path through in `ColonyMsg::RescanTemplates`. Tests
/// that do not drive a rescan may leave `PathBuf::new()` as a stub.
#[derive(Clone)]
pub struct ColonyHandle {
    pub inbox: tokio::sync::mpsc::Sender<ColonyMsg>,
    pub templates_root: std::path::PathBuf,
}
