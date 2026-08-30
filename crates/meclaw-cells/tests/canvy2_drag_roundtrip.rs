//! W8 Task 14 (GH #383): a drag on canvy's own picture, over the wire.
//!
//! The claim under test is the one the whole re-cut rests on: **moving a box
//! costs no message.** The browser sends `object:set` on a prop the node
//! component declared `editable`, the display writes it into its own database
//! and pushes the new picture to every viewer — and the colony router never
//! hears about it.
//!
//! Three things are asserted together, because any two of them without the
//! third would look correct and be wrong:
//!
//! 1. **A second browser sees it.** A viewer that did nothing gets the diff, so
//!    two people arranging the same canvas are looking at the same canvas.
//! 2. **It is on disk.** The position survives, which is what makes the next
//!    tick of the layout keep it instead of computing one.
//! 3. **Nothing was emitted.** A cell that quietly sent a message per drag would
//!    still look right in the browser and would flood the topology at 60 Hz.
//!
//! …and the negative case: a prop the component did NOT declare `editable` is
//! refused with `not_editable`, nothing is written, and the other viewer is not
//! disturbed. The declaration is the authorisation — a browser may move what a
//! component said may be moved, and nothing else.
//!
//! The picture is the SHIPPED one: the bootstrap bundle comes out of
//! `templates/canvy/layout/config.json`, run as the `code` cell runs it. A test
//! that hand-wrote a component here would prove a display, not canvy.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const CELL_PATH: &str = "/canvy/web";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// R2b: where the template does not ship, this skips rather than failing on a
/// dead reference. Python too — a missing interpreter is not a failing canvas.
fn shipped_canvy() -> Option<std::path::PathBuf> {
    let root = core_root().join("templates/canvy");
    for rel in ["layout/config.json", "probe/config.json"] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()?;
    Some(root)
}

/// Run the layout cell's shipped bytes, with the script on stdin (GH #349: the
/// script carries the whole browser half of canvy and is far past the 128 KiB
/// argv cap).
fn run_layout(root: &std::path::Path, body: Value, hop: Value, context: Value) -> Vec<Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("layout/config.json")).unwrap(),
    )
    .unwrap();
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap();
    let doc = json!({
        "envelope": {
            "header": { "context": context, "hop": hop },
            "target": "/canvy/layout",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64
        },
        "body": body,
        "params": {}
    })
    .to_string();

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&doc).unwrap(),
    );
    let mut child = Command::new(runner)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "layout exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json") {
        Value::Array(items) => items,
        other => vec![other],
    }
}

/// The bootstrap bundle for a two-cell colony: exactly what an instance of
/// `canvy@2.0.0` sends at its first tick.
fn bootstrap_calls(root: &std::path::Path) -> Vec<Value> {
    let graph = json!({
        "scope": "/",
        "nodes": [
            {"path": "/a/one", "cell_type": "code"},
            {"path": "/a/two", "cell_type": "store"},
        ],
        "edges": [{"id": "e1", "from": "/a/one", "to": "/a/two"}],
    });
    let ask = run_layout(
        root,
        json!({ "messages": [], "graph": graph }),
        json!({ "route": "snapshot" }),
        json!({}),
    );
    let hop = ask[0]["header"]["canvy_graph"]
        .as_str()
        .unwrap()
        .to_string();
    let boot = run_layout(
        root,
        json!({
            "messages": [{"origin": "tool", "type": "tool_result", "id": "q",
                          "text": "no page declares the route \"/\""}]
        }),
        json!({ "operation": "query", "error_code": "invalid_input" }),
        json!({ "canvy_origin": "layout", "canvy_graph": hop }),
    );
    boot[0]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|t| {
            meclaw_core::serde_json::from_str(t["text"].as_str().expect("text")).expect("args")
        })
        .collect()
}

struct Live {
    port: u16,
    cell_dir: std::path::PathBuf,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

/// A `web` cell with an empty database — what a canvy instance starts with,
/// since a ref directory carries no seed — bootstrapped by the shipped bundle.
async fn start(root: &std::path::Path, cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new(CELL_PATH),
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
        if reqwest::get(format!("http://127.0.0.1:{port}/"))
            .await
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never bound its port");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut live = Live {
        port,
        cell_dir: cell_dir.to_path_buf(),
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    };

    let calls = bootstrap_calls(root);
    let turns: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, args)| {
            json!({"origin": "assistant", "type": "tool_call",
                   "text": args.to_string(), "id": format!("c{i}")})
        })
        .collect();
    live.mailbox
        .send(
            MessageBuilder::new(Path::new(CELL_PATH))
                .body(Body::Inline(json!({ "messages": turns })))
                .reply_to(Path::new("/canvy/layout"))
                .build(),
        )
        .await
        .expect("mailbox");
    let reply = tokio::time::timeout(Duration::from_secs(60), live.out_rx.recv())
        .await
        .expect("the display must answer the bootstrap")
        .expect("an emission");
    assert_eq!(
        reply.content["header"]["bundle_errors"],
        json!(0),
        "every leg of the bootstrap must land: {}",
        reply.content["header"]
    );
    live
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn topic() -> String {
    format!("lv:{}", meclaw_surface::session::container_id(CELL_PATH))
}

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
    ws.send(WsMessage::Text(
        json!([join_ref, "1", topic(), "phx_join", {"session": token, "url": "/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let _ = ws.next().await.expect("open").expect("frame");
    ws
}

async fn drag(ws: &mut Ws, join_ref: &str, id: &str, prop: &str, value: Value) {
    ws.send(WsMessage::Text(
        json!([join_ref, "9", topic(), "event",
               {"event": "object:set", "value": {"id": id, "prop": prop, "value": value}}])
        .to_string()
        .into(),
    ))
    .await
    .expect("send");
}

async fn next_frame(ws: &mut Ws, within: Duration) -> Option<Value> {
    match tokio::time::timeout(within, ws.next()).await {
        Ok(Some(Ok(WsMessage::Text(t)))) => {
            Some(meclaw_core::serde_json::from_str(&t).expect("frames are JSON"))
        }
        _ => None,
    }
}

/// The stored value of a prop, read straight out of the display's database.
fn stored_prop(cell_dir: &std::path::Path, id: &str, prop: &str) -> Value {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open");
    let props: String = conn
        .query_row("SELECT props FROM objects WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .expect("row");
    let v: Value = meclaw_core::serde_json::from_str(&props).expect("json");
    v.get(prop).cloned().unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drag_reaches_every_viewer_and_costs_no_message() {
    let Some(root) = shipped_canvy() else { return };
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&root, &cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    let mut b = join_page(live.port, "2").await;

    drag(&mut a, "1", "n/a/one", "x", json!(4321)).await;

    // A's own reply, then the diff both of them get.
    let mut a_frames = Vec::new();
    while let Some(f) = next_frame(&mut a, Duration::from_secs(3)).await {
        a_frames.push(f);
    }
    assert!(
        a_frames
            .iter()
            .any(|f| f[3] == json!("phx_reply") && f[4]["status"] == json!("ok")),
        "the dragger is told it was accepted: {a_frames:#?}"
    );
    assert!(
        a_frames.iter().any(|f| f[3] == json!("diff")),
        "and sees the new picture: {a_frames:#?}"
    );

    // B did nothing and must still see it. This is what makes two people
    // arranging one canvas look at one canvas.
    let b_diff = next_frame(&mut b, Duration::from_secs(5))
        .await
        .expect("the other viewer receives the diff");
    assert_eq!(b_diff[3], json!("diff"), "{b_diff}");
    // The tree rides BARE in the push payload (GH #413): the LiveView client
    // hands the payload straight to `Rendered.extract`, so a `{"diff": ...}`
    // wrapper becomes a junk slot and no browser ever applies the update.
    assert!(
        b_diff[4].get("diff").is_none(),
        "the push payload is the tree itself, not a reply-shaped wrapper: {b_diff}"
    );
    assert!(
        b_diff[4].to_string().contains("translate(4321"),
        "the diff carries the box at its new place: {b_diff}"
    );

    // The write landed, in the shape the layout reads back on the next tick.
    assert_eq!(stored_prop(&live.cell_dir, "n/a/one", "x"), json!(4321));

    // And nothing entered the colony. A drag that emitted would still look
    // correct in the browser and would flood the topology.
    assert!(
        live.out_rx.try_recv().is_err(),
        "a drag must emit NO message — zero topology round trip"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prop_the_component_did_not_open_is_refused() {
    let Some(root) = shipped_canvy() else { return };
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let live = start(&root, &cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    let mut b = join_page(live.port, "2").await;

    // `canvy-node` declares `editable: ["x","y"]` and nothing else. A browser
    // that asks for the cell's TYPE would be rewriting what the colony said.
    drag(&mut a, "1", "n/a/one", "type", json!("llm")).await;

    let reply = next_frame(&mut a, Duration::from_secs(10))
        .await
        .expect("the sender gets an answer");
    assert_eq!(reply[4]["status"], json!("error"), "{reply}");
    assert_eq!(
        reply[4]["response"]["reason"],
        json!("not_editable"),
        "the refusal names the rule: {reply}"
    );

    assert_eq!(
        stored_prop(&live.cell_dir, "n/a/one", "type"),
        json!("code"),
        "a refused write must leave the value alone"
    );
    assert!(
        next_frame(&mut b, Duration::from_secs(2)).await.is_none(),
        "a refused write pushes no diff"
    );

    live.join.abort();
}

/// The whole point of `object:set` being the ONE browser event: what the hook
/// sends on release is two of them, and both land.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_events_of_one_release_both_land() {
    let Some(root) = shipped_canvy() else { return };
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&root, &cell_dir).await;

    let mut a = join_page(live.port, "1").await;
    for (prop, value) in [("x", 700), ("y", 240)] {
        drag(&mut a, "1", "n/a/one", prop, json!(value)).await;
        // Drain this event's reply and diff before sending the next, so the two
        // are not read for each other.
        let mut saw_ok = false;
        while let Some(f) = next_frame(&mut a, Duration::from_secs(3)).await {
            if f[3] == json!("phx_reply") && f[4]["status"] == json!("ok") {
                saw_ok = true;
            }
        }
        assert!(saw_ok, "{prop} was not accepted");
    }

    assert_eq!(stored_prop(&live.cell_dir, "n/a/one", "x"), json!(700));
    assert_eq!(stored_prop(&live.cell_dir, "n/a/one", "y"), json!(240));
    assert!(
        live.out_rx.try_recv().is_err(),
        "two events, still no message"
    );

    live.join.abort();
}

/// The un-pin has to be writable from a browser, or GH #415 is only half fixed:
/// the panel would send an event the display refuses with `not_editable` and the
/// cell would stay nailed down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_may_clear_the_pin_marker() {
    let Some(root) = shipped_canvy() else { return };
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&root, &cell_dir).await;

    let mut a = join_page(live.port, "1").await;

    // The drag's marker…
    drag(&mut a, "1", "n/a/one", "pinned", json!("1")).await;
    let mut ok_seen = false;
    while let Some(f) = next_frame(&mut a, Duration::from_secs(3)).await {
        if f[3] == json!("phx_reply") && f[4]["status"] == json!("ok") {
            ok_seen = true;
        }
    }
    assert!(ok_seen, "a drag's marker is a declared editable prop");
    assert_eq!(stored_prop(&live.cell_dir, "n/a/one", "pinned"), json!("1"));

    // …and the panel's release, which is the same prop back to empty.
    drag(&mut a, "1", "n/a/one", "pinned", json!("")).await;
    let mut released = false;
    while let Some(f) = next_frame(&mut a, Duration::from_secs(3)).await {
        if f[3] == json!("phx_reply") && f[4]["status"] == json!("ok") {
            released = true;
        }
    }
    assert!(
        released,
        "and clearing it is the same declaration, not a second one"
    );
    assert_eq!(
        stored_prop(&live.cell_dir, "n/a/one", "pinned"),
        json!(""),
        "the marker is gone, so the next layout tick lays this cell out again"
    );

    // Still the local lane: an un-pin is CRUD on the display's own database.
    assert!(
        live.out_rx.try_recv().is_err(),
        "releasing a cell must emit NO message either"
    );

    live.join.abort();
}
