//! W8 Task 9 (GH #380): `editable` events are local CRUD, with no round trip.
//!
//! Acceptance bullet 4 of the issue: *a drag on an `editable` prop round-trips
//! browser → cell → all viewers without entering the colony router*.
//!
//! The test proves both halves of that. The write lands and every joined viewer
//! sees it — **including a second browser that did nothing** — and the cell
//! emits **no message at all** while it happens. The second half is the one
//! that would otherwise go unnoticed: a cell that quietly emitted on every drag
//! would still look correct in the browser, and would flood the topology.
//!
//! And the negative case: a prop the component did not declare `editable` is
//! refused with `not_editable`, nothing is written, and the other viewer is not
//! disturbed. The declaration is the authorisation — a browser may move what a
//! component said may be moved, and nothing else.

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

/// A node component with `x` declared editable and `label` deliberately not.
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
    cell_dir: std::path::PathBuf,
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
        cell_dir: cell_dir.to_path_buf(),
        _mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn join_page(port: u16, join_ref: &str) -> Ws {
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
        json!([join_ref, "1", topic, "phx_join", {"session": token, "url": "/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let _ = ws.next().await.expect("open").expect("frame");
    ws
}

async fn next_frame(ws: &mut Ws, within: Duration) -> Option<Value> {
    match tokio::time::timeout(within, ws.next()).await {
        Ok(Some(Ok(WsMessage::Text(t)))) => {
            Some(meclaw_core::serde_json::from_str(&t).expect("json"))
        }
        _ => None,
    }
}

/// The stored value of a prop, read straight out of the cell's database.
fn stored_prop(cell_dir: &std::path::Path, id: &str, prop: &str) -> String {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open");
    let props: String = conn
        .query_row("SELECT props FROM objects WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .expect("row");
    let v: Value = meclaw_core::serde_json::from_str(&props).expect("json");
    v.get(prop)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_editable_write_reaches_every_viewer_without_a_message() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    let mut b = join_page(live.port, "2").await;

    a.send(WsMessage::Text(
        json!(["1", "9", format!("lv:{}", meclaw_surface::session::container_id("/web")),
               "event", {"event": "object:set", "value": {"id": "n1", "prop": "x", "value": "250"}}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send");

    // A's own reply, then the diff both of them get.
    let mut a_frames = Vec::new();
    while let Some(f) = next_frame(&mut a, Duration::from_secs(3)).await {
        a_frames.push(f);
    }
    assert!(
        a_frames
            .iter()
            .any(|f| f[3] == json!("phx_reply") && f[4]["status"] == json!("ok")),
        "the sender is told it was accepted: {a_frames:#?}"
    );
    assert!(
        a_frames.iter().any(|f| f[3] == json!("diff")),
        "the sender also sees the new picture: {a_frames:#?}"
    );

    // B did nothing and must still see it.
    let b_diff = next_frame(&mut b, Duration::from_secs(3))
        .await
        .expect("the other viewer receives the diff");
    assert_eq!(b_diff[3], json!("diff"), "{b_diff}");
    // Bare tree, no `{"diff": ...}` wrapper (GH #413) — the wrapper is the
    // reply shape, and on the push lane it reads as a junk slot client-side.
    assert!(
        b_diff[4].get("diff").is_none(),
        "the push payload is the tree itself, not a reply-shaped wrapper: {b_diff}"
    );
    assert!(
        b_diff[4].to_string().contains("250"),
        "the diff carries the new value: {b_diff}"
    );

    // The write landed.
    assert_eq!(stored_prop(&live.cell_dir, "n1", "x"), "250");

    // And nothing entered the colony. This is the assertion that a
    // browser-looking-correct test would miss.
    assert!(
        live.out_rx.try_recv().is_err(),
        "an editable write must emit NO message — zero topology round trip"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prop_that_is_not_editable_is_refused_and_nothing_moves() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let live = start(&cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    let mut b = join_page(live.port, "2").await;

    a.send(WsMessage::Text(
        json!(["1", "9", format!("lv:{}", meclaw_surface::session::container_id("/web")),
               "event", {"event": "object:set", "value": {"id": "n1", "prop": "label", "value": "hacked"}}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send");

    let reply = next_frame(&mut a, Duration::from_secs(5))
        .await
        .expect("the sender gets an answer");
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("not_editable"),
        "the refusal names the rule: {reply}"
    );

    // Nothing was written…
    assert_eq!(
        stored_prop(&live.cell_dir, "n1", "label"),
        "one",
        "a refused write must leave the value alone"
    );
    // …and nobody else was disturbed.
    assert!(
        next_frame(&mut b, Duration::from_secs(2)).await.is_none(),
        "a refused write pushes no diff"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_set_on_an_object_that_does_not_exist_is_refused() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let live = start(&cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    a.send(WsMessage::Text(
        json!(["1", "9", format!("lv:{}", meclaw_surface::session::container_id("/web")),
               "event", {"event": "object:set", "value": {"id": "ghost", "prop": "x", "value": "1"}}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send");

    let reply = next_frame(&mut a, Duration::from_secs(5))
        .await
        .expect("answer");
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_before_a_join_is_refused() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let live = start(&cell_dir).await;

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/live/websocket", live.port))
            .await
            .expect("connect");
    ws.send(WsMessage::Text(
        json!(["1", "1", "lv:whatever", "event", {"event": "object:set", "value": {}}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send");

    let reply = next_frame(&mut ws, Duration::from_secs(5))
        .await
        .expect("answer");
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");

    live.join.abort();
}
