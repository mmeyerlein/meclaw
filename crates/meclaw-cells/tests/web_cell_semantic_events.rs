//! W8 Task 10 (GH #380): semantic events leave as ordinary messages.
//!
//! Acceptance bullet 5: *a semantic event arrives as a normal message on an
//! out-edge with minted ingress context.*
//!
//! This is the other half of R-W8-5, and the pair is what the test is really
//! about. An event on a prop the component declared `editable` is absorbed as
//! local CRUD and emits **nothing** (Task 9). Everything else — a button, a
//! form, later a microphone frame — leaves the cell as an ordinary source
//! emission on `hop.route = "event"`, exactly as the proxy emits an inbound
//! platform turn. Two classes, one socket, and which one an event belongs to is
//! decided by the component's declaration rather than by the event's name.
//!
//! # What this test does and does not prove
//!
//! It proves what the **cell** does: the emission exists, it is a source
//! emission, and its header carries `route`, `event_name`, `session_id` and the
//! page route.
//!
//! It does not prove context promotion, and deliberately so. Lifting
//! `session_id` into `context.session_id` is the ingress **edge's** job via
//! `set_context` — substrate behaviour, the same for this cell as for the
//! proxy. Asserting it here would be testing the colony through a display.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// A button (nothing editable) next to a node (`x` editable), so the two lanes
/// can be told apart in one cell.
fn seed(cell_dir: &std::path::Path) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        concat!(
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            "\n",
            r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
            "\n",
            r#"{"name":"node","template":"<i data-x=\"{{x}}\">{{label}}</i>","prop_schema":"{\"x\":\"text\",\"label\":\"text\"}","editable":"[\"x\"]","layer":"content"}"#,
            "\n"
        ),
    )
    .expect("components");
    std::fs::write(
        seed.join("objects.jsonl"),
        concat!(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            "\n",
            r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n",
            r#"{"id":"n1","parent":"root","component":"node","ord":0,"props":"{\"x\":\"10\",\"label\":\"one\"}"}"#,
            "\n"
        ),
    )
    .expect("objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        concat!(
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            "\n",
            r#"{"route":"/","root":"root","title":"Home"}"#,
            "\n"
        ),
    )
    .expect("pages");
}

struct Live {
    port: u16,
    _mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
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
        panic!("Active");
    };
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
        _mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Join the page, and hand back the socket plus the session token it used.
async fn join_page(port: u16) -> (Ws, String) {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body.find(marker).expect("token") + marker.len();
    let end = start + body[start..].find('"').expect("quote");
    let token = body[start..end].to_string();

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/live/websocket"))
            .await
            .expect("connect");
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    ws.send(WsMessage::Text(
        json!(["1", "1", topic, "phx_join", {"session": token, "url": "/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let _ = ws.next().await.expect("open").expect("frame");
    (ws, token)
}

async fn send_event(ws: &mut Ws, event: &str, value: Value) {
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    ws.send(WsMessage::Text(
        json!(["1", "9", topic, "event", {"event": event, "value": value}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_semantic_event_leaves_as_a_source_emission() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let (mut ws, token) = join_page(live.port).await;
    send_event(
        &mut ws,
        "action",
        json!({"name": "start", "payload": {"n": 1}}),
    )
    .await;

    let emission = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("a semantic event must reach the out-edges")
        .expect("emission");

    // A source emission: nobody asked for it, so there is no parent.
    assert!(
        emission.parent_message_id.is_none(),
        "a browser event has no parent message — it is a source emission"
    );
    assert_eq!(emission.sender_path.as_str(), "/web");

    let header = &emission.content["header"];
    assert_eq!(
        header["route"],
        json!("event"),
        "the out-edge is chosen by hop.route: {header}"
    );
    assert_eq!(header["event_name"], json!("action"));
    assert_eq!(header["page_route"], json!("/"));

    // The session id is the token's nonce — the half that is unique per page
    // load. It is what an ingress edge promotes into `context.session_id`.
    let nonce = token.split('.').next().expect("token has a nonce");
    assert_eq!(
        header["session_id"],
        json!(nonce),
        "the session id names this page load: {header}"
    );

    // The payload travels intact.
    assert_eq!(emission.content["event"]["value"]["name"], json!("start"));
    assert_eq!(emission.content["event"]["value"]["payload"]["n"], json!(1));

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_event_classes_do_not_mix() {
    // The pair that R-W8-5 is really about. One socket, two events, and which
    // lane each takes is decided by the component's `editable` declaration.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let (mut ws, _) = join_page(live.port).await;

    // Local: a declared editable prop. Absorbed, no message.
    send_event(
        &mut ws,
        "object:set",
        json!({"id": "n1", "prop": "x", "value": "99"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        live.out_rx.try_recv().is_err(),
        "an editable write must not enter the topology"
    );

    // Semantic: anything else. Emitted.
    send_event(&mut ws, "submit", json!({"form": "signup"})).await;
    let emission = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("a semantic event must be emitted")
        .expect("emission");
    assert_eq!(emission.content["header"]["event_name"], json!("submit"));

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_page_loads_carry_two_session_ids() {
    // The session id has to distinguish page loads, or an application on the
    // other end of the edge could not tell two browsers apart.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let (mut a, _) = join_page(live.port).await;
    let (mut b, _) = join_page(live.port).await;

    send_event(&mut a, "action", json!({"who": "a"})).await;
    let first = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("emission")
        .expect("emission");
    send_event(&mut b, "action", json!({"who": "b"})).await;
    let second = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("emission")
        .expect("emission");

    let id_a = first.content["header"]["session_id"].clone();
    let id_b = second.content["header"]["session_id"].clone();
    assert!(!id_a.as_str().unwrap_or_default().is_empty());
    assert_ne!(id_a, id_b, "two page loads are two sessions");

    live.join.abort();
}
