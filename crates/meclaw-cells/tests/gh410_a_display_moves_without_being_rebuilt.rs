//! GH #410: a running display moves to another address without being rebuilt.
//!
//! Before this, `port` and `bind` were `IMMUTABLE_KEYS`, so moving a display
//! from loopback to a LAN bind meant re-instantiating the cell and replaying
//! every hand-made object position. The listener now rebinds on an accepted
//! params update while the `cell.db` — objects, components, pages, assets —
//! stays exactly where it is.
//!
//! What each test here proves:
//!
//! - the port moves: the old address stops answering, the new one serves the
//!   **same seeded page**, and a viewer that was joined is dropped and can
//!   rejoin against the new address;
//! - the bind address moves the same way, proven on a second loopback address
//!   (`127.0.0.2`) so "the new address serves" is not the same socket answering;
//! - a refused value does not move anything: neither the parse-lane refusal
//!   (port `0`) nor the runtime-lane one (a bind string nothing can resolve)
//!   costs the display its listener.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The failure-marker window (30 s convention): generous, because it only has
/// to be longer than a healthy run ever takes.
const MARKER: Duration = Duration::from_secs(30);

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// One page with one line of text on it, so "the new listener serves the same
/// display" is an assertion about content and not merely about a 200.
fn seed_one_page(cell_dir: &std::path::Path) {
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
            r#"{"id":"h1","parent":"home","component":"text","ord":0,"props":"{\"body\":\"still here\"}"}"#,
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

/// A live cell, plus everything that keeps it alive.
struct Live {
    sender: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path, params: Value) -> Live {
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(16);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            params,
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
    Live {
        sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

/// The params-update message: the `params` body slot, the shape
/// `config.md` § Access gives every cell type.
fn params_msg(params: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/web"))
        .reply_to(Path::new("/sender"))
        .body(Body::Inline(json!({ "params": params })))
        .build()
}

/// Wait until `host:port` serves the seeded page.
async fn wait_until_serving(host: &str, port: u16) {
    let deadline = Instant::now() + MARKER;
    loop {
        if let Ok(r) = reqwest::get(format!("http://{host}:{port}/")).await
            && r.status().is_success()
            && r.text().await.unwrap_or_default().contains("still here")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "http://{host}:{port}/ never served the display"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until nothing answers on `host:port` any more.
async fn wait_until_dead(host: &str, port: u16) {
    let deadline = Instant::now() + MARKER;
    loop {
        // A fresh client per attempt: a pooled connection to the old listener
        // would answer from a socket that was accepted before the move and say
        // nothing about whether the address is still bound.
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .expect("client");
        if client
            .get(format!("http://{host}:{port}/"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_err()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "http://{host}:{port}/ still answers after the move"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn token_of(host: &str, port: u16) -> String {
    let body = reqwest::get(format!("http://{host}:{port}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body.find(marker).expect("the shell carries a token") + marker.len();
    let end = start + body[start..].find('"').expect("token is quoted");
    body[start..end].to_string()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(host: &str, port: u16) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{host}:{port}/live/websocket"))
        .await
        .expect("the cell accepts a websocket on /live/websocket");
    ws
}

/// Join the root page and assert the display answered from its materialised
/// tree. Returns the socket, still open.
async fn join(host: &str, port: u16) -> Ws {
    let token = token_of(host, port).await;
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    let mut ws = connect(host, port).await;
    ws.send(WsMessage::Text(
        json!(["1", "1", topic, "phx_join", {
            "session": token,
            "url": format!("http://{host}:{port}/")
        }])
        .to_string()
        .into(),
    ))
    .await
    .expect("send join");
    let msg = tokio::time::timeout(MARKER, ws.next())
        .await
        .expect("the cell answers the join")
        .expect("stream open")
        .expect("frame");
    let WsMessage::Text(t) = msg else {
        panic!("expected a text frame, got {msg:?}")
    };
    let reply: Value = meclaw_core::serde_json::from_str(&t).expect("json");
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
    assert_eq!(
        reply[4]["response"]["rendered"]["0"],
        json!("<p>still here</p>"),
        "the join answers from the materialised page"
    );
    ws
}

/// Drain the socket until it ends. A dropped viewer is the point: the listener
/// moved, so the connection it was accepted on is gone and the client's own
/// reconnect brings it back on the new address.
async fn expect_socket_dropped(ws: &mut Ws) {
    let ended = tokio::time::timeout(MARKER, async {
        loop {
            match ws.next().await {
                None => return true,
                Some(Err(_)) => return true,
                // A close frame or a stray push is fine; keep reading until the
                // stream itself ends.
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert_eq!(
        ended,
        Ok(true),
        "the viewer's socket must be dropped when the listener moves"
    );
}

/// The shipped `web` template, or `None` in a tree that does not carry it.
fn shipped_web() -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/web");
    for rel in ["README.md", "template.json", "config.json"] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

/// The shipped contract admits a params update at all.
///
/// This is the gate that decides whether the capability exists on a real
/// instance, and it sits **before** the cell: `enforce_consumes_for_delivery`
/// checks the declared `consumes` against the body and, on a violation, answers
/// `consumes_violation` without ever calling `handle`. Up to `web@1.0.0` the
/// template declared `messages` as **required**, so the body every other test
/// here sends would have been refused at the door of a display instantiated
/// from the shipped template — the cell would have been able to move and never
/// been asked to.
///
/// `messages` is optional now, and `params` is declared beside it. Nothing is
/// lost by that: the two body shapes cannot be told apart by a declarative
/// type check, and the cell refuses a body carrying neither in its own words
/// (`invalid_input`, "expected tool_call turn"), which the second half asserts.
#[test]
fn the_shipped_contract_lets_a_params_update_reach_the_cell() {
    let Some(root) = shipped_web() else { return };
    let config: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(root.join("config.json")).expect("config.json"),
    )
    .expect("config.json is JSON");
    let block: meclaw_core::ConsumesBlock =
        meclaw_core::serde_json::from_value(config["contract"]["consumes"].clone())
            .expect("the consumes block deserialises");
    let compiled = meclaw_core::CompiledConsumes::compile(&block);
    let headers = meclaw_core::Headers::new();

    let update = json!({ "params": { "bind": "0.0.0.0" } });
    meclaw_core::validate_consumes(&update, &headers, &compiled)
        .expect("a params update must reach the cell of a shipped display");

    // And the tool-call body it shares the door with still passes.
    let patch = json!({ "messages": [{"type": "tool_call", "text": "{}", "id": "a"}] });
    meclaw_core::validate_consumes(&patch, &headers, &compiled).expect("a patch still passes");

    // The guard did not vanish, it moved: a body carrying neither slot is
    // admitted by the contract and refused by the cell, which is the only side
    // that can tell a display patch from a params update.
    assert!(
        meclaw_core::validate_consumes(&json!({}), &headers, &compiled).is_ok(),
        "an empty body is the cell's refusal to make, not the contract's"
    );
    assert!(
        block.body.get("params").is_some_and(|s| !s.required),
        "`params` is declared and optional: a patch carries none"
    );
}

/// The other half of the guard that moved: the cell refuses what the contract
/// now lets through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_with_neither_slot_is_refused_by_the_cell() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let port = free_port();
    let mut live = start(&cell_dir, json!({ "port": port })).await;
    wait_until_serving("127.0.0.1", port).await;

    live.sender
        .send(
            MessageBuilder::new(Path::new("/web"))
                .reply_to(Path::new("/sender"))
                .body(Body::Inline(json!({})))
                .build(),
        )
        .await
        .expect("mailbox");

    let emission = tokio::time::timeout(MARKER, live.out_rx.recv())
        .await
        .expect("an unreadable body is answered")
        .expect("emission");
    assert_eq!(
        emission.content["header"]["error_code"],
        json!("invalid_input"),
        "the cell keeps the refusal the contract stopped making: {}",
        emission.content
    );

    live.join.abort();
}

/// § 2d drift lock — the template's public surfaces promise the move, and the
/// promise is the one the cell keeps.
///
/// Both halves, which is what makes it a lock rather than a string pin. The
/// prose half reads the recipe out of the two shipped surfaces (`README.md` and
/// `template.json`); the mechanism half sends **that literal body** to a running
/// display and watches the listener follow it. A README that renames the slot,
/// and a cell that stops honouring it, are the same red.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_documented_move_recipe_moves_a_running_display() {
    let Some(root) = shipped_web() else { return };
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README");
    let template = std::fs::read_to_string(root.join("template.json")).expect("template.json");

    // The prose half. Both surfaces carry the retraction and the recipe; a
    // retraction that landed on only one of them is the Task-14 defect.
    for (name, text) in [("README.md", &readme), ("template.json", &template)] {
        assert!(
            text.contains("GH #410"),
            "{name} must name the retraction it carries"
        );
    }
    assert!(
        readme.contains(r#"{"params": {"bind": "0.0.0.0"}}"#),
        "the README's recipe is the body a caller sends"
    );
    assert!(
        template.contains("port and bind are NOT immutable"),
        "template.json states the retracted promise as retracted"
    );
    assert!(
        !readme.contains("`port` and `bind` are immutable, and the refusal says so."),
        "the retracted sentence must not still stand as a promise"
    );

    // The mechanism half: the README's own body, verbatim, against a live cell.
    // `0.0.0.0` is what the recipe says, and it is also the case that could not
    // be served by binding the new address before closing the old one — it
    // collides with `127.0.0.1` on the same port.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let port = free_port();
    let live = start(&cell_dir, json!({ "port": port })).await;
    wait_until_serving("127.0.0.1", port).await;

    let recipe: Value =
        meclaw_core::serde_json::from_str(r#"{"params": {"bind": "0.0.0.0"}}"#).expect("recipe");
    live.sender
        .send(
            MessageBuilder::new(Path::new("/web"))
                .reply_to(Path::new("/sender"))
                .body(Body::Inline(recipe))
                .build(),
        )
        .await
        .expect("mailbox");

    // The wildcard covers loopback, so `127.0.0.1` answering proves nothing on
    // its own — it answered before. A **second** loopback address does: nothing
    // was listening there while the bind was `127.0.0.1`, and the wildcard
    // reaches it.
    wait_until_serving("127.0.0.2", port).await;
    wait_until_serving("127.0.0.1", port).await;

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_update_moves_the_port_of_a_running_display() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let old = free_port();
    let new = free_port();
    let live = start(&cell_dir, json!({ "port": old })).await;
    wait_until_serving("127.0.0.1", old).await;
    let mut ws = join("127.0.0.1", old).await;

    live.sender
        .send(params_msg(json!({ "port": new })))
        .await
        .expect("mailbox");

    wait_until_serving("127.0.0.1", new).await;
    wait_until_dead("127.0.0.1", old).await;
    expect_socket_dropped(&mut ws).await;

    // The reconnect is the client's move, and it lands: the same display, the
    // same materialised page, on the new address.
    let _rejoined = join("127.0.0.1", new).await;

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_survives_a_respawn() {
    // The other side of "move, then write": what actually bound is written to
    // the `cell.db` params overlay, and a fresh spawn on the SAME directory
    // replays it. Without this the crash of a cell would silently undo an
    // operator's move — the display would come back on its birth port, while
    // whatever was told about the new one keeps pointing at nothing.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let birth = free_port();
    let moved = free_port();
    let first = start(&cell_dir, json!({ "port": birth })).await;
    wait_until_serving("127.0.0.1", birth).await;
    first
        .sender
        .send(params_msg(json!({ "port": moved })))
        .await
        .expect("mailbox");
    wait_until_serving("127.0.0.1", moved).await;

    // Take the display down and wait for the socket to actually be free, so the
    // second spawn is binding rather than colliding.
    first.join.abort();
    drop(first);
    let deadline = Instant::now() + MARKER;
    loop {
        if std::net::TcpListener::bind(("127.0.0.1", moved)).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "the first cell held its port");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Same directory, same BIRTH params — the overlay is the only thing that
    // can put this display anywhere but `birth`.
    let second = start(&cell_dir, json!({ "port": birth })).await;
    wait_until_serving("127.0.0.1", moved).await;
    wait_until_dead("127.0.0.1", birth).await;

    second.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_update_moves_the_bind_address_of_a_running_display() {
    // A second loopback address rather than `0.0.0.0`: binding the wildcard
    // would still answer on `127.0.0.1`, so "the new address serves" would be
    // provable without the old one having moved at all.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let port = free_port();
    let live = start(&cell_dir, json!({ "port": port, "bind": "127.0.0.1" })).await;
    wait_until_serving("127.0.0.1", port).await;

    live.sender
        .send(params_msg(json!({ "bind": "127.0.0.2" })))
        .await
        .expect("mailbox");

    wait_until_serving("127.0.0.2", port).await;
    wait_until_dead("127.0.0.1", port).await;

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_display_that_could_not_bind_at_boot_is_moved_by_a_message() {
    // The other half of the same capability. A port collision at boot used to
    // cost a restart to fix, because the I/O half parked until shutdown and saw
    // no reconfig. It is now an ordinary round with no listener, so the way out
    // is the same message as any other move.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy");
    let port = taken.local_addr().expect("addr").port();
    let free = free_port();

    let live = start(&cell_dir, json!({ "port": port })).await;
    // Whatever answers on `port` is the foreign listener, not the display: it
    // accepts and never speaks HTTP, so a request there cannot be mistaken for
    // a served page.

    live.sender
        .send(params_msg(json!({ "port": free })))
        .await
        .expect("mailbox");

    wait_until_serving("127.0.0.1", free).await;
    drop(taken);

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_port_leaves_the_old_listener_serving() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let port = free_port();
    let mut live = start(&cell_dir, json!({ "port": port })).await;
    wait_until_serving("127.0.0.1", port).await;

    // `0` is not a port here (the OS would read it as "pick one"), so the
    // update is refused before anything is closed.
    live.sender
        .send(params_msg(json!({ "port": 0 })))
        .await
        .expect("mailbox");

    let emission = tokio::time::timeout(MARKER, live.out_rx.recv())
        .await
        .expect("a refused params update is answered")
        .expect("emission");
    let content = &emission.content;
    assert_eq!(
        content["header"]["error_code"],
        json!("invalid_input"),
        "the existing refusal shape: {content}"
    );
    let text = content["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("65535"),
        "the refusal is the parser's, naming the bound it broke: {text}"
    );

    // And the display is exactly where it was.
    wait_until_serving("127.0.0.1", port).await;

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bind_nothing_can_resolve_leaves_the_old_listener_serving() {
    // This one passes the parser — any non-empty string is a legal `bind` — and
    // fails at the socket. The refusal therefore has to come back from the I/O
    // half, and the old address has to come back with it.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("cell dir");
    seed_one_page(&cell_dir);

    let port = free_port();
    let next = free_port();
    let mut live = start(&cell_dir, json!({ "port": port })).await;
    wait_until_serving("127.0.0.1", port).await;

    live.sender
        .send(params_msg(json!({ "bind": "no-such-address.invalid" })))
        .await
        .expect("mailbox");

    // The positive receipt: the sender is told, in the cell's ordinary error
    // shape, that the address it named could not be bound. Without this the
    // test would pass on "nothing happened at all".
    let emission = tokio::time::timeout(MARKER, live.out_rx.recv())
        .await
        .expect("an unbindable address is answered")
        .expect("emission");
    let content = &emission.content;
    assert_eq!(content["header"]["error_code"], json!("invalid_input"));
    let text = content["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("bind failed"),
        "the refusal names what failed: {text}"
    );

    // And the display is back where it was.
    wait_until_serving("127.0.0.1", port).await;

    // A display that fell back is still a display that can move: if the failed
    // bind had wedged or ended the I/O loop, this would go nowhere. It also
    // shows the refused value was never kept — an update naming only the port
    // moves the display, so the bind it merges over is still `127.0.0.1`.
    live.sender
        .send(params_msg(json!({ "port": next })))
        .await
        .expect("mailbox");
    wait_until_serving("127.0.0.1", next).await;
    wait_until_dead("127.0.0.1", port).await;

    live.join.abort();
}
