//! GH #643 — a topic on a page's own socket is a door into another cell.
//!
//! The socket a `web` cell serves carries one `lv:` topic for the page. This
//! file is about a second kind of topic on the same socket: `voice:<call>`, which
//! the loop does not interpret at all. It opens a link on the mount the join
//! names, forwards text one way as `frame`, binary the other way as an `audio`
//! broadcast, and repeats a close with its code. What the frames MEAN is the
//! voice cell's business, and nothing here knows any of it.
//!
//! The cell behind the mount is a fake that echoes, and that is the point: it
//! makes every assertion about the LOOP. A real voice cell would answer with
//! transcripts and speech, which would prove the recogniser rather than the
//! door — `gh643_audio_in_the_display_window.rs` is where that is measured.
//!
//! The five arms, in the order they are made:
//!
//! (a) a join opens a link and answers `ok {}`; `hello` follows as the first
//!     `frame` push, so a client has one handler for every frame (O-639-3);
//! (b) a text `frame` push is forwarded and replied to (O-639-4);
//! (c) a binary push is forwarded with no reply at all, and the way back is a
//!     broadcast the vendored client decodes;
//! (d) a close from the cell arrives as `close` with its code, then `phx_close`;
//! (e) a join naming a mount nothing holds is refused with the mount in the
//!     sentence — and the page's own topic is untouched by all of it.
//!
//! Three more, each about what happens when something ENDS:
//!
//! (f) a `phx_leave` reaches the cell as a disconnect, and the topic is gone;
//! (g) a closed socket reaches every cell it was talking to;
//! (h) a topic the cell closed can be joined again -- which is what a Phoenix
//!     client does after a `phx_close`, and what used to be refused for the life
//!     of the socket.
//!
//! And one about what happens when nothing ends: (i) traffic in both directions
//! at once leaves the loop turning. The deadlock that shape can reach is pinned
//! as a unit test beside `hand_over`, where it can be stated without depending on
//! a kernel buffer size -- see the note on that test.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::surfaces::{BoxFuture, LINK_QUEUE};
use meclaw_colony::{
    CellFactory, ContractView, Link, LinkFrame, LinkOpener, LinkRefused, LinkRequest,
    SpawnedCellKind, SurfaceEntry, SurfaceRegistry,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
use meclaw_testing::{surface_listener, wait_for_mount};

/// The name the display answers to. The port in the URLs below is the
/// LISTENER's: a `web` cell has none since `web@2.0.0`.
const MOUNT: &str = "screen";
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// A cell behind a mount that says `hello` and then mirrors everything.
///
/// One special case, and it is the only interpretation in this file: the text
/// `{"type":"bye"}` makes it close with `4409`, so the close path has something
/// to close.
///
/// It also SAYS what it saw. `watch` gets one line per session that ended — the
/// only way for a test to prove a `phx_leave` or a closed socket reached the cell,
/// because both of them are the ABSENCE of a frame on this side. Without it an
/// empty `phx_leave` arm and a loop that forgot to drop its links would pass
/// every assertion in this file.
struct EchoOpener {
    /// One `gone <session>` per link whose client went away.
    watch: mpsc::Sender<String>,
}

impl LinkOpener for EchoOpener {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        let watch = self.watch.clone();
        Box::pin(async move {
            let session = req.session.clone().unwrap_or_default();
            if session == "refuse-me" {
                return Err(LinkRefused {
                    status: 400,
                    detail: "session must be 1..=128 characters from [A-Za-z0-9._:-]\n".to_string(),
                });
            }
            let (to_cell_tx, mut to_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            let (from_cell_tx, from_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            tokio::spawn(async move {
                let hello = json!({"type": "hello", "session_id": session.clone()}).to_string();
                if from_cell_tx.send(LinkFrame::Text(hello)).await.is_err() {
                    return;
                }
                loop {
                    // `None` is the client going away: the loop dropped this
                    // link's sender. That is the same fact a closed socket
                    // carries, and it is what this fake reports.
                    let Some(frame) = to_cell_rx.recv().await else {
                        let _ = watch.send(format!("gone {session}")).await;
                        return;
                    };
                    let back = match frame {
                        LinkFrame::Text(text) if text.contains("\"bye\"") => LinkFrame::Close {
                            code: 4409,
                            reason: "the echo said goodbye".to_string(),
                        },
                        other => other,
                    };
                    let closing = matches!(back, LinkFrame::Close { .. });
                    if from_cell_tx.send(back).await.is_err() || closing {
                        return;
                    }
                }
            });
            Ok(Link {
                to_cell: to_cell_tx,
                from_cell: from_cell_rx,
            })
        })
    }
}

/// A cell that talks without being asked, so a browser that stops reading has
/// something to be behind.
///
/// It emits `FLOOD` binary frames and only looks at what the client sent between
/// two of them — the shape a real voice connection has, one task doing both. Once
/// the page stops reading, `from_cell` fills, this task parks in its `send`, and
/// with it stops draining `to_cell`. That is the far end of the ring the socket
/// loop must not close (see `hand_over`).
struct FloodOpener;

/// Enough frames to fill the outbound queue twice over and stay parked.
const FLOOD: usize = 500;

impl LinkOpener for FloodOpener {
    fn open(&self, _req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        Box::pin(async move {
            let (to_cell_tx, mut to_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            let (from_cell_tx, from_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            tokio::spawn(async move {
                for _ in 0..FLOOD {
                    if from_cell_tx
                        .send(LinkFrame::Binary(vec![7u8; 64]))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    // Between two frames, not instead of them: a text frame the
                    // page sent is answered as a text frame, which is what the
                    // test waits for on the other side of the flood.
                    if let Ok(LinkFrame::Text(text)) = to_cell_rx.try_recv()
                        && from_cell_tx.send(LinkFrame::Text(text)).await.is_err()
                    {
                        return;
                    }
                }
                while let Some(frame) = to_cell_rx.recv().await {
                    if from_cell_tx.send(frame).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Link {
                to_cell: to_cell_tx,
                from_cell: from_cell_rx,
            })
        })
    }
}

/// One page with one route, so a join has something to answer with.
fn seed_a_page(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"home","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n"
        ),
    )
    .expect("objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"home","title":"Home"}"#,
            "\n"
        ),
    )
    .expect("pages");
}

/// A live `web` cell reading the registry this test filled.
struct Live {
    /// The port of the one listener in front of the cell.
    port: u16,
    listener: tokio::task::JoinHandle<()>,
    _sender: mpsc::Sender<meclaw_core::Message>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.join.abort();
        self.listener.abort();
    }
}

async fn start(cell_dir: &std::path::Path, surfaces: Arc<SurfaceRegistry>) -> Live {
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({ "mount": MOUNT }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active");
    };
    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let port = addr.port();
    let deadline = Instant::now() + MARKER;
    loop {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/")).await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Live {
        port,
        listener,
        _sender: sender,
        _stop: stop_tx,
        join,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn token_of(port: u16) -> String {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body.find(marker).expect("the shell carries a token") + marker.len();
    let end = start + body[start..].find('"').expect("the token is quoted");
    body[start..end].to_string()
}

async fn send(ws: &mut Ws, frame: Value) {
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("send");
}

/// What `phoenix.min.js` `binaryEncode` writes for a client push.
fn binary_push(join_ref: &str, msg_ref: &str, topic: &str, event: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![
        0u8,
        join_ref.len() as u8,
        msg_ref.len() as u8,
        topic.len() as u8,
        event.len() as u8,
    ];
    out.extend_from_slice(join_ref.as_bytes());
    out.extend_from_slice(msg_ref.as_bytes());
    out.extend_from_slice(topic.as_bytes());
    out.extend_from_slice(event.as_bytes());
    out.extend_from_slice(payload);
    out
}

async fn next_msg(ws: &mut Ws) -> WsMessage {
    tokio::time::timeout(MARKER, ws.next())
        .await
        .expect("the cell answers within the failure-marker window")
        .expect("the stream stays open")
        .expect("a frame")
}

async fn next_text(ws: &mut Ws) -> Value {
    match next_msg(ws).await {
        WsMessage::Text(t) => meclaw_core::serde_json::from_str(&t).expect("the frame is JSON"),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn next_binary(ws: &mut Ws) -> Vec<u8> {
    match next_msg(ws).await {
        WsMessage::Binary(b) => b.into(),
        other => panic!("expected a binary frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_voice_topic_rides_the_display_socket() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (watch, mut watched) = mpsc::channel::<String>(16);
    // Held for the life of the test: dropping the receiver would not unmount the
    // entry, but it is what a real cell keeps, so the fixture keeps it too.
    let (_handoff, _registration) = surfaces
        .register(
            "voice",
            SurfaceEntry {
                kind: "voice",
                cell_path: Path::new("/voice"),
                links: Some(Arc::new(EchoOpener { watch })),
            },
        )
        .await
        .expect("the mount is free");

    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_a_page(&cell_dir);
    let live = start(&cell_dir, Arc::clone(&surfaces)).await;

    let token = token_of(live.port).await;
    let page_topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
        live.port
    ))
    .await
    .expect("the cell accepts a websocket");

    send(
        &mut ws,
        json!(["1", "1", page_topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/{MOUNT}/", live.port)
        }]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));

    // (a) the join, and `hello` as the first push on the topic.
    send(
        &mut ws,
        json!(["1", "2", "voice:c1", "phx_join", {"mount": "voice", "mode": "hold"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "the join is answered: {reply}"
    );
    assert_eq!(reply[2], json!("voice:c1"));
    let first = next_text(&mut ws).await;
    assert_eq!(
        first[3],
        json!("frame"),
        "hello travels as a frame: {first}"
    );
    assert_eq!(first[2], json!("voice:c1"));
    assert_eq!(first[4]["type"], json!("hello"));
    assert_eq!(
        first[4]["session_id"],
        json!("c1"),
        "the session is the topic suffix, and nothing else names it: {first}"
    );

    // (b) a text frame is forwarded and replied to.
    send(
        &mut ws,
        json!(["1", "3", "voice:c1", "frame", {"type": "hold"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(reply[3], json!("phx_reply"));
    assert_eq!(reply[4]["status"], json!("ok"), "{reply}");
    let echoed = next_text(&mut ws).await;
    assert_eq!(echoed[3], json!("frame"));
    assert_eq!(echoed[4], json!({"type": "hold"}), "verbatim: {echoed}");

    // (c) audio goes both ways in binary, and the way back needs no reference.
    ws.send(WsMessage::Binary(
        binary_push("1", "", "voice:c1", "audio", &[1, 2, 3, 4]).into(),
    ))
    .await
    .expect("send audio");
    let back = next_binary(&mut ws).await;
    assert_eq!(
        &back[..3],
        &[2u8, 8, 5],
        "a broadcast: kind 2, then two lengths"
    );
    assert_eq!(&back[3..11], b"voice:c1");
    assert_eq!(&back[11..16], b"audio");
    assert_eq!(&back[16..], &[1, 2, 3, 4], "the audio came back unchanged");

    // (d) the cell closes, and the page learns the code before the topic ends.
    send(
        &mut ws,
        json!(["1", "4", "voice:c1", "frame", {"type": "bye"}]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    let closed = next_text(&mut ws).await;
    assert_eq!(closed[3], json!("close"), "{closed}");
    assert_eq!(closed[4]["code"], json!(4409));
    let gone = next_text(&mut ws).await;
    assert_eq!(gone[3], json!("phx_close"), "{gone}");
    assert_eq!(gone[2], json!("voice:c1"));

    // (e) a mount nothing holds, and a refusal the cell itself wrote.
    send(
        &mut ws,
        json!(["1", "5", "voice:c2", "phx_join", {"mount": "nothing"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("no surface is mounted as \"nothing\""),
        "the sentence names the mount that was asked for: {reply}"
    );
    send(
        &mut ws,
        json!(["1", "6", "voice:refuse-me", "phx_join", {}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert!(
        reply[4]["response"]["reason"]
            .as_str()
            .unwrap_or_default()
            .starts_with("session must be"),
        "the cell's own refusal travels verbatim: {reply}"
    );

    // A second join on a topic already held is refused rather than replacing it.
    send(
        &mut ws,
        json!(["1", "7", "voice:c3", "phx_join", {"mount": "voice"}]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    assert_eq!(next_text(&mut ws).await[4]["type"], json!("hello"));
    send(
        &mut ws,
        json!(["1", "8", "voice:c3", "phx_join", {"mount": "voice"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("topic already joined")
    );

    // (f) A leave reaches the cell as a disconnect, and the topic is gone.
    //     Both halves are needed: `watched` proves the cell heard it, the refused
    //     `frame` proves the entry left the socket's own table.
    send(&mut ws, json!(["1", "9", "voice:c3", "phx_leave", {}])).await;
    let left = next_text(&mut ws).await;
    assert_eq!(left[4]["status"], json!("ok"), "{left}");
    assert_eq!(
        reported_gone(&mut watched).await.as_deref(),
        Some("gone c3"),
        "dropping the sender is what the cell reads as a disconnect"
    );
    send(
        &mut ws,
        json!(["1", "10", "voice:c3", "frame", {"type": "hold"}]),
    )
    .await;
    let after = next_text(&mut ws).await;
    assert_eq!(after[4]["status"], json!("error"), "{after}");
    assert_eq!(
        after[4]["response"]["reason"],
        json!("this topic is not joined")
    );

    // (h) A topic the CELL closed can be joined again. `voice:c1` was closed with
    //     `4409` in (d); a Phoenix client answers a `phx_close` by rejoining, and
    //     until the link's own state was read that was refused for the life of the
    //     socket.
    send(
        &mut ws,
        json!(["1", "11", "voice:c1", "phx_join", {"mount": "voice"}]),
    )
    .await;
    let rejoined = next_text(&mut ws).await;
    assert_eq!(
        rejoined[4]["status"],
        json!("ok"),
        "a closed link is absent, not joined: {rejoined}"
    );
    assert_eq!(next_text(&mut ws).await[4]["type"], json!("hello"));

    // And the page's own topic never noticed any of it.
    send(&mut ws, json!(["1", "12", "phoenix", "heartbeat", {}])).await;
    let beat = next_text(&mut ws).await;
    assert_eq!(beat[4]["status"], json!("ok"), "{beat}");
    assert_eq!(beat[1], json!("12"), "a reply reuses the message ref");

    // (g) The socket ends, and every cell it was talking to hears it.
    drop(ws);
    assert_eq!(
        reported_gone(&mut watched).await.as_deref(),
        Some("gone c1"),
        "the links leave with the socket, whatever the page did last"
    );
}

/// The next session the fake reported gone, or `None` inside the marker window.
async fn reported_gone(watched: &mut mpsc::Receiver<String>) -> Option<String> {
    tokio::time::timeout(MARKER, watched.recv())
        .await
        .ok()
        .flatten()
}

/// Traffic in both directions at once, and the loop still turns.
///
/// Two hundred audio pushes go in while nothing is read and the cell talks the
/// whole time, and the frame behind them is still answered. Bounded by the
/// failure marker, so a stall is a red test rather than a hung one.
///
/// **What this does NOT do** is reproduce the deadlock of GH #643, and that was
/// measured rather than assumed: with the old blocking `to_cell.send().await` in
/// place this test is still green, because reaching the deadlock over a real
/// socket means filling the kernel's own buffers, and the frame sizes that decide
/// it are the host's rather than this file's. The cycle itself is pinned where it
/// can be stated exactly -- `web::socket::tests::a_full_link_does_not_stop_the_browser_being_written_to`,
/// which fails at the marker if the hand-over stops draining. This one is the
/// end-to-end company it keeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traffic_both_ways_at_once_leaves_the_loop_turning() {
    /// Enough inbound pushes to fill the link's queue several times over.
    const PUSHES: usize = 200;

    let surfaces = Arc::new(SurfaceRegistry::new());
    let (_handoff, _registration) = surfaces
        .register(
            "voice",
            SurfaceEntry {
                kind: "voice",
                cell_path: Path::new("/voice"),
                links: Some(Arc::new(FloodOpener)),
            },
        )
        .await
        .expect("the mount is free");

    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_a_page(&cell_dir);
    let live = start(&cell_dir, Arc::clone(&surfaces)).await;

    let token = token_of(live.port).await;
    let page_topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
        live.port
    ))
    .await
    .expect("the cell accepts a websocket");
    send(
        &mut ws,
        json!(["1", "1", page_topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/{MOUNT}/", live.port)
        }]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));

    send(
        &mut ws,
        json!(["1", "2", "voice:slow", "phx_join", {"mount": "voice"}]),
    )
    .await;
    assert_eq!(
        next_text(&mut ws).await[4]["status"],
        json!("ok"),
        "the join itself is answered"
    );

    // From here on nothing is read: the flood fills the viewer's queue behind the
    // socket while these go in.
    for _ in 0..PUSHES {
        ws.send(WsMessage::Binary(
            binary_push("1", "", "voice:slow", "audio", &[1, 2, 3, 4]).into(),
        ))
        .await
        .expect("the client can still write");
    }
    // The question, at the end of the queue the loop has to have worked through.
    send(
        &mut ws,
        json!(["1", "3", "voice:slow", "frame", {"type": "still-there"}]),
    )
    .await;

    // Reading resumes. With the ring closed, no `frame` push ever arrives and this
    // ends at the marker; with the inbound wait racing the outbound drain, the
    // flood comes through and the echo behind it.
    let deadline = Instant::now() + MARKER;
    let mut binaries = 0usize;
    let echoed = loop {
        assert!(
            Instant::now() < deadline,
            "the loop stopped: {binaries} broadcasts came through and the frame never did"
        );
        match next_msg(&mut ws).await {
            WsMessage::Binary(_) => binaries += 1,
            WsMessage::Text(t) => {
                let v: Value = meclaw_core::serde_json::from_str(&t).expect("JSON");
                if v[3] == json!("frame") && v[4]["type"] == json!("still-there") {
                    break v;
                }
            }
            _ => {}
        }
    };
    assert_eq!(echoed[2], json!("voice:slow"), "{echoed}");
    assert!(
        binaries > 0,
        "the cell was talking the whole time, which is what made the queue fill"
    );
}

/// A socket carries at most four live calls at once (GH #639).
///
/// A `voice:` join presents no token — the socket was opened by the page and
/// the door behind the mount authenticates nothing either — and every accepted
/// one starts a recognition session with a provider that bills for it. What
/// bounds that is the count, and nothing else did: the map took as many topics
/// as a client cared to name.
///
/// The three facts, in the order they are made: four joins are admitted, the
/// fifth is refused with the reason a client reads, and a topic given up makes
/// room for the next one. The last of the three is what separates a cap from a
/// lifetime budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socket_holds_at_most_four_voice_topics() {
    /// The cap, as `web::socket::MAX_VOICE_LINKS` states it.
    const CAP: usize = 4;

    let surfaces = Arc::new(SurfaceRegistry::new());
    let (watch, mut watched) = mpsc::channel::<String>(16);
    let (_handoff, _registration) = surfaces
        .register(
            "voice",
            SurfaceEntry {
                kind: "voice",
                cell_path: Path::new("/voice"),
                links: Some(Arc::new(EchoOpener { watch })),
            },
        )
        .await
        .expect("the mount is free");

    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_a_page(&cell_dir);
    let live = start(&cell_dir, Arc::clone(&surfaces)).await;

    let token = token_of(live.port).await;
    let page_topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
        live.port
    ))
    .await
    .expect("the cell accepts a websocket");
    send(
        &mut ws,
        json!(["1", "1", page_topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/{MOUNT}/", live.port)
        }]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));

    // The cap is a cap, not a ration: every join up to it is an ordinary one.
    for call in 1..=CAP {
        send(
            &mut ws,
            json!(["1", format!("j{call}"), format!("voice:c{call}"), "phx_join", {"mount": "voice"}]),
        )
        .await;
        let reply = next_text(&mut ws).await;
        assert_eq!(
            reply[4]["status"],
            json!("ok"),
            "call {call} is inside the cap: {reply}"
        );
        assert_eq!(next_text(&mut ws).await[4]["type"], json!("hello"));
    }

    // The one over it, and the sentence a client reads for it.
    send(
        &mut ws,
        json!(["1", "over", "voice:c5", "phx_join", {"mount": "voice"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("too many voice topics on this socket"),
        "{reply}"
    );

    // A topic given up is a slot given back, and the cell hears the disconnect.
    send(&mut ws, json!(["1", "leave", "voice:c1", "phx_leave", {}])).await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    assert_eq!(
        reported_gone(&mut watched).await.as_deref(),
        Some("gone c1"),
        "the leave reached the cell"
    );
    send(
        &mut ws,
        json!(["1", "again", "voice:c5", "phx_join", {"mount": "voice"}]),
    )
    .await;
    let reply = next_text(&mut ws).await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "the freed slot is the next call's: {reply}"
    );
    assert_eq!(next_text(&mut ws).await[4]["type"], json!("hello"));
}
