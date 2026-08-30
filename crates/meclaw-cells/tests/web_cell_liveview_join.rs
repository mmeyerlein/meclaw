//! W8 Task 6 (GH #380, closes #381): the cell serves the LiveView socket itself.
//!
//! The point of #381 was that the serving machinery had to work for a **second**
//! consumer, one that is not the HTTP API. This is that consumer: a websocket
//! opened against the cell's own port, speaking the same Phoenix vsn 2.0.0
//! protocol, answered by the cell out of its own materialised pages — no
//! `Dispatcher`, no colony round trip, no `/surface/` prefix.
//!
//! R-W8-4b is what the join assertion is really about: the reply carries the
//! tree that the last **write** produced. A join is a read, and a read does no
//! diff work.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Seed a cell with two routes, so "which page is this socket on" is a real
/// question rather than one with only one possible answer.
fn seed_two_pages(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
            "\n",
            r#"{"name":"text","template":"<p>{{body}}</p>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#,
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
            "\n",
            r#"{"id":"h1","parent":"home","component":"text","ord":0,"props":"{\"body\":\"on home\"}"}"#,
            "\n",
            r#"{"id":"other","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n",
            r#"{"id":"o1","parent":"other","component":"text","ord":0,"props":"{\"body\":\"on other\"}"}"#,
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
            "\n",
            r#"{"route":"/other","root":"other","title":"Other"}"#,
            "\n"
        ),
    )
    .expect("pages");
}

/// A live cell, plus the handles that keep it alive.
struct Live {
    port: u16,
    _sender: mpsc::Sender<meclaw_core::Message>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
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

    // Wait for the page to be served before opening a socket against it: the
    // listener and the first render are both asynchronous.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/")).await
            && r.status().is_success()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Live {
        port,
        _sender: sender,
        _stop: stop_tx,
        join,
    }
}

/// Read the page and pull the session token out of its shell.
async fn token_of(port: u16) -> String {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body
        .find(marker)
        .expect("the shell carries a session token")
        + marker.len();
    let end = start + body[start..].find('"').expect("token is quoted");
    body[start..end].to_string()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(port: u16) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/live/websocket"))
        .await
        .expect("the cell must accept a websocket on /live/websocket");
    ws
}

async fn send(ws: &mut Ws, frame: Value) {
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("send");
}

async fn recv(ws: &mut Ws) -> Value {
    let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
        .await
        .expect("the cell must answer within the failure-marker window")
        .expect("stream open")
        .expect("frame");
    let WsMessage::Text(t) = msg else {
        panic!("expected a text frame, got {msg:?}")
    };
    meclaw_core::serde_json::from_str(&t).expect("the reply is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_is_answered_from_the_materialised_page() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_two_pages(&cell_dir);
    let live = start(&cell_dir).await;

    let token = token_of(live.port).await;
    let container = meclaw_surface::session::container_id("/web");
    let topic = format!("lv:{container}");
    let mut ws = connect(live.port).await;

    send(
        &mut ws,
        json!(["1", "1", topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/", live.port)
        }]),
    )
    .await;

    let reply = recv(&mut ws).await;
    assert_eq!(reply[3], json!("phx_reply"));
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");

    let rendered = &reply[4]["response"]["rendered"];
    // The packed tree of the seeded page: the root's template split at its
    // children, one slot per direct child.
    assert_eq!(rendered["s"], json!(["<main>", "</main>"]));
    assert_eq!(rendered["0"], json!("<p>on home</p>"));
    assert_eq!(
        reply[4]["response"]["liveview_version"],
        json!(meclaw_surface::LIVEVIEW_VERSION),
        "the version travels with the compiled-in bundle"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_heartbeat_is_answered_ok() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_two_pages(&cell_dir);
    let live = start(&cell_dir).await;

    let mut ws = connect(live.port).await;
    send(&mut ws, json!(["1", "2", "phoenix", "heartbeat", {}])).await;
    let reply = recv(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("ok"));
    assert_eq!(reply[1], json!("2"), "a reply reuses the message ref");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_join_url_decides_which_page_the_socket_is_on() {
    // One cell, one container id, two routes. Without the URL the socket could
    // not tell which page a viewer is looking at.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_two_pages(&cell_dir);
    let live = start(&cell_dir).await;

    let token = token_of(live.port).await;
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let mut ws = connect(live.port).await;

    send(
        &mut ws,
        json!(["1", "1", topic, "phx_join", {
            "session": token,
            "url": format!("http://127.0.0.1:{}/other", live.port)
        }]),
    )
    .await;

    let reply = recv(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("ok"), "{reply}");
    assert_eq!(
        reply[4]["response"]["rendered"]["0"],
        json!("<p>on other</p>"),
        "the socket answered with the OTHER page"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_that_names_another_surface_is_refused() {
    // The security property the session token exists for.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_two_pages(&cell_dir);
    let live = start(&cell_dir).await;

    let foreign = meclaw_surface::session::mint("/somebody-else");
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let mut ws = connect(live.port).await;

    send(
        &mut ws,
        json!(["1", "1", topic, "phx_join", { "session": foreign, "url": "/" }]),
    )
    .await;

    let reply = recv(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert!(
        reply[4]["response"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("does not name this surface"),
        "{reply}"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_on_a_route_nothing_declares_is_refused() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_two_pages(&cell_dir);
    let live = start(&cell_dir).await;

    let token = token_of(live.port).await;
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let mut ws = connect(live.port).await;

    send(
        &mut ws,
        json!(["1", "1", topic, "phx_join", { "session": token, "url": "/nope" }]),
    )
    .await;

    let reply = recv(&mut ws).await;
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");

    live.join.abort();
}
