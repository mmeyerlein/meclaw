//! Serving a connection somebody else accepted.
//!
//! The CLI's one listener decides a connection by its first request line and
//! hands the stream, unread, to the cell that mounted the name it asked for
//! (ADR-0031). From there on it is an ordinary HTTP/1.1 connection with
//! upgrades — exactly what `axum::serve` builds per accept, spelled out here
//! because the accept happened somewhere else.
//!
//! `meclaw-api` carries the same helper for the API's own router. The
//! duplication is deliberate: a cell may not depend on the HTTP API (GH #381),
//! and ten lines of hyper call are the cheaper of the two prices.

/// Serve one already-accepted connection with `router` until it ends.
///
/// Upgrades are on, because the door this serves is a WebSocket door. Errors are
/// logged and never returned: a client that hangs up mid-request is the ordinary
/// end of a connection, and there is nobody above this call to tell.
pub async fn serve_handed(stream: tokio::net::TcpStream, router: axum::Router) {
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    let service = TowerToHyperService::new(router);
    let conn = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(e) = conn.await {
        tracing::debug!(error = %e, "handed connection ended with an error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_handed_stream_is_served_by_the_router_it_was_given() {
        let router =
            axum::Router::new().route("/voice/info", axum::routing::get(|| async { "hello" }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (stream, _) = l.accept().await.expect("accept");
            serve_handed(stream, router).await;
        });
        let body = reqwest::get(format!("http://{addr}/voice/info"))
            .await
            .expect("get")
            .text()
            .await
            .expect("text");
        assert_eq!(body, "hello");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), server).await;
    }
}
