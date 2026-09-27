//! GH #869 -- a display's socket reaches only the mounts it names.
//!
//! A `voice:` or `page:` join names its mount in the payload, and the page
//! writes the payload. Before `params.link_mounts` every cell with a link door
//! in the process was one join away from every display socket: a page could
//! name `phone` and talk to the telephone's door through the screen. The list
//! closes that; an empty list, the default, is the old behaviour, so a display
//! that never names the key behaves as it did.
//!
//! Two locks. The socket one: with `["voice"]` a join to `voice` opens, a join
//! to `phone` and a `page:` join to the kind's default `browser` are refused
//! with the list in the sentence; without the key the `phone` join opens. The
//! drift one: the display ships `link_mounts` equal to the two mounts its
//! curator is configured to speak to (`voice_mount`, `browser_mount`), so an
//! operator who moves one learns from the README that the other moves with it,
//! and this test fails if the shipped pair drifts.

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

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// A door that says `hello` and then holds the link open.
struct HelloOpener;

impl LinkOpener for HelloOpener {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        Box::pin(async move {
            let (to_cell_tx, mut to_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            let (from_cell_tx, from_cell_rx) = mpsc::channel::<LinkFrame>(LINK_QUEUE);
            tokio::spawn(async move {
                let hello = json!({"type": "hello", "session_id": req.session}).to_string();
                if from_cell_tx.send(LinkFrame::Text(hello)).await.is_err() {
                    return;
                }
                // Held until the client goes away.
                while to_cell_rx.recv().await.is_some() {}
            });
            Ok(Link {
                to_cell: to_cell_tx,
                from_cell: from_cell_rx,
            })
        })
    }
}

/// One page with one route, so the page topic has something to answer with.
fn seed_a_page(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    for (table, body) in [
        (
            "components",
            concat!(
                r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
                "\n",
                r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
                "\n"
            ),
        ),
        (
            "objects",
            concat!(
                r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
                "\n",
                r#"{"id":"home","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
                "\n"
            ),
        ),
        (
            "pages",
            concat!(
                r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
                "\n",
                r#"{"route":"/","root":"home","title":"Home"}"#,
                "\n"
            ),
        ),
    ] {
        std::fs::write(seed.join(format!("{table}.jsonl")), body).expect("seed file");
    }
}

/// A live `web` cell behind one listener. Aborted on drop.
struct Live {
    port: u16,
    listener: tokio::task::JoinHandle<()>,
    _sender: mpsc::Sender<meclaw_core::Message>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
    _dir: TempDir,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.join.abort();
        self.listener.abort();
    }
}

async fn start(params: Value, surfaces: Arc<SurfaceRegistry>) -> Live {
    let dir = TempDir::new().expect("tempdir");
    let cell_dir = dir.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_a_page(&cell_dir);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            params,
            out_tx,
            cell_dir.clone(),
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
        _dir: dir,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_text(ws: &mut Ws) -> Value {
    let msg = tokio::time::timeout(MARKER, ws.next())
        .await
        .expect("the cell answers within the failure-marker window")
        .expect("the stream stays open")
        .expect("a frame");
    match msg {
        WsMessage::Text(t) => meclaw_core::serde_json::from_str(&t).expect("the frame is JSON"),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

/// Open the socket and join the page topic, so the link joins that follow ride
/// a socket a real page would hold.
async fn socket(port: u16) -> Ws {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body.find(marker).expect("the shell carries a token") + marker.len();
    let token = &body[start..start + body[start..].find('"').expect("the token is quoted")];
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/{MOUNT}/live/websocket"))
            .await
            .expect("the cell accepts a websocket");
    let page_topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let join = json!(["1", "1", page_topic, "phx_join", {
        "session": token,
        "url": format!("http://127.0.0.1:{port}/{MOUNT}/")
    }]);
    ws.send(WsMessage::Text(join.to_string().into()))
        .await
        .expect("send");
    assert_eq!(next_text(&mut ws).await[4]["status"], json!("ok"));
    ws
}

/// Join `topic` with `payload` and return the reply.
async fn join(ws: &mut Ws, msg_ref: &str, topic: &str, payload: Value) -> Value {
    let frame = json!(["1", msg_ref, topic, "phx_join", payload]);
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("send");
    let reply = next_text(ws).await;
    assert_eq!(reply[3], json!("phx_reply"), "{reply}");
    reply
}

async fn registry_with(
    mounts: &[&'static str],
) -> (Arc<SurfaceRegistry>, Vec<Box<dyn std::any::Any + Send>>) {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
    for mount in mounts {
        let registered = surfaces
            .register(
                mount,
                SurfaceEntry {
                    kind: "voice",
                    cell_path: Path::new(&format!("/{mount}")),
                    links: Some(Arc::new(HelloOpener)),
                },
            )
            .await
            .expect("the mount is free");
        held.push(Box::new(registered));
    }
    (surfaces, held)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listed_display_reaches_its_mounts_and_no_other() {
    let (surfaces, _held) = registry_with(&["voice", "phone", "browser"]).await;
    let live = start(
        json!({"mount": MOUNT, "link_mounts": ["voice"]}),
        Arc::clone(&surfaces),
    )
    .await;
    let mut ws = socket(live.port).await;

    let reply = join(&mut ws, "2", "voice:c1", json!({"mount": "voice"})).await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "a listed mount opens: {reply}"
    );
    let hello = next_text(&mut ws).await;
    assert_eq!(hello[4]["type"], json!("hello"), "{hello}");

    let reply = join(&mut ws, "3", "voice:c2", json!({"mount": "phone"})).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("this display links no topic to \"phone\" (params.link_mounts: voice)"),
        "the refusal names the mount and the list: {reply}"
    );

    // The kind's default is a name like any other: `browser` is not listed.
    let reply = join(&mut ws, "4", "page:p1", json!({})).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert!(
        reply[4]["response"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("\"browser\"")),
        "{reply}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_key_every_mount_is_reachable_as_before() {
    let (surfaces, _held) = registry_with(&["voice", "phone"]).await;
    let live = start(json!({"mount": MOUNT}), Arc::clone(&surfaces)).await;
    let mut ws = socket(live.port).await;
    let reply = join(&mut ws, "2", "voice:c1", json!({"mount": "phone"})).await;
    assert_eq!(
        reply[4]["status"],
        json!("ok"),
        "the default is the old behaviour: {reply}"
    );
}

/// The drift lock: the display ships `link_mounts` equal to the two mounts its
/// curator is configured to speak to.
#[test]
fn the_display_links_exactly_the_mounts_its_curator_names() {
    let web = repo("templates/display/web/config.json");
    let compose = repo("templates/display/compose/config.json");
    if !web.is_file() || !compose.is_file() {
        return;
    }
    let read = |p: &std::path::Path| -> Value {
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("read")).expect("json")
    };
    let web = read(&web);
    let compose = read(&compose);
    let shipped = &web["override_params"][""]["link_mounts"];
    let curator = json!([
        compose["params"]["voice_mount"],
        compose["params"]["browser_mount"]
    ]);
    assert_eq!(
        shipped, &curator,
        "display/web link_mounts must equal [voice_mount, browser_mount] of display/compose"
    );
}
