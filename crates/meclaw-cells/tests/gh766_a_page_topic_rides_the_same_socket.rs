//! GH #766 (wave G, T2) — a `page:` topic rides the same socket a `voice:` one does.
//!
//! GH #643 put one foreign topic on the display's socket and wrote three
//! `starts_with("voice:")` guards to carry it. This file is about the second
//! one: `page:<page>`, the picture of a browser page. What it measures is not
//! the browser — there is none here, the mount is a fake — but that the LOOP
//! learned a table instead of a second branch: which mount a join reaches when
//! it names none, which event a binary frame rides on, whether the client may
//! send binary at all, and how many links of the kind one socket holds are
//! columns now, and the loop reads them.
//!
//! Seven arms, in the order they are made:
//!
//! (a) a `page:` join reaches the mount `browser` without naming it, and the
//!     join payload reaches the cell one level deep, minus the `mount` that
//!     chose the door (OR-G32);
//! (b) a binary frame from the cell arrives as `image`, not `audio`;
//! (c) a text frame from the client reaches the cell;
//! (d) a binary frame FROM the client on a `page:` topic is dropped — a page
//!     sends pointers and keys, never a picture (OR-G25);
//! (g) eight pages are eight pages and cost no call;
//! (f) the fifth call is still refused with the sentence calls have;
//! (e) the ninth page is refused with the sentence pages have.
//!
//! (g) sits before (f) and (e) on purpose: the whole point of the per-kind
//!     count is that a screen already full of windows can still take a call.

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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The name the display answers to.
const MOUNT: &str = "screen";

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// A mount that says what request it was opened with, then mirrors.
///
/// Its first frame is the whole of arm (a): the session the door derived from
/// the topic and the params it did not read, as the cell sees them. After that
/// it echoes text, and the one text it interprets — `{"type":"shoot"}` — makes
/// it send a binary frame, so arm (b) has a picture to look at.
struct Fake;

impl LinkOpener for Fake {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        Box::pin(async move {
            let session = req.session.clone().unwrap_or_default();
            let params = req.params.clone();
            let (to_cell_tx, mut to_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            let (from_cell_tx, from_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            tokio::spawn(async move {
                let told = json!({"type": "joined", "session": session, "params": params});
                if from_cell_tx
                    .send(LinkFrame::Text(told.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                while let Some(frame) = to_cell_rx.recv().await {
                    let back = match frame {
                        LinkFrame::Text(text) if text.contains("\"shoot\"") => {
                            LinkFrame::Binary(vec![0xFF, 0xD8, 0xFF, 0xE0])
                        }
                        other => other,
                    };
                    if from_cell_tx.send(back).await.is_err() {
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

/// A live `web` cell in front of one listener.
struct Live {
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

/// Join `topic` and return the reply frame.
async fn join(ws: &mut Ws, msg_ref: &str, topic: &str, payload: Value) -> Value {
    send(ws, json!(["1", msg_ref, topic, "phx_join", payload])).await;
    next_text(ws).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_topic_rides_the_same_socket() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    // Held for the life of the test: a real cell keeps both, so the fixture
    // keeps them too.
    let mut _held = Vec::new();
    for (mount, kind) in [("browser", "browser"), ("voice", "voice")] {
        _held.push(
            surfaces
                .register(
                    mount,
                    SurfaceEntry {
                        kind,
                        cell_path: Path::new(&format!("/{mount}")),
                        links: Some(Arc::new(Fake)),
                    },
                )
                .await
                .expect("the mount is free"),
        );
    }

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

    // (a) the join names no mount and reaches `browser` anyway, and what it
    //     said beyond the mount arrives one level deep.
    let reply = join(
        &mut ws,
        "2",
        "page:card-1",
        json!({"viewport": {"width": 960, "height": 600, "dpr": 2.0, "mobile": false}}),
    )
    .await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "a `page:` join reaches the default mount: {reply}"
    );
    let told = next_text(&mut ws).await;
    assert_eq!(told[2], json!("page:card-1"));
    assert_eq!(told[3], json!("frame"));
    assert_eq!(
        told[4]["session"],
        json!("card-1"),
        "the session is the topic suffix: {told}"
    );
    assert_eq!(
        told[4]["params"]["viewport"]["width"],
        json!(960),
        "the viewport is ONE level deep, not two (OR-G32): {told}"
    );
    assert!(
        told[4]["params"].get("mount").is_none(),
        "the mount chose the door and does not travel with it: {told}"
    );

    // (b) a picture from the cell rides on `image`, and nothing calls it audio.
    send(
        &mut ws,
        json!(["1", "3", "page:card-1", "frame", {"type": "shoot"}]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    let back = match next_msg(&mut ws).await {
        WsMessage::Binary(b) => b.to_vec(),
        other => panic!("expected the picture as a binary frame, got {other:?}"),
    };
    assert_eq!(
        &back[..3],
        &[2u8, 11, 5],
        "a broadcast: kind 2, then the two lengths"
    );
    assert_eq!(&back[3..14], b"page:card-1");
    assert_eq!(
        &back[14..19],
        b"image",
        "the event is the kind's, not the other kind's"
    );
    assert_eq!(&back[19..], &[0xFF, 0xD8, 0xFF, 0xE0]);

    // (c) + (d) a text frame reaches the cell; a binary one from the client
    //     does not exist on this kind and is dropped where it arrives.
    ws.send(WsMessage::Binary(
        binary_push("1", "", "page:card-1", "image", &[9, 9, 9]).into(),
    ))
    .await
    .expect("send a picture nobody asked for");
    send(
        &mut ws,
        json!(["1", "4", "page:card-1", "frame", {"type": "pointer", "kind": "down"}]),
    )
    .await;
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    let echoed = next_text(&mut ws).await;
    assert_eq!(
        echoed[4],
        json!({"type": "pointer", "kind": "down"}),
        "the text reached the cell verbatim — and it is the NEXT thing back, \
         so the binary push before it never went anywhere: {echoed}"
    );

    // And a mount nothing holds is still the cell's own refusal, named.
    let reply = join(&mut ws, "27", "page:card-x", json!({"mount": "nothing"})).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("no surface is mounted as \"nothing\""),
        "{reply}"
    );

    // (g) eight pages, and the eight of them cost no call.
    for n in 2..=8 {
        let reply = join(
            &mut ws,
            &format!("1{n}"),
            &format!("page:card-{n}"),
            json!({}),
        )
        .await;
        assert_eq!(reply[4]["status"], json!("ok"), "page {n}: {reply}");
        let _joined = next_text(&mut ws).await;
    }
    let reply = join(&mut ws, "20", "voice:c1", json!({"mount": "voice"})).await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "eight windows must not cost a call: {reply}"
    );
    let _joined = next_text(&mut ws).await;

    // (f) the fifth call, with the sentence calls have.
    for n in 2..=4 {
        let reply = join(&mut ws, &format!("2{n}"), &format!("voice:c{n}"), json!({})).await;
        assert_eq!(reply[4]["status"], json!("ok"), "call {n}: {reply}");
        let _joined = next_text(&mut ws).await;
    }
    let reply = join(&mut ws, "25", "voice:c5", json!({})).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("too many voice topics on this socket"),
        "the count is per kind and says which one: {reply}"
    );

    // (e) the ninth page, with the sentence pages have.
    let reply = join(&mut ws, "26", "page:card-9", json!({})).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("too many page topics on this socket"),
        "{reply}"
    );
}
