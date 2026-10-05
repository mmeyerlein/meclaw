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
//! # A topic that is not this page (GH #643, GH #766)
//!
//! A topic whose name starts with one of the prefixes in [`TOPIC_KINDS`] is not
//! a page and is not answered here at all. The loop asks the process's mount
//! table for a link, forwards text one way and binary the other, and repeats a
//! close with its code. It interprets none of it: a frame this loop does not
//! understand is a frame the cell behind the mount answers, which is why a
//! wrong frame produces the **cell's** refusal rather than one invented on the
//! way. That is what makes a second surface reachable in the window a person is
//! already looking at, without a second port and without a second listener.
//!
//! There are two prefixes since GH #766, and the second one is the reason the
//! three `starts_with` guards became a table: `voice:` carries a call, `page:`
//! carries a browser page, and everything that differs between them — which
//! mount a join reaches when it names none, which event a binary frame from the
//! cell rides on, whether the client may send binary at all, and how many of
//! them one socket may hold — is a column rather than a branch. The loop still
//! knows nothing about either: it knows that a prefix names a kind.
//!
//! Which mounts a join may reach is the one thing the loop does decide
//! (GH #869). The page names the mount, so without a list every cell with a
//! link door in the process is reachable from every display socket.
//! `params.link_mounts` is that list; empty, the default, keeps the old
//! behaviour.
//!
//! # Which page a socket belongs to
//!
//! One cell, one container id — it is derived from the cell path — so the topic
//! alone cannot say which of several routes a viewer is looking at. The
//! LiveView client sends the page's URL in the join payload, and that is what
//! decides. A viewer is then registered under that route, and a write to it
//! reaches exactly the viewers of that page (Task 7).

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::{FuturesUnordered, SplitSink};
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::{LinkFrame, LinkRequest};
use meclaw_core::serde_json::{Value, json};
use meclaw_surface::{frames, session};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::web::backlog::{Meter, Outbox, Queued};
use crate::web::cell::{EventReply, LOCAL_EVENT, WebEvent};
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

/// One kind of link topic: a prefix and everything that follows from it.
///
/// A kind is named by its prefix, and the name in an operator-facing sentence
/// is the prefix without the colon — `too many page topics on this socket`.
struct TopicKind {
    /// What a topic of this kind starts with. Everything after it is the
    /// session: the call for `voice:`, the page for `page:`.
    prefix: &'static str,
    /// The mount a join of this kind reaches when it names none.
    default_mount: &'static str,
    /// The event a binary frame FROM the cell rides on, on its way to the page.
    binary_event: &'static str,
    /// The event a binary frame FROM the client may carry, if any at all.
    ///
    /// `None` is a one-way kind: a `page:` client sends pointers and keys as
    /// text and never a picture, so a binary push on such a topic is dropped
    /// rather than forwarded. Accepting it would make the socket a way into the
    /// cell for bytes nothing on the other side knows how to read.
    client_binary_event: Option<&'static str>,
    /// How many live links of this kind one socket may hold.
    max_links: usize,
}

/// The kinds this loop carries. Closed set, read in order.
///
/// The numbers, and why they are not the same: a `voice:` join presents no
/// session token, by ruling — the socket was opened by the page, and the door
/// behind the mount has no authentication of its own either (R-W8-2 on both).
/// What the ruling never sized is the COUNT. Every accepted join opens a link,
/// and a `voice:` link starts a recognition session with a provider that bills
/// for it, so an uncapped map is an unbounded number of paid sessions per
/// socket; four is what a screen can plausibly be having at once, and a page
/// that wants a fifth call gives one up first. A `page:` link costs a screencast
/// out of a browser the member already runs, and a screen full of windows is a
/// plausible thing to look at — eight, the same number the browser cell caps its
/// pages at (OR-G6, OR-G7). The count is kept PER KIND: a screen with four calls
/// on it must still be able to open a window.
const TOPIC_KINDS: &[TopicKind] = &[
    TopicKind {
        prefix: "voice:",
        default_mount: "voice",
        binary_event: "audio",
        client_binary_event: Some("audio"),
        max_links: 4,
    },
    TopicKind {
        prefix: "page:",
        default_mount: "browser",
        binary_event: "image",
        client_binary_event: None,
        max_links: 8,
    },
];

impl TopicKind {
    /// The kind's name in a sentence an operator reads: the prefix, no colon.
    fn name(&self) -> &'static str {
        self.prefix.trim_end_matches(':')
    }
}

/// Which kind `topic` belongs to, or `None` for a topic this loop answers itself.
fn kind_of(topic: &str) -> Option<&'static TopicKind> {
    TOPIC_KINDS.iter().find(|k| topic.starts_with(k.prefix))
}

/// How many live links of `kind` this socket holds.
///
/// A link the cell already closed does not count: it is absent, which is the
/// same reading the rejoin path takes of it.
fn live_of(links: &HashMap<String, TopicLink>, kind: &TopicKind) -> usize {
    links
        .iter()
        .filter(|(topic, held)| topic.starts_with(kind.prefix) && !held.to_cell.is_closed())
        .count()
}

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
    out_tx: Outbox,
    join_ref: Value,
    topic: String,
    binary_event: &'static str,
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
                let encoded = frames::binary_broadcast(&topic, binary_event, &bytes);
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
    out_rx: &mut mpsc::Receiver<Queued>,
    meter: &Meter,
    sink: &mut S,
) -> Result<(), HandOff>
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    let permit = loop {
        tokio::select! {
            permit = to_cell.reserve() => break permit,
            out = out_rx.recv() => match out {
                Some(queued) => {
                    if !write_out(sink, queued, meter).await {
                        return Err(HandOff::SocketGone);
                    }
                }
                None => {
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

/// Write one queued frame onto the socket and count it out of the meter
/// (GH #1006).
///
/// Counted out AFTER `sink.send` returns, because that await is where a slow
/// browser shows: the frame being written is still the viewer's backlog until
/// the socket took it. `false` when the connection is over — the socket refused
/// the frame, or the frame was the close.
async fn write_out<S>(sink: &mut S, queued: Queued, meter: &Meter) -> bool
where
    S: futures_util::Sink<WsMessage> + Unpin,
{
    let Queued {
        msg,
        at,
        bytes,
        page,
    } = queued;
    meter.writing(at);
    // GH #1013: a page frame the viewer's snapshot already holds is counted
    // out and dropped here — the one place that knows, in queue order, which
    // snapshot the client holds (see [`Meter::admits`]).
    if !meter.admits(page) {
        meter.written(at, bytes);
        return true;
    }
    let open = match msg {
        ViewerMsg::Frame(text) => sink.send(WsMessage::Text(text)).await.is_ok(),
        ViewerMsg::Binary(b) => sink.send(WsMessage::Binary(b)).await.is_ok(),
        // The listener moved (GH #410).
        ViewerMsg::Close => {
            let _ = sink.send(WsMessage::Close(None)).await;
            false
        }
    };
    meter.written(at, bytes);
    open
}

/// One joined viewer, as the registry holds it.
pub struct Viewer {
    /// Where to send frames, with the meter that counts them (GH #1006).
    pub tx: Outbox,
    /// The route this viewer is looking at.
    pub route: String,
    /// The client's join reference, needed to address a server-initiated push.
    pub join_ref: Value,
    /// The topic this viewer joined.
    pub topic: String,
    /// The page load it belongs to — the nonce half of its token.
    pub session_id: String,
    /// What the client measured of its screen at load (GH #1003): `w`, `h`,
    /// `dpr`, `orientation`, `coarse` — or `null` from a client that sent none.
    /// A hint for the app's layout per screen class, never state: every viewer
    /// of a route is sent the same bytes.
    pub screen: Value,
}

/// The longest edge, in CSS pixels, a `_screen` may claim (GH #1003): wider
/// than any real screen, small enough that no app has to guard its arithmetic.
const SCREEN_EDGE_MAX: u64 = 100_000;

/// The `_screen` join param of a page, checked and normalised (GH #1003).
///
/// `boot.js` sends `{w, h, dpr, orientation, coarse}` at load: `w`/`h` the
/// viewport size in CSS pixels (`innerWidth`/`innerHeight`), `coarse` from
/// `matchMedia`. Absent is `null` (an older client, or a page shell
/// without the boot); anything else that is not exactly that shape is `null`
/// too, said once at debug level — a hint the client got wrong must never cost
/// the viewer its join.
fn screen_of(payload: &Value) -> Value {
    let Some(raw) = payload.get("params").and_then(|p| p.get("_screen")) else {
        return Value::Null;
    };
    let edge = |k: &str| {
        raw.get(k)
            .and_then(Value::as_u64)
            .filter(|n| (1..=SCREEN_EDGE_MAX).contains(n))
    };
    // Kept as the client wrote it (`3` stays `3`, `1.5` stays `1.5`), checked
    // as a number.
    let dpr = raw.get("dpr").filter(|d| {
        d.as_f64()
            .is_some_and(|d| d.is_finite() && d > 0.0 && d <= 16.0)
    });
    let orientation = raw
        .get("orientation")
        .and_then(Value::as_str)
        .filter(|o| matches!(*o, "portrait" | "landscape"));
    let coarse = raw.get("coarse").and_then(Value::as_bool);
    match (edge("w"), edge("h"), dpr, orientation, coarse) {
        (Some(w), Some(h), Some(dpr), Some(orientation), Some(coarse)) => json!({
            "w": w, "h": h, "dpr": dpr, "orientation": orientation, "coarse": coarse,
        }),
        _ => {
            tracing::debug!(screen = %raw, "web: a join's _screen param was not understood");
            Value::Null
        }
    }
}

/// Tell the app a viewer came or went (GH #1003), when it opted in.
///
/// The same lane as every browser event: the handler emits it as
/// `viewer:screen` on the out-edge. Awaited like a browser event, so a join and
/// its leave reach the app in that order.
async fn report_screen(
    viewers: &crate::web::io::ViewerRegistry,
    events_tx: &mpsc::Sender<WebEvent>,
    session_id: &str,
    route: &str,
    screen: &Value,
    joined: bool,
) {
    if !viewers.screen_events() {
        return;
    }
    let _ = events_tx
        .send(WebEvent::Screen {
            session_id: session_id.to_string(),
            route: route.to_string(),
            screen: screen.clone(),
            joined,
        })
        .await;
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

/// A reply this socket owes and only the handler can give (GH #1004): the
/// verdict of a [`LOCAL_EVENT`], already rendered as the `phx_reply` text.
type Verdict = std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>;

/// What a browser event is owed once the handler's channel took it.
enum Forwarded {
    /// The reply, now: a semantic event (its verdict is fixed at hand-over),
    /// a refusal, or a cell that is shutting down.
    Now(String),
    /// The reply, once the handler has written — waited for beside the loop,
    /// never in it.
    Later(Verdict),
}

/// Hand one `event` frame to the handler and say what its client is owed.
///
/// The hand-over itself stays an `await` on the bounded channel, in the loop:
/// that keeps one viewer's events in the order it sent them, and a full
/// channel (64) holding the loop is the intended backpressure. What does NOT
/// stay in the loop is the wait for a verdict (GH #1004). Measured on the
/// deployed pan (plan W4 § 1): with the loop parked on the handler's answer
/// it read neither the browser's next frame nor its own outbound queue, so a
/// viewer's diffs stood behind its own event — 0 frames for 2–5 s on a
/// zoom-out while the cell answered 8–10 bundles a second, and event gaps of
/// p95 317–600 ms against a 150-ms hook.
async fn forward_event(
    frame: &frames::Frame,
    events_tx: &mpsc::Sender<WebEvent>,
    viewer_id: &str,
    route: String,
    session_id: String,
    user_id: Option<&str>,
) -> Forwarded {
    let name = frame
        .payload
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let value = frame.payload.get("value").cloned().unwrap_or(json!({}));
    let local = name == LOCAL_EVENT;
    let (respond, verdict) = if local {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
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
        return Forwarded::Now(frames::error_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            "the cell is shutting down".to_string(),
        ));
    }
    let Some(verdict) = verdict else {
        // A semantic event leaves on the out-edge whatever the handler is
        // doing now; there is no verdict left to wait for.
        return Forwarded::Now(frames::ok_reply(
            &frame.join_ref,
            &frame.msg_ref,
            &frame.topic,
            json!({}),
        ));
    };
    let (join_ref, msg_ref, topic) = (
        frame.join_ref.clone(),
        frame.msg_ref.clone(),
        frame.topic.clone(),
    );
    Forwarded::Later(Box::pin(async move {
        // Operation timeout (hard rule 12), per open write: a wedged handler
        // must not hold a browser's reply open forever. The client sees a
        // refusal it can act on instead of a spinner that never resolves.
        match tokio::time::timeout(EVENT_TIMEOUT, verdict).await {
            Ok(Ok(EventReply::Ok)) => frames::ok_reply(&join_ref, &msg_ref, &topic, json!({})),
            Ok(Ok(EventReply::Error(reason))) => {
                frames::error_reply(&join_ref, &msg_ref, &topic, reason)
            }
            Ok(Err(_)) | Err(_) => frames::error_reply(
                &join_ref,
                &msg_ref,
                &topic,
                "the cell did not answer".to_string(),
            ),
        }
    }))
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
    let (out_tx, mut out_rx) = mpsc::channel::<Queued>(64);
    // GH #1006: one meter per connection, counting every frame that enters
    // this queue and leaves it onto the socket. It reports only when the cell
    // opted in (`viewer_events: ["backlog"]`); counting is always on, because
    // the `viewers` op reads it.
    let meter = Arc::new(Meter::new(viewers.policy(), Some(events_tx.clone())));
    let out_tx = Outbox::new(out_tx, Arc::clone(&meter));
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
    // Audio frames that arrived for a topic this socket does not hold (GH #697).
    let mut unrouted_binaries: u64 = 0;
    // Binary a KIND does not accept at all, which is not the same thing: a
    // `page:` topic declares no client binary, so a display that pushed one
    // used to have it vanish without a trace on either side.
    let mut refused_binaries: u64 = 0;
    // GH #1004: the open `object:set` verdicts of this socket. Polled in the
    // same `select!` rather than spawned, so the end of the connection takes
    // them with it; replies may leave in any order (the client matches `ref`).
    let mut verdicts: FuturesUnordered<Verdict> = FuturesUnordered::new();

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
                    Some(queued) => {
                        if !write_out(&mut sink, queued, &meter).await {
                            break;
                        }
                    }
                    // The sender is gone.
                    None => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                }
            }
            // A verdict the handler gave while the loop went on reading.
            Some(reply) = verdicts.next(), if !verdicts.is_empty() => {
                if sink.send(WsMessage::Text(reply)).await.is_err() {
                    break;
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
                        // The kind decides whether the client may send binary at
                        // all, and under which event. A `page:` topic declares
                        // none: pointers and keys travel as text, and a picture
                        // only ever goes the other way (OR-G25).
                        let admitted = kind_of(&binary.topic)
                            .and_then(|kind| kind.client_binary_event)
                            .is_some_and(|event| event == binary.event);
                        if !admitted {
                            refused_binaries = refused_binaries.saturating_add(1);
                            if refused_binaries == 1 {
                                tracing::warn!(
                                    topic = %binary.topic,
                                    event = %binary.event,
                                    "web: this kind of topic takes no binary from the client"
                                );
                            }
                            continue;
                        }
                        let Some(to_cell) = links.get(&binary.topic).map(|l| l.to_cell.clone())
                        else {
                            // Audio for a topic this link does not hold. It is
                            // dropped, as it always was — a rejoin in flight is
                            // the ordinary cause — but it is counted, and the
                            // first one says so: a page whose frames go nowhere
                            // used to be indistinguishable from one that sent
                            // none (GH #697).
                            unrouted_binaries = unrouted_binaries.saturating_add(1);
                            if unrouted_binaries == 1 {
                                tracing::warn!(
                                    topic = %binary.topic,
                                    "web: binary frames for a topic this socket does not hold"
                                );
                            }
                            continue;
                        };
                        match hand_over(
                            &to_cell,
                            LinkFrame::Binary(binary.payload),
                            &mut out_rx,
                            &meter,
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
                    &mut verdicts,
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
    // The count behind the one warning above, so a socket that dropped audio
    // for the whole of its life says how much when it goes (GH #697).
    if unrouted_binaries > 0 {
        tracing::info!(
            count = unrouted_binaries,
            "web: binary frames dropped for topics this socket did not hold"
        );
    }
    if refused_binaries > 0 {
        tracing::info!(
            count = refused_binaries,
            "web: binary frames refused because the topic's kind takes none"
        );
    }
    if let Some(gone) = viewers.remove(&viewer_id).await {
        report_screen(
            &viewers,
            &events_tx,
            &gone.session_id,
            &gone.route,
            &gone.screen,
            false,
        )
        .await;
    }
}

/// GH #1002: how long one join piece may take to leave: 10 s plus 1 s for
/// every 4 KB of it.
///
/// Operation timeout (hard rule 12) on the join's own writes, which go
/// straight to the socket. The rate is a floor, not a guess: Slow 3G as
/// measured is 50 KB/s, so a default 96 KB piece needs about 2 s and is given
/// 34 s; a viewer slower than 4 KB/s for a whole piece is a socket that is not
/// draining, and the join ends instead of holding this task for ever.
fn piece_timeout(bytes: usize) -> Duration {
    Duration::from_secs(10) + Duration::from_millis((bytes / 4) as u64)
}

/// Write one join frame within [`piece_timeout`]; `false` means the socket is over.
///
/// The frame bypasses the queue (see the join arm) but not the meter
/// (GH #1006): it is counted in before the write and out after it, like
/// [`write_out`], so a viewer stuck on a large join reads as behind.
async fn piece_sent(sink: &mut SocketSink, text: String, meter: &Meter) -> bool {
    let bytes = text.len() as u64;
    let at = meter.written_directly(bytes);
    let limit = piece_timeout(text.len());
    let sent = matches!(
        tokio::time::timeout(limit, sink.send(WsMessage::Text(text))).await,
        Ok(Ok(()))
    );
    meter.written(at, bytes);
    sent
}

/// Answer one frame.
#[allow(clippy::too_many_arguments)]
async fn answer(
    frame: &frames::Frame,
    io: &WebIo,
    events_tx: &mpsc::Sender<WebEvent>,
    viewers: &Arc<crate::web::io::ViewerRegistry>,
    viewer_id: &str,
    out_tx: &Outbox,
    joined: &mut Option<(String, String)>,
    links: &mut HashMap<String, TopicLink>,
    out_rx: &mut mpsc::Receiver<Queued>,
    sink: &mut SocketSink,
    verdicts: &mut FuturesUnordered<Verdict>,
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
        (topic, "phx_join") if kind_of(topic).is_some() => {
            // Unreachable through the guard, and written as a refusal rather
            // than as an `expect`: this loop serves a page, and a panic in it
            // takes the socket the page is looking through.
            let Some(kind) = kind_of(topic) else {
                return refuse("this socket does not carry that kind of topic".to_string());
            };
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
            // The count is the whole guard (GH #639), and it is per kind
            // (GH #766): a screen already holding four calls must still be able
            // to open a window.
            if live_of(links, kind) >= kind.max_links {
                return refuse(format!("too many {} topics on this socket", kind.name()));
            }
            let mount = frame
                .payload
                .get("mount")
                .and_then(Value::as_str)
                .unwrap_or(kind.default_mount)
                .to_string();
            // GH #869: the mount comes from the page, so without a list any
            // cell holding a link door in this process was one join away --
            // `phone` through a display socket, for one. An empty list is the
            // old behaviour; a list admits exactly its names, the kind's
            // default included, and says which ones it admits.
            if !io.link_mounts.is_empty() && !io.link_mounts.contains(&mount) {
                return refuse(format!(
                    "this display links no topic to {mount:?} (params.link_mounts: {})",
                    io.link_mounts.join(", ")
                ));
            }
            // Everything the payload said except the `mount` that chose the
            // door, one level deep (OR-G32). The door reads the four voice
            // fields because it always did; what it does NOT read — a viewport,
            // say — travels whole, because only the cell behind the mount knows
            // what it means.
            let params = match frame.payload.as_object() {
                Some(obj) => {
                    let mut rest = obj.clone();
                    rest.remove("mount");
                    Value::Object(rest)
                }
                None => Value::Null,
            };
            let request = LinkRequest {
                // The session is the topic suffix and nothing else names it, so
                // a page cannot join one topic and speak for another.
                session: Some(topic[kind.prefix.len()..].to_string()),
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
                params,
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
                        kind.binary_event,
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
        (topic, "frame") if kind_of(topic).is_some() => {
            let Some(to_cell) = links.get(topic).map(|l| l.to_cell.clone()) else {
                return refuse("this topic is not joined".to_string());
            };
            // Same hand-over as the audio path, and for the same reason: a text
            // frame must not be able to hold up the direction that relieves it.
            match hand_over(
                &to_cell,
                LinkFrame::Text(frame.payload.to_string()),
                out_rx,
                out_tx.meter(),
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
        (topic, "phx_leave") if kind_of(topic).is_some() => {
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
            if !io.pages.borrow().contains_key(&route) {
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    format!("no page declares the route {route:?}"),
                )));
            }

            // The session id is the token's nonce — the half that is unique per
            // page load. The path half says which surface it names and is
            // already checked above, so it carries no information here.
            let session_id = token.split('.').next().unwrap_or_default().to_string();
            out_tx.meter().joined(&route, &session_id);
            *joined = Some((route.clone(), session_id.clone()));
            // GH #1002: registered BEFORE the snapshot is taken. A write is
            // published before its diff is fanned out, so any write the snapshot
            // misses has its diff fanned out after this insert — and reaches
            // this viewer. The other order lost it: snapshot, write, fan-out to
            // a registry without this viewer, insert. The lock is
            // `gh1002_a_write_during_the_chunks_is_not_lost_or_early`.
            // GH #1013: the same order lets a diff the snapshot ALREADY holds
            // reach the viewer too (published, then this join, then its
            // fan-out). Every page frame carries the generation of the pages it
            // leads to, the join records its snapshot's generation
            // (`holds_snapshot` below), and the write loop drops a diff of that
            // generation or older (`Meter::admits`). The lock is
            // `gh1013_a_diff_older_than_the_join_does_not_reach_the_viewer`.
            let screen = screen_of(&frame.payload);
            let replaced = viewers
                .insert(
                    viewer_id.to_string(),
                    Viewer {
                        tx: out_tx.clone(),
                        route: route.clone(),
                        join_ref: frame.join_ref.clone(),
                        topic: frame.topic.clone(),
                        session_id: session_id.clone(),
                        screen: screen.clone(),
                    },
                )
                .await;
            // GH #1003: a second join on this socket ends the first one's view.
            if let Some(old) = replaced {
                report_screen(
                    viewers,
                    events_tx,
                    &old.session_id,
                    &old.route,
                    &old.screen,
                    false,
                )
                .await;
            }
            let pages: Arc<PageMap> = io.pages.borrow().clone();
            // GH #1013: what this viewer holds from now on. Every page frame
            // still queued for it is read against this generation by the write
            // loop, after the join's own frames below.
            out_tx.meter().holds_snapshot(pages.generation);
            let Some(page) = pages.get(&route) else {
                // Removed between the two looks: no page, no viewer.
                viewers.remove(viewer_id).await;
                *joined = None;
                return Ok(Some(frames::error_reply(
                    &frame.join_ref,
                    &frame.msg_ref,
                    &frame.topic,
                    format!("no page declares the route {route:?}"),
                )));
            };
            // GH #1003: the app hears which screen joined, on which route —
            // its screen class (display-hive § 6.1) — when it opted in.
            report_screen(viewers, events_tx, &session_id, &route, &screen, true).await;

            // No render here, and that is R-W8-4b: the tree was built by the
            // last write. GH #1002: a large page is cut at `join_chunk` — the
            // reply carries the first piece, the rest follow as plain diffs; any
            // other page joins in one frame as before (`join_frames`).
            let (head, pieces) = page.join_frames(io.join_chunk);
            let reply = frames::ok_reply(
                &frame.join_ref,
                &frame.msg_ref,
                &frame.topic,
                json!({
                    "rendered": head,
                    "liveview_version": meclaw_surface::LIVEVIEW_VERSION
                }),
            );
            if pieces.is_empty() {
                return Ok(Some(reply));
            }
            // Written here, straight to the socket and before this task reads
            // its queue again: every diff a write queued for this viewer since
            // the insert above lands AFTER the last piece, so a piece cut from
            // this snapshot can never overwrite a newer diff. A diff older than
            // the snapshot (or of its generation) is dropped by the write loop
            // (GH #1013, `Meter::admits`): applied to a snapshot that already
            // holds it, a keyed-list diff is not the same page.
            if !piece_sent(sink, reply, out_tx.meter()).await {
                return Err(HandOff::SocketGone);
            }
            for piece in pieces {
                let text = frames::push(&frame.join_ref, &frame.topic, "diff", piece);
                // Only a single slot larger than the limit can make a piece
                // larger than it (`cut_frames`); said once per join, with the
                // number, because it is a page worth splitting.
                if text.len() > io.join_chunk + 1024 {
                    tracing::info!(
                        route = %route,
                        bytes = text.len(),
                        limit = io.join_chunk,
                        "web: one slot is larger than the join piece limit and goes alone"
                    );
                }
                if !piece_sent(sink, text, out_tx.meter()).await {
                    return Err(HandOff::SocketGone);
                }
            }
            Ok(None)
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
            // with an `OutputSink`. This half forwards; it waits for nobody
            // (GH #1004, [`forward_event`]).
            let (route, session_id) = joined.clone().unwrap_or_default();
            match forward_event(frame, events_tx, viewer_id, route, session_id, user_id).await {
                Forwarded::Now(reply) => Ok(Some(reply)),
                Forwarded::Later(verdict) => {
                    verdicts.push(verdict);
                    Ok(None)
                }
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
        let (out_tx, mut out_rx) = mpsc::channel::<Queued>(QUEUED);
        let meter = Arc::new(Meter::silent());
        let out_tx = Outbox::new(out_tx, Arc::clone(&meter));
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
                &meter,
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

    /// An `event` frame as the client sends it.
    fn event_frame(name: &str) -> frames::Frame {
        frames::Frame {
            join_ref: json!("1"),
            msg_ref: json!("7"),
            topic: "lv:x".to_string(),
            event: "event".to_string(),
            payload: json!({"type": "click", "event": name, "value": {"n": 1}}),
        }
    }

    fn reply_status(text: &str) -> Value {
        let v: Value = meclaw_core::serde_json::from_str(text).expect("a reply is JSON");
        v[4].clone()
    }

    /// GH #1004 T6: with the handler gone, an event is refused as before —
    /// at once, for both lanes, and with the same sentence.
    #[tokio::test]
    async fn gh1004_an_event_after_shutdown_is_refused() {
        for name in ["pick", LOCAL_EVENT] {
            let (events_tx, events_rx) = mpsc::channel::<WebEvent>(4);
            drop(events_rx);
            let forwarded = forward_event(
                &event_frame(name),
                &events_tx,
                "v1",
                "/".to_string(),
                "s".to_string(),
                None,
            )
            .await;
            let Forwarded::Now(text) = forwarded else {
                panic!("{name}: a closed channel is answered at once");
            };
            let reply = reply_status(&text);
            assert_eq!(reply["status"], json!("error"), "{name}: {text}");
            assert_eq!(
                reply["response"]["reason"],
                json!("the cell is shutting down"),
                "{name}: {text}"
            );
        }
    }

    /// GH #1004 T1 at the seam: a semantic event is `ok` the moment the
    /// handler's channel took it — nobody reads the channel here — and the
    /// handler is handed no reply to give.
    #[tokio::test]
    async fn gh1004_a_semantic_event_is_answered_at_hand_over() {
        let (events_tx, mut events_rx) = mpsc::channel::<WebEvent>(4);
        let forwarded = forward_event(
            &event_frame("pick"),
            &events_tx,
            "v1",
            "/".to_string(),
            "s".to_string(),
            None,
        )
        .await;
        let Forwarded::Now(text) = forwarded else {
            panic!("a semantic event waits for no verdict");
        };
        assert_eq!(reply_status(&text)["status"], json!("ok"), "{text}");
        let Some(WebEvent::Browser { name, respond, .. }) = events_rx.recv().await else {
            panic!("the event reached the handler's channel");
        };
        assert_eq!(name, "pick");
        assert!(respond.is_none(), "nobody is waiting for this verdict");
    }

    /// GH #1004 T5 at the seam: `object:set` still owes the handler's verdict,
    /// and the reply carries it — ok or the handler's refusal.
    #[tokio::test]
    async fn gh1004_an_object_set_waits_for_the_handlers_verdict() {
        for (verdict, status) in [
            (EventReply::Ok, "ok"),
            (EventReply::Error("not_editable".to_string()), "error"),
        ] {
            let (events_tx, mut events_rx) = mpsc::channel::<WebEvent>(4);
            let forwarded = forward_event(
                &event_frame(LOCAL_EVENT),
                &events_tx,
                "v1",
                "/".to_string(),
                "s".to_string(),
                None,
            )
            .await;
            let Forwarded::Later(pending) = forwarded else {
                panic!("object:set is answered by the handler");
            };
            let Some(WebEvent::Browser {
                respond: Some(respond),
                ..
            }) = events_rx.recv().await
            else {
                panic!("the handler is handed the reply to give");
            };
            respond.send(verdict).expect("the socket is waiting");
            let text = pending.await;
            assert_eq!(reply_status(&text)["status"], json!(status), "{text}");
        }
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
