//! W8 (GH #380): the LiveView socket, served by the cell itself.
//!
//! # Why the cell has its own loop
//!
//! The api-side connection answered a join by asking a cell over its
//! `Dispatcher` — message out, HTML back. A `web` cell **is** the thing that
//! would be asked, and it already holds the answer: the page was materialised
//! before the request arrived. Reusing that type would have meant inventing a
//! dispatcher pointing at ourselves. So the wire format moved to
//! `meclaw_surface::frames` (shared, tested once) and this is the cell's own
//! thin loop over it. That other loop is gone since GH #396 — it never had a
//! consumer — and this one is what it always was: the only one.
//!
//! R-W8-4b lands here: **a join does no diff work**. It answers from
//! [`Materialized::packed_tree`], which was built by a write, not by this read.
//!
//! # A topic that is not this page (GH #643)
//!
//! A topic whose name starts with `voice:` is not a page and is not answered
//! here at all. The loop asks the process's mount table for a link, forwards
//! text one way and binary the other, and repeats a close with its code. It
//! interprets none of it: a frame this loop does not understand is a frame the
//! cell behind the mount answers, which is why a wrong frame produces the
//! **cell's** refusal rather than one invented on the way. That is what makes a
//! second surface reachable in the window a person is already looking at,
//! without a second port and without a second listener.
//!
//! # Which page a socket belongs to
//!
//! One cell, one container id — it is derived from the cell path — so the topic
//! alone cannot say which of several routes a viewer is looking at. The
//! LiveView client sends the page's URL in the join payload, and that is what
//! decides. A viewer is then registered under that route, and a write to it
//! reaches exactly the viewers of that page (Task 7).

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::{LinkFrame, LinkRequest};
use meclaw_core::serde_json::{Value, json};
use meclaw_surface::{frames, session};
use std::collections::HashMap;
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
    /// A binary frame, already encoded (GH #643).
    ///
    /// The one thing a page is sent that is not text: audio, as the v2
    /// serializer's broadcast. It travels through the same queue as every other
    /// frame, so a browser that stops reading holds up its own audio and nothing
    /// else — and the cell behind the link counts it out by the same rule its own
    /// socket would.
    Binary(Vec<u8>),
    /// End this connection (GH #410).
    ///
    /// Sent when the listener moves to another address. The socket was accepted
    /// on the old one and cannot follow it; the client's own reconnect is what
    /// brings the viewer back, against the address its page now resolves to. A
    /// close frame goes out first so the browser learns the connection ended
    /// rather than waiting for its heartbeat to time out.
    Close,
}

/// The topic prefix a link rides on. Everything after it is the call.
const VOICE_TOPIC: &str = "voice:";

/// The mount a `voice:` join reaches when it names none.
const DEFAULT_VOICE_MOUNT: &str = "voice";

/// How many live `voice:` links one socket may hold.
///
/// A `voice:` join presents no session token, by ruling: the socket was opened
/// by the page, and the door behind the mount has no authentication of its own
/// either (R-W8-2 on both). What the ruling never sized is the COUNT. Every
/// accepted join opens a link, and a link starts a recognition session with a
/// provider that bills for it — so an uncapped map is an unbounded number of
/// paid sessions per socket. Four is what a screen can plausibly be having at
/// once, and a page that wants a fifth call gives one up first.
const MAX_VOICE_LINKS: usize = 4;

/// One `voice:` topic this socket holds.
struct TopicLink {
    /// The client's frames on their way to the cell.
    to_cell: mpsc::Sender<LinkFrame>,
    /// The task that writes the cell's frames back onto this socket.
    forwarder: tokio::task::AbortHandle,
}

/// Write one link's frames onto the socket, until the link or the socket ends.
///
/// It writes into the viewer's existing queue with `send().await`, which is the
/// whole backpressure story: a browser that stops reading blocks this task, the
/// link's own channel fills behind it, and the cell on the other side gives the
/// client up by the same count it would on a socket of its own.
async fn forward(
    mut from_cell: mpsc::Receiver<LinkFrame>,
    out_tx: mpsc::Sender<ViewerMsg>,
    join_ref: Value,
    topic: String,
) {
    while let Some(frame) = from_cell.recv().await {
        let out = match frame {
            LinkFrame::Text(text) => {
                // Parsed so the payload arrives as an object rather than as a
                // string a client would have to parse a second time. A frame
                // that is not JSON is not this loop's to repair, so it travels
                // under a key that says what it is.
                let payload = meclaw_core::serde_json::from_str::<Value>(&text)
                    .unwrap_or_else(|_| json!({"raw": text}));
                ViewerMsg::Frame(frames::push(&join_ref, &topic, "frame", payload))
            }
            LinkFrame::Binary(bytes) => {
                let encoded = frames::binary_broadcast(&topic, "audio", &bytes);
                if encoded.is_empty() {
                    // A topic or event too long for a single length byte. The
                    // codec says so by returning nothing, and nothing is sent.
                    continue;
                }
                ViewerMsg::Binary(encoded)
            }
            LinkFrame::Close { code, reason } => {
                // The code first, then the channel: a client that only sees
                // `phx_close` learns that the topic ended but not why.
                let told = frames::push(
                    &join_ref,
                    &topic,
                    "close",
                    json!({"code": code, "reason": reason}),
                );
                if out_tx.send(ViewerMsg::Frame(told)).await.is_ok() {
                    let ended = frames::push(&join_ref, &topic, "phx_close", json!({}));
                    let _ = out_tx.send(ViewerMsg::Frame(ended)).await;
                }
                return;
            }
        };
        if out_tx.send(out).await.is_err() {
            return;
        }
    }
}

/// The sending half of this socket. Named because the hand-over below needs it.
type SocketSink = SplitSink<WebSocket, WsMessage>;

/// Why a frame did not reach the cell it was addressed to.
enum HandOff {
    /// The link is over: the cell behind the mount stopped taking frames.
    LinkGone,
    /// The socket is over. Nothing more is answered on it.
    SocketGone,
}

/// Hand one frame to a link, draining this socket's OTHER direction while waiting.
///
/// # The cycle this exists to break (GH #643)
///
/// A plain `to_cell.send(frame).await` inside the `select!` looks like ordinary
/// backpressure and is a deadlock. While that await is pending, `select!` does not
/// poll `out_rx`, so nothing the cell writes reaches the browser — and the chain
/// closes on itself: a slow browser fills `out_tx`, the forwarder blocks writing
/// into it, the link's `from_cell` fills behind the forwarder, the cell's own
/// connection blocks writing into `from_cell` and therefore stops reading
/// `to_cell`, `to_cell` fills, and this send parks for ever. Nothing in that ring
/// has a timeout, so it does not resolve itself, and the page's own `lv:` topic
/// dies with the call: the heartbeat is answered by the same loop.
///
/// So the inbound frame waits on a **permit** instead of on a send, and the wait
/// races the outbound drain. `reserve()` is cancel-safe, so losing that race
/// costs nothing and the next pass asks again; every pass the outbound wins moves
/// one frame to the browser, which is exactly what unblocks the far end of the
/// ring. Backpressure is kept — a page whose cell is behind still waits — but it
/// is now a wait that can end.
/// Generic over the sink so the property above can be measured without a socket:
/// „no permit available, an outbound backlog waiting" IS the cycle, and a test
/// that has to build a real WebSocket to reach it would be measuring TCP buffer
/// sizes instead. [`SocketSink`] is the one production instantiation.
async fn hand_over<S>(
    to_cell: &mpsc::Sender<LinkFrame>,
    frame: LinkFrame,
    out_rx: &mut mpsc::Receiver<ViewerMsg>,
    sink: &mut S,
) -> Result<(), HandOff>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    let permit = loop {
        tokio::select! {
            permit = to_cell.reserve() => break permit,
            out = out_rx.recv() => match out {
                Some(ViewerMsg::Frame(text)) => {
                    if sink.send(WsMessage::Text(text)).await.is_err() {
                        return Err(HandOff::SocketGone);
                    }
                }
                Some(ViewerMsg::Binary(bytes)) => {
                    if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                        return Err(HandOff::SocketGone);
                    }
                }
                Some(ViewerMsg::Close) | None => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    return Err(HandOff::SocketGone);
                }
            },
        }
    };
    match permit {
        Ok(permit) => {
            permit.send(frame);
            Ok(())
        }
        Err(_) => Err(HandOff::LinkGone),
    }
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

/// Resolve when the cell's I/O half is gone, or never when there is none.
///
/// `changed()` errors when the last sender drops, and that drop is the whole
/// signal — nobody ever sends on this channel.
async fn closed(shutdown: &mut Option<tokio::sync::watch::Receiver<()>>) {
    match shutdown {
        Some(rx) => {
            let _ = rx.changed().await;
        }
        None => std::future::pending().await,
    }
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
    base: String,
    user_id: Option<String>,
) {
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ViewerMsg>(64);
    // The cell's own way out. An upgraded socket runs on a task axum spawned,
    // which nothing in the I/O half holds a handle to, so this is how it hears
    // that the half is gone — see [`crate::web::io::WebIo::shutdown`].
    let mut shutdown = io.shutdown.clone();

    // The id this connection is known by, so it can be removed on close.
    let viewer_id = next_viewer_id();
    // (route, session id) — set at join, carried on every event.
    let mut joined: Option<(String, String)> = None;
    // The `voice:` topics this socket holds, by topic (GH #643).
    let mut links: HashMap<String, TopicLink> = HashMap::new();

    loop {
        tokio::select! {
            // The cell is going away. The close frame is the point: a client
            // that reads one reconnects, and reconnecting is how a wall screen
            // reaches the NEXT life of this cell without a person touching it.
            _ = closed(&mut shutdown) => {
                let _ = sink.send(WsMessage::Close(None)).await;
                break;
            }
            // Frames the handler (or anybody else) wants pushed at this viewer.
            //
            // Matched inside the arm rather than in the pattern: a pattern that
            // only names `Frame` would consume a `Close` and disable the branch
            // for the rest of this `select!`, which is silently dropping the one
            // message that must not be dropped.
            out = out_rx.recv() => {
                match out {
                    Some(ViewerMsg::Frame(text)) => {
                        if sink.send(WsMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Some(ViewerMsg::Binary(bytes)) => {
                        if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    // The listener moved (GH #410), or the sender is gone.
                    Some(ViewerMsg::Close) | None => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                }
            }
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    WsMessage::Text(text) => text,
                    // Audio from the page (GH #643). It is never replied to:
                    // these arrive every 20 ms and a reply would arm a timer per
                    // frame (O-639-4). It waits for room on the link when the cell
                    // is behind, and that wait drains the other direction while it
                    // lasts — see [`hand_over`].
                    WsMessage::Binary(bytes) => {
                        let Some(binary) = frames::parse_binary(&bytes) else {
                            continue;
                        };
                        if binary.event != "audio" {
                            continue;
                        }
                        let Some(to_cell) = links.get(&binary.topic).map(|l| l.to_cell.clone())
                        else {
                            continue;
                        };
                        match hand_over(
                            &to_cell,
                            LinkFrame::Binary(binary.payload),
                            &mut out_rx,
                            &mut sink,
                        )
                        .await
                        {
                            Ok(()) => {}
                            // The call is over. The entry goes with it, so a
                            // rejoin of the same topic is a join and not a
                            // refusal, and the page learns the reason from the
                            // `close` the forwarder already wrote.
                            Err(HandOff::LinkGone) => {
                                if let Some(gone) = links.remove(&binary.topic) {
                                    gone.forwarder.abort();
                                }
                                tracing::debug!(
                                    topic = %binary.topic,
                                    "web: audio for a link that ended"
                                );
                            }
                            Err(HandOff::SocketGone) => break,
                        }
                        continue;
                    }
                    _ => continue,
                };

                let Some(frame) = frames::parse(&text) else {
                    // Not a vsn 2.0.0 tuple: close rather than guess.
                    break;
                };

                let answered = answer(
                    &frame,
                    &io,
                    &events_tx,
                    &viewers,
                    &viewer_id,
                    &out_tx,
                    &mut joined,
                    &mut links,
                    &mut out_rx,
                    &mut sink,
                    &base,
                    user_id.as_deref(),
                )
                .await;

                let Ok(reply) = answered else { break };
                if let Some(text) = reply
                    && sink.send(WsMessage::Text(text)).await.is_err()
                {
                    break;
                }
            }
        }
    }

    // Every link goes with the socket: the forwarders have nowhere left to
    // write, and dropping the senders is what the cells read as a disconnect.
    for (_, link) in links.drain() {
        link.forwarder.abort();
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
    links: &mut HashMap<String, TopicLink>,
    out_rx: &mut mpsc::Receiver<ViewerMsg>,
    sink: &mut SocketSink,
    base: &str,
    user_id: Option<&str>,
) -> Result<Option<String>, HandOff> {
    let ok = |response| {
        Ok(Some(frames::ok_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            response,
        )))
    };
    let refuse = |reason: String| {
        Ok(Some(frames::error_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            reason,
        )))
    };
    match (frame.topic.as_str(), frame.event.as_str()) {
        ("phoenix", "heartbeat") => ok(json!({})),

        // A join on a `voice:` topic opens a link on the mount it names. No
        // session token is asked for: the socket was opened by the page, and the
        // door behind the mount has no authentication of its own either
        // (R-W8-2 holds on both).
        (topic, "phx_join") if topic.starts_with(VOICE_TOPIC) => {
            // A link the cell already closed is ABSENT, not joined. The forwarder
            // writes `close` and `phx_close` and ends, and it has no way to reach
            // this map — so the entry outlives the call it named, and a client
            // doing the correct Phoenix thing after a `phx_close` (rejoin the same
            // topic) was refused for the life of the socket.
            if let Some(held) = links.get(topic) {
                if held.to_cell.is_closed() {
                    if let Some(over) = links.remove(topic) {
                        over.forwarder.abort();
                    }
                } else {
                    return refuse("topic already joined".to_string());
                }
            }
            // The count is the whole guard (GH #639). A link the cell already
            // closed does not count: it is absent, which is the same reading the
            // rejoin path above takes of it.
            let live = links
                .values()
                .filter(|held| !held.to_cell.is_closed())
                .count();
            if live >= MAX_VOICE_LINKS {
                return refuse("too many voice topics on this socket".to_string());
            }
            let mount = frame
                .payload
                .get("mount")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_VOICE_MOUNT)
                .to_string();
            let request = LinkRequest {
                // The call is the topic suffix and nothing else names it, so a
                // page cannot join one topic and speak for another.
                session: Some(topic[VOICE_TOPIC.len()..].to_string()),
                mode: frame
                    .payload
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sample_rate: frame
                    .payload
                    .get("sample_rate")
                    .and_then(Value::as_u64)
                    .and_then(|r| u32::try_from(r).ok()),
                encoding: frame
                    .payload
                    .get("encoding")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
            match io.surfaces.open_link(&mount, request).await {
                None => refuse(format!("no surface is mounted as {mount:?}")),
                // The refusal the cell wrote, verbatim: a page reads the sentence
                // a `curl` against the cell's own door would read.
                Some(Err(refused)) => refuse(refused.detail),
                Some(Ok(link)) => {
                    let forwarder = tokio::spawn(forward(
                        link.from_cell,
                        out_tx.clone(),
                        frame.join_ref.clone(),
                        topic.to_string(),
                    ));
                    links.insert(
                        topic.to_string(),
                        TopicLink {
                            to_cell: link.to_cell,
                            forwarder: forwarder.abort_handle(),
                        },
                    );
                    ok(json!({}))
                }
            }
        }

        // One client frame, as JSON, on its way to the cell. Replied to so the
        // client's own timeout never fires for a frame that arrived (O-639-4).
        (topic, "frame") if topic.starts_with(VOICE_TOPIC) => {
            let Some(to_cell) = links.get(topic).map(|l| l.to_cell.clone()) else {
                return refuse("this topic is not joined".to_string());
            };
            // Same hand-over as the audio path, and for the same reason: a text
            // frame must not be able to hold up the direction that relieves it.
            match hand_over(
                &to_cell,
                LinkFrame::Text(frame.payload.to_string()),
                out_rx,
                sink,
            )
            .await
            {
                Ok(()) => ok(json!({})),
                Err(HandOff::LinkGone) => {
                    if let Some(gone) = links.remove(topic) {
                        gone.forwarder.abort();
                    }
                    refuse("this call is over".to_string())
                }
                Err(HandOff::SocketGone) => Err(HandOff::SocketGone),
            }
        }

        // The page is done with the topic. Dropping the sender is what the cell
        // reads as a disconnect, which is the same fact a closed socket carries.
        (topic, "phx_leave") if topic.starts_with(VOICE_TOPIC) => {
            if let Some(link) = links.remove(topic) {
                link.forwarder.abort();
            }
            ok(json!({}))
        }

        (_, "phx_join") => {
            let expected = format!("lv:{}", session::container_id(&io.cell_path));
            if frame.topic != expected {
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "this socket does not serve that container".to_string(),
                )));
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
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the session token does not name this surface".to_string(),
                )));
            }

            let route = route_of(&frame.payload, base);
            let pages: Arc<PageMap> = io.pages.borrow().clone();
            let Some(page) = pages.get(&route) else {
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    format!("no page declares the route {route:?}"),
                )));
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
            Ok(Some(frames::ok_reply(
                &frame.join_ref,
                &frame.msg_ref,
                &frame.topic,
                json!({
                    "rendered": page.packed_tree(),
                    "liveview_version": meclaw_surface::LIVEVIEW_VERSION
                }),
            )))
        }

        (_, "event") => {
            if joined.is_none() {
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "event before join".to_string(),
                )));
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
                    user_id: user_id.map(str::to_string),
                    name,
                    value,
                    respond,
                })
                .await
                .is_err()
            {
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the cell is shutting down".to_string(),
                )));
            }

            // Operation timeout (hard rule 12): a wedged handler must not hold
            // a browser's reply open forever. The client sees a refusal it can
            // act on instead of a spinner that never resolves.
            match tokio::time::timeout(EVENT_TIMEOUT, verdict).await {
                Ok(Ok(EventReply::Ok)) => Ok(Some(frames::ok_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    json!({}),
                ))),
                Ok(Ok(EventReply::Error(reason))) => Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    reason,
                ))),
                Ok(Err(_)) | Err(_) => Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    "the cell did not answer".to_string(),
                ))),
            }
        }

        // live_patch, phx_leave, allow_upload, … An empty ok keeps the
        // connection up, which is what the client expects.
        _ => Ok(Some(frames::ok_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            json!({}),
        ))),
    }
}

/// The route a join payload refers to, under `base`.
///
/// LiveView sends the page's absolute URL; only its path matters here. That
/// path is what the **browser** sees, so it carries the mount this display is
/// reached under and whatever prefix a proxy in front stripped — `base` is
/// exactly those two ([`crate::web::io::base_of`]), and taking it off is what
/// leaves the route the `pages` table declares (`/`, `/a/b`).
///
/// A join without a URL is treated as the root, which is what a hand-written
/// client doing the minimum will hit. A path that does not start with `base` is
/// left alone: it is either a client that made the URL up, and it will find no
/// page under it, or a proxy setup this cell was told nothing about — and
/// silently cutting a prefix off such a path would answer the wrong page.
fn route_of(payload: &Value, base: &str) -> String {
    let Some(url) = payload.get("url").and_then(Value::as_str) else {
        return "/".to_string();
    };
    // Cheap path extraction: everything from the first `/` after the scheme.
    let path = match url.find("://") {
        Some(i) => match url[i + 3..].find('/') {
            Some(j) => url[i + 3 + j..].split(['?', '#']).next().unwrap_or("/"),
            None => "/",
        },
        None => url.split(['?', '#']).next().unwrap_or("/"),
    };
    match path.strip_prefix(base) {
        // `/egon/screen` itself is the display's root, and `/egon/screen/a` is
        // its `/a`. A base that only matches as a string prefix of a longer
        // segment (`/egon/screenshot`) is not this display's and stays whole.
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => path.to_string(),
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

    /// The cycle of GH #643, measured where it lives.
    ///
    /// The state that used to deadlock is exactly this one: the link has no room
    /// (`to_cell` is full) and the browser's own queue has frames waiting. A
    /// `to_cell.send(..).await` there parks the whole `select!`, so `out_rx` is
    /// never drained, so the far end of the ring never frees room, so the send
    /// never completes — for ever, with no timeout anywhere in it.
    ///
    /// This pins the way out: the permit is only ever freed HERE by a task that
    /// waits until every queued outbound frame has reached the sink. So a
    /// `hand_over` that did not drain while waiting would never get its permit
    /// and this test would end at the failure marker instead of passing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_full_link_does_not_stop_the_browser_being_written_to() {
        /// The 30 s convention: a wedge is a red test, not a hung one.
        const MARKER: std::time::Duration = std::time::Duration::from_secs(30);
        /// Frames waiting for the browser when the inbound frame arrives.
        const QUEUED: usize = 3;

        // A link with exactly one slot, already taken: no permit is available.
        let (to_cell, mut to_cell_rx) = mpsc::channel::<LinkFrame>(1);
        to_cell
            .send(LinkFrame::Text("first".into()))
            .await
            .expect("the one slot");

        // The browser's queue, with a backlog in it.
        let (out_tx, mut out_rx) = mpsc::channel::<ViewerMsg>(QUEUED);
        for i in 0..QUEUED {
            out_tx
                .send(ViewerMsg::Frame(format!("outbound {i}")))
                .await
                .expect("room");
        }

        // A sink that says what it was handed.
        let (wrote_tx, mut wrote_rx) = mpsc::channel::<WsMessage>(QUEUED + 1);
        let mut sink = Box::pin(futures_util::sink::unfold(
            wrote_tx,
            |tx, msg: WsMessage| async move {
                tx.send(msg).await.map_err(|_| ())?;
                Ok::<_, ()>(tx)
            },
        ));

        // The permit appears only once the backlog is gone. This is the whole
        // discriminator: a hand-over that blocks instead of draining never gets
        // here.
        let freeing = tokio::spawn(async move {
            for _ in 0..QUEUED {
                wrote_rx.recv().await?;
            }
            // One frame off the link, which frees the slot.
            to_cell_rx.recv().await;
            Some(to_cell_rx)
        });

        let handed = tokio::time::timeout(
            MARKER,
            hand_over(
                &to_cell,
                LinkFrame::Text("inbound".into()),
                &mut out_rx,
                &mut sink,
            ),
        )
        .await
        .expect("a full link must not be able to stop the outbound drain");
        assert!(
            handed.is_ok(),
            "the frame reached the link once there was room"
        );

        let mut rest = freeing
            .await
            .expect("no panic")
            .expect("every queued frame reached the browser");
        assert_eq!(
            rest.recv().await,
            Some(LinkFrame::Text("inbound".into())),
            "and the inbound frame is what the link holds now"
        );
    }

    #[test]
    fn a_route_is_taken_from_the_join_url_under_the_base() {
        // The base a display serves under is `/<mount>`, and the browser's URL
        // carries it: the page at the display's own root is `/screen`.
        assert_eq!(
            route_of(&json!({"url": "http://h:7800/screen/demo"}), "/screen"),
            "/demo"
        );
        assert_eq!(
            route_of(&json!({"url": "https://h/screen"}), "/screen"),
            "/"
        );
        assert_eq!(
            route_of(&json!({"url": "https://h/screen/a/b?x=1"}), "/screen"),
            "/a/b"
        );
        // With a proxy in front the base carries its prefix too, and the same
        // page resolves to the same route.
        assert_eq!(
            route_of(&json!({"url": "https://h/egon/screen/a/b"}), "/egon/screen"),
            "/a/b"
        );
        assert_eq!(
            route_of(&json!({"url": "https://h/egon/screen"}), "/egon/screen"),
            "/"
        );
        // A path that is not this display's is left whole: it will find no
        // page, which is the honest answer to a URL nobody served.
        assert_eq!(
            route_of(&json!({"url": "https://h/screenshot/a"}), "/screen"),
            "/screenshot/a"
        );
        assert_eq!(route_of(&json!({"url": "http://h:7800"}), "/screen"), "/");
        assert_eq!(route_of(&json!({}), "/screen"), "/");
    }
}
