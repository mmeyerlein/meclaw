//! W8 (GH #380): the LiveView socket, served by the cell itself.
//!
//! # Why the cell has its own loop
//!
//! `meclaw_surface::socket::Connection` answers a join by asking a cell over
//! the api's `Dispatcher` — message out, HTML back. A `web` cell **is** the
//! thing that would be asked, and it already holds the answer: the page was
//! materialised before the request arrived. Reusing that type would have meant
//! inventing a dispatcher pointing at ourselves. So the wire format moved to
//! `meclaw_surface::frames` (shared, tested once) and this is the cell's own
//! thin loop over it.
//!
//! R-W8-4b lands here: **a join does no diff work**. It answers from
//! [`Materialized::packed_tree`], which was built by a write, not by this read.
//!
//! # Which page a socket belongs to
//!
//! One cell, one container id — it is derived from the cell path — so the topic
//! alone cannot say which of several routes a viewer is looking at. The
//! LiveView client sends the page's URL in the join payload, and that is what
//! decides. A viewer is then registered under that route, and a write to it
//! reaches exactly the viewers of that page (Task 7).

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value, json};
use meclaw_surface::{frames, session};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::web::cell::{EventReply, WebEvent};
use std::time::Duration;

/// How long a browser event may wait for the handler's verdict.
const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
use crate::web::io::WebIo;
use crate::web::render::PageMap;

/// What a joined viewer is sent.
#[derive(Debug, Clone)]
pub enum ViewerMsg {
    /// A raw frame, already encoded.
    Frame(String),
}

/// One joined viewer, as the registry holds it.
pub struct Viewer {
    /// Where to send frames.
    pub tx: mpsc::Sender<ViewerMsg>,
    /// The route this viewer is looking at.
    pub route: String,
    /// The client's join reference, needed to address a server-initiated push.
    pub join_ref: Value,
    /// The topic this viewer joined.
    pub topic: String,
}

/// Drive one websocket connection for its lifetime.
///
/// `events_tx` carries browser events to the handler half — the only writer.
/// This task never touches the database.
pub async fn run_connection(
    ws: WebSocket,
    io: WebIo,
    events_tx: mpsc::Sender<WebEvent>,
    viewers: Arc<crate::web::io::ViewerRegistry>,
) {
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ViewerMsg>(64);

    // The id this connection is known by, so it can be removed on close.
    let viewer_id = next_viewer_id();
    // (route, session id) — set at join, carried on every event.
    let mut joined: Option<(String, String)> = None;

    loop {
        tokio::select! {
            // Frames the handler (or anybody else) wants pushed at this viewer.
            Some(ViewerMsg::Frame(text)) = out_rx.recv() => {
                if sink.send(WsMessage::Text(text)).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let WsMessage::Text(text) = msg else { continue };

                let Some(frame) = frames::parse(&text) else {
                    // Not a vsn 2.0.0 tuple: close rather than guess.
                    break;
                };

                let reply = answer(
                    &frame,
                    &io,
                    &events_tx,
                    &viewers,
                    &viewer_id,
                    &out_tx,
                    &mut joined,
                )
                .await;

                if let Some(text) = reply
                    && sink.send(WsMessage::Text(text)).await.is_err()
                {
                    break;
                }
            }
        }
    }

    viewers.remove(&viewer_id).await;
}

/// Answer one frame.
#[allow(clippy::too_many_arguments)]
async fn answer(
    frame: &frames::Frame,
    io: &WebIo,
    events_tx: &mpsc::Sender<WebEvent>,
    viewers: &Arc<crate::web::io::ViewerRegistry>,
    viewer_id: &str,
    out_tx: &mpsc::Sender<ViewerMsg>,
    joined: &mut Option<(String, String)>,
) -> Option<String> {
    match (frame.topic.as_str(), frame.event.as_str()) {
        ("phoenix", "heartbeat") => Some(frames::ok_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            json!({}),
        )),

        (_, "phx_join") => {
            let expected = format!("lv:{}", session::container_id(&io.cell_path));
            if frame.topic != expected {
                return Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "this socket does not serve that container".to_string(),
                ));
            }
            let token = frame
                .payload
                .get("session")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !session::names(token, &io.cell_path) {
                // The security property the token exists for: a page's token
                // must not open somebody else's socket.
                tracing::warn!(
                    surface = %io.cell_path,
                    "join refused: the session token names another surface"
                );
                return Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the session token does not name this surface".to_string(),
                ));
            }

            let route = route_of(&frame.payload);
            let pages: Arc<PageMap> = io.pages.borrow().clone();
            let Some(page) = pages.get(&route) else {
                return Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    format!("no page declares the route {route:?}"),
                ));
            };

            // The session id is the token's nonce — the half that is unique per
            // page load. The path half says which surface it names and is
            // already checked above, so it carries no information here.
            let session_id = token.split('.').next().unwrap_or_default().to_string();
            *joined = Some((route.clone(), session_id));
            viewers
                .insert(
                    viewer_id.to_string(),
                    Viewer {
                        tx: out_tx.clone(),
                        route,
                        join_ref: frame.join_ref.clone(),
                        topic: frame.topic.clone(),
                    },
                )
                .await;

            // No render here, and that is R-W8-4b: the tree was built by the
            // last write.
            Some(frames::ok_reply(
                &frame.join_ref,
                &frame.msg_ref,
                &frame.topic,
                json!({
                    "rendered": page.packed_tree(),
                    "liveview_version": meclaw_surface::socket::LIVEVIEW_VERSION
                }),
            ))
        }

        (_, "event") => {
            if joined.is_none() {
                return Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "event before join".to_string(),
                ));
            }
            // Tasks 9 and 10 decide what an event *is* — a local `editable`
            // write or a semantic event on an out-edge. Both are the handler's
            // call, because the handler is the only writer and the only side
            // with an `OutputSink`. This half forwards and says ok.
            let name = frame
                .payload
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let value = frame.payload.get("value").cloned().unwrap_or(json!({}));
            let (route, session_id) = joined.clone().unwrap_or_default();
            let (respond, verdict) = tokio::sync::oneshot::channel();
            if events_tx
                .send(WebEvent::Browser {
                    viewer: viewer_id.to_string(),
                    route,
                    session_id,
                    name,
                    value,
                    respond,
                })
                .await
                .is_err()
            {
                return Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the cell is shutting down".to_string(),
                ));
            }

            // Operation timeout (hard rule 12): a wedged handler must not hold
            // a browser's reply open forever. The client sees a refusal it can
            // act on instead of a spinner that never resolves.
            match tokio::time::timeout(EVENT_TIMEOUT, verdict).await {
                Ok(Ok(EventReply::Ok)) => Some(frames::ok_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    json!({}),
                )),
                Ok(Ok(EventReply::Error(reason))) => Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    reason,
                )),
                Ok(Err(_)) | Err(_) => Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the cell did not answer".to_string(),
                )),
            }
        }

        // live_patch, phx_leave, allow_upload, … An empty ok keeps the
        // connection up, which is what the client expects.
        _ => Some(frames::ok_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            json!({}),
        )),
    }
}

/// The route a join payload refers to.
///
/// LiveView sends the page's absolute URL; only its path matters here. A join
/// without one is treated as the root, which is what a hand-written client
/// doing the minimum will hit.
fn route_of(payload: &Value) -> String {
    let Some(url) = payload.get("url").and_then(Value::as_str) else {
        return "/".to_string();
    };
    // Cheap path extraction: everything from the first `/` after the scheme.
    match url.find("://") {
        Some(i) => match url[i + 3..].find('/') {
            Some(j) => url[i + 3 + j..]
                .split(['?', '#'])
                .next()
                .unwrap_or("/")
                .to_string(),
            None => "/".to_string(),
        },
        None => url.split(['?', '#']).next().unwrap_or("/").to_string(),
    }
}

/// A connection id.
///
/// Not a security value and never leaves the process: it only has to tell live
/// connections apart so one can be removed from the registry when it closes. A
/// counter is enough, and it avoids pulling a uuid dependency into this crate
/// for a label nobody reads.
///
/// The atomic is deliberate and is not the forbidden shape: the substrate's
/// rule bans shared mutable state in a **cell or colony actor**, and this is a
/// process-local id source in the I/O half, touched once per connection.
fn next_viewer_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("v{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_is_taken_from_the_join_url() {
        assert_eq!(route_of(&json!({"url": "http://h:7800/demo"})), "/demo");
        assert_eq!(route_of(&json!({"url": "https://h/a/b?x=1"})), "/a/b");
        assert_eq!(route_of(&json!({"url": "http://h:7800"})), "/");
        assert_eq!(route_of(&json!({})), "/");
    }
}
