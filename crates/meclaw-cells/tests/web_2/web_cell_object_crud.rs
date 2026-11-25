//! W8 Task 7 (GH #380): object CRUD over messages, and the diff a bundle sends.
//!
//! Two acceptance bullets of the issue meet here:
//!
//! * *A bundle of N `object.*` calls is answered by one reply with N results in
//!   call order* — and the counting is of `tool_call` turns, not of messages.
//! * *A joined viewer gets ONE diff for the whole bundle*, and it carries the
//!   result of all N writes.
//!
//! The second bullet read "one diff per write" until GH #718. It was measured
//! wrong on the fresh instance (18.09.2026): a bundle is one caller's one
//! intention — a curator pass is exactly that — and the steps inside it are
//! pictures nobody should see. Per leg, one tap drew the window open, closed
//! and open again in 250 ms. The frames are still counted, for the opposite
//! reason: a test that only checked the final HTML would pass just as happily
//! if the cell had gone back to a frame per leg.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::deadline::{HANG, product_wait_ms, scaled, time_factor};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// A page with a stack root and one text child, so there is a slot to patch.
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
            r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
            "\n",
            r#"{"id":"a","parent":"root","component":"text","ord":0,"props":"{\"body\":\"first\"}"}"#,
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

/// The name this fixture's display answers to on the colony's one listener.
/// The port in every URL below is the LISTENER's: a `web` cell has none since
/// `web@2.0.0`.
const MOUNT: &str = "screen";

struct Live {
    /// The port of the one listener in front of the cell.
    port: u16,
    _listener: tokio::task::JoinHandle<()>,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

async fn start(cell_dir: &std::path::Path) -> Live {
    start_with(cell_dir, json!({ "mount": MOUNT })).await
}

async fn start_with(cell_dir: &std::path::Path, params: Value) -> Live {
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
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
        panic!("Active");
    };

    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let port = addr.port();
    let deadline = Instant::now() + Duration::from_secs(30);
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
        _listener: listener,
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

/// Send a body carrying the given tool calls, and read the one reply.
async fn call(live: &mut Live, calls: &[(&str, Value)]) -> Value {
    let turns: Vec<Value> = calls
        .iter()
        .map(|(id, args)| {
            json!({"origin": "tool", "type": "tool_call", "text": args.to_string(), "id": id})
        })
        .collect();
    let msg = MessageBuilder::new(Path::new("/web"))
        .body(Body::Inline(json!({ "messages": turns })))
        .reply_to(Path::new("/caller"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");

    loop {
        let emission = tokio::time::timeout(scaled(HANG), live.out_rx.recv())
            .await
            .expect("the cell must answer a tool call")
            .expect("an emission");
        // The end mark of the message before: no content, no answer.
        if !emission.content.is_null() {
            return emission.content;
        }
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a socket and join the root page.
async fn join(port: u16) -> Ws {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body.find(marker).expect("token") + marker.len();
    let end = start + body[start..].find('"').expect("quote");
    let token = &body[start..end];

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/{MOUNT}/live/websocket"))
            .await
            .expect("connect");
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    ws.send(WsMessage::Text(
        json!(["1", "1", topic, "phx_join", {"session": token, "url": "/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("send join");
    // The join reply.
    let _ = ws.next().await.expect("open").expect("frame");
    ws
}

/// Read the next frame, or `None` if none arrives within the window.
async fn next_frame(ws: &mut Ws, within: Duration) -> Option<Value> {
    match tokio::time::timeout(within, ws.next()).await {
        Ok(Some(Ok(WsMessage::Text(t)))) => {
            Some(meclaw_core::serde_json::from_str(&t).expect("frames are JSON"))
        }
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_of_three_answers_once_with_three_results_in_order() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(
        &mut live,
        &[
            (
                "c1",
                json!({"op": "object.create", "id": "b", "parent": "root", "component": "text", "ord": 1, "props": {"body": "second"}}),
            ),
            (
                "c2",
                json!({"op": "object.update", "id": "a", "props": {"body": "FIRST"}}),
            ),
            // `ord` is a sort key, not a list index: a move does not renumber
            // siblings, so putting `b` first means giving it an `ord` below
            // `a`'s, not claiming `a`'s. Asking for 0 here would tie with `a`
            // and be broken by id — deterministic, but not what was meant.
            ("c3", json!({"op": "object.move", "id": "b", "ord": -1})),
        ],
    )
    .await;

    assert_eq!(reply["header"]["operation"], json!("bundle"));
    assert_eq!(reply["header"]["bundle_errors"], json!(0));
    let results = reply["results"].as_array().expect("results[]");
    assert_eq!(results.len(), 3, "one result per call: {reply}");
    assert_eq!(results[0]["tool_call_id"], json!("c1"));
    assert_eq!(results[1]["tool_call_id"], json!("c2"));
    assert_eq!(results[2]["tool_call_id"], json!("c3"));
    assert_eq!(results[2]["operation"], json!("object.move"));

    // A turn carries exactly the four schema-pure keys; metadata lives in
    // `results[]`, or the colony would dead-letter the whole reply.
    for turn in reply["messages"].as_array().expect("messages") {
        assert_eq!(turn.as_object().unwrap().len(), 4, "turn: {turn}");
    }

    // The moved order is what the page now shows.
    let html = reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let second = html.find("second").expect("second is on the page");
    let first = html.find("FIRST").expect("the update landed");
    assert!(second < first, "object b moved to ord 0: {html}");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_joined_viewer_gets_one_diff_for_the_whole_bundle() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;
    let mut ws = join(live.port).await;

    let _ = call(
        &mut live,
        &[
            (
                "c1",
                json!({"op": "object.create", "id": "b", "parent": "root", "component": "text", "ord": 1, "props": {"body": "two"}}),
            ),
            (
                "c2",
                json!({"op": "object.update", "id": "a", "props": {"body": "one"}}),
            ),
            ("c3", json!({"op": "object.move", "id": "b", "ord": -1})),
        ],
    )
    .await;

    // Three writes, ONE frame (GH #718). Counting them is the point: a cell
    // that pushed per leg would leave the same final picture and would have
    // drawn two intermediate ones on the way there, which is what a viewer
    // sees as a flicker.
    let mut frames = Vec::new();
    while let Some(f) = next_frame(&mut ws, Duration::from_secs(2)).await {
        frames.push(f);
    }
    assert_eq!(
        frames.len(),
        1,
        "expected one diff for the bundle, got {}: {frames:#?}",
        frames.len()
    );
    for f in &frames {
        assert_eq!(f[3], json!("diff"), "a push is a diff frame: {f}");
        assert_eq!(f[1], Value::Null, "a push has no message ref: {f}");
        // Bare tree, no `{"diff": ...}` wrapper (GH #413): the client hands a
        // push payload straight to `Rendered.extract`.
        assert!(f[4].is_object(), "the payload is the diff tree itself: {f}");
        assert!(
            f[4].get("diff").is_none(),
            "no reply-shaped wrapper on the push lane: {f}"
        );
    }

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_broken_leg_leaves_its_siblings_standing() {
    // A bundle is explicitly not a transaction. The refusal names the leg, the
    // siblings still applied, and the header does not claim the whole reply
    // failed.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(
        &mut live,
        &[
            (
                "ok",
                json!({"op": "object.update", "id": "a", "props": {"body": "applied"}}),
            ),
            (
                "bad",
                json!({"op": "object.create", "id": "z", "parent": "root", "component": "ghost", "props": {}}),
            ),
        ],
    )
    .await;

    assert_eq!(reply["header"]["bundle_errors"], json!(1));
    assert!(
        !reply["header"]
            .as_object()
            .unwrap()
            .contains_key("error_code"),
        "the reply as a whole is not a refusal: {reply}"
    );
    assert_eq!(
        reply["results"][1]["error_code"],
        json!("unknown_component")
    );
    assert_eq!(
        reply["header"]["rows_affected"],
        json!(1),
        "the sibling's write counted"
    );

    let html = reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    assert!(html.contains("applied"), "the good leg landed: {html}");

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_call_answers_with_its_metadata_on_the_header() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(
        &mut live,
        &[(
            "only",
            json!({"op": "object.update", "id": "a", "props": {"body": "solo"}}),
        )],
    )
    .await;

    assert_eq!(reply["header"]["operation"], json!("object.update"));
    assert_eq!(reply["header"]["rows_affected"], json!(1));
    assert!(
        reply.get("results").is_none(),
        "a single op has no results[] slot: {reply}"
    );
    assert_eq!(reply["messages"][0]["id"], json!("only"));

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_with_children_is_refused_and_names_them() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(
        &mut live,
        &[("d", json!({"op": "object.delete", "id": "root"}))],
    )
    .await;
    assert_eq!(reply["header"]["error_code"], json!("invalid_input"));
    let text = reply["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains('a'),
        "the refusal names the children that block it: {text}"
    );

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_reads_state_back() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(&mut live, &[("q", json!({"op": "query", "id": "a"}))]).await;
    assert_eq!(reply["header"]["operation"], json!("query"));
    let text = reply["messages"][0]["text"].as_str().expect("text");
    let payload: Value = meclaw_core::serde_json::from_str(text).expect("json");
    assert_eq!(payload["object"]["component"], json!("text"));
    assert_eq!(payload["object"]["props"]["body"], json!("first"));

    live.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_prop_is_refused_rather_than_stored() {
    // A template can only render what it names, so an undeclared prop is
    // invisible — accepting it would let a model believe it had set something.
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;

    let reply = call(
        &mut live,
        &[(
            "u",
            json!({"op": "object.update", "id": "a", "props": {"colour": "red"}}),
        )],
    )
    .await;
    assert_eq!(reply["header"]["error_code"], json!("invalid_input"));
    let text = reply["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("colour"),
        "the refusal names the prop: {text}"
    );
    assert!(text.contains("body"), "and says what is declared: {text}");

    live.join.abort();
}

/// An `object.update` on the ROOT publishes and pushes (GH #414 family).
///
/// `touched_by` cannot name a slot for the root — the whole page is the slot —
/// and returns `structural` with an empty slot list. The bundle path used to
/// publish only for non-empty slot lists, so a root update sat in the database
/// while the served page and every joined viewer kept the old render
/// indefinitely (the steady-state tick emits nothing, so nothing caught up).
/// Found live: a `client_js` refresh that never reached one single tab.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_root_update_republishes_the_page_and_pushes_every_viewer() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;
    let mut ws = join(live.port).await;

    let _ = call(
        &mut live,
        &[(
            "r1",
            json!({"op": "object.update", "id": "root", "props": {}}),
        )],
    )
    .await;

    // The viewer hears about it: one push. The whole page is re-rendered, and
    // the frame is its difference against what the viewer holds — this root
    // draws no prop, so its statics are the same and no packed tree travels:
    // a packed tree makes the client re-create every node of the page
    // (GH #1009). A root that draws a changed prop still sends the packed
    // tree (`718_a_bundle_is_pushed_once`).
    let frame = next_frame(&mut ws, Duration::from_secs(5))
        .await
        .expect("a root update reaches the joined viewer");
    assert_eq!(frame[3], json!("diff"), "{frame}");
    assert!(
        frame[4].get("s").is_none(),
        "the root's statics did not change, so the diff is no packed tree: {frame}"
    );

    // And a fresh GET serves the re-materialised page rather than a snapshot
    // from before the write.
    let body = reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    assert!(
        body.contains("<main>"),
        "the page still renders after a root republish: {body:.200}"
    );

    live.join.abort();
}

/// The view every stuck statement below reads: more rows than any machine counts
/// inside the cell's timeout, and still an end, so a cell that does not
/// interrupt it fails the test instead of holding the run.
const ENDLESS: &str = "CREATE VIEW endless AS
     WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 100000000)
     SELECT i FROM n;";

/// The timeout of a call that has to land a statement before the stuck one is
/// cut: seconds for the statement, far below what [`ENDLESS`] takes.
fn lands_then_cut_ms() -> u64 {
    (3_000.0 * time_factor()) as u64
}

/// The timeout of a call that only has to be cut.
const CUT_MS: u64 = 300;

/// Change the running cell's operation timeout; it holds from the next call on.
async fn set_timeout(live: &Live, ms: u64) {
    let msg = MessageBuilder::new(Path::new("/web"))
        .body(Body::Inline(
            json!({ "params": { "external_timeout_ms": ms } }),
        ))
        .reply_to(Path::new("/caller"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");
}

/// Change the cell's database beside the running cell.
fn rig(cell_dir: &std::path::Path, sql: &str) {
    let c = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open cell.db");
    c.busy_timeout(scaled(HANG)).expect("busy");
    c.execute_batch(sql).expect("rig");
}

async fn served(live: &Live) -> String {
    reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text")
}

/// Give the page root a prop that shows, so a root update can be seen.
async fn root_with_a_prop(live: &mut Live) {
    let done = call(
        live,
        &[(
            "d",
            json!({"op": "component.define", "name": "stack",
                   "template": "<main data-t=\"{{t}}\">{{children}}</main>",
                   "prop_schema": {"t": "text"}}),
        )],
    )
    .await;
    assert_eq!(done["header"]["rows_affected"], json!(1), "{done}");
}

/// GH #1264: a database call past `external_timeout_ms` is answered, not waited
/// for. The bundle ran on an unbounded call: nothing but the substrate's
/// backstop ended it. Now its transaction is gone with the interrupt: every
/// leg that wrote or had not run answers `query_timeout`, nothing is stored,
/// a read that was done keeps its payload, and the cell takes the next call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1264_a_bundle_past_the_query_timeout_is_answered_not_hung() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start_with(
        &cell_dir,
        json!({ "mount": MOUNT, "external_timeout_ms": product_wait_ms() }),
    )
    .await;
    rig(
        &cell_dir,
        &format!(
            "{ENDLESS} CREATE TRIGGER stuck AFTER UPDATE ON objects
               WHEN NEW.props LIKE '%stuck%'
               BEGIN SELECT count(*) FROM endless; END;"
        ),
    );

    let update = |body: &str| json!({"op": "object.update", "id": "a", "props": {"body": body}});
    set_timeout(&live, lands_then_cut_ms()).await;
    let reply = call(
        &mut live,
        &[
            ("r", json!({"op": "query", "id": "a"})),
            ("w", update("one")),
            ("s", update("stuck")),
            ("l", update("late")),
        ],
    )
    .await;
    let results = reply["results"].as_array().expect("results");
    assert_eq!(results.len(), 4, "{reply}");
    assert!(results[0].get("error_code").is_none(), "{reply}");
    for (leg, op) in results.iter().zip(["r", "w", "s", "l"]).skip(1) {
        assert_eq!(leg["tool_call_id"], json!(op), "{reply}");
        assert_eq!(leg["error_code"], json!("query_timeout"), "{reply}");
        assert_eq!(leg["rows_affected"], json!(0), "{reply}");
        assert_eq!(leg["operation"], json!("object.update"), "{reply}");
    }
    assert_eq!(reply["header"]["bundle_errors"], json!(3), "{reply}");

    set_timeout(&live, CUT_MS).await;
    let single = call(&mut live, &[("s", update("stuck"))]).await;
    assert_eq!(
        single["header"]["error_code"],
        json!("query_timeout"),
        "{single}"
    );

    set_timeout(&live, product_wait_ms()).await;
    let read = call(&mut live, &[("r", json!({"op": "query", "id": "a"}))]).await;
    let text = read["messages"][0]["text"].as_str().expect("text");
    assert!(text.contains("first"), "nothing was stored: {read}");
    let next = call(&mut live, &[("n", update("next"))]).await;
    assert_eq!(next["header"]["rows_affected"], json!(1), "{next}");
    assert!(served(&live).await.contains("<p>next</p>"));

    live.join.abort();
}

/// GH #1264: a re-render that fails as a whole leaves its write unpublished, and
/// the next write publishes it too. It used to patch its own slot into the old
/// pages, so the lost root change stayed invisible until some later broadcast.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1264_a_failed_republish_is_caught_up_by_the_next_write() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start(&cell_dir).await;
    root_with_a_prop(&mut live).await;

    rig(&cell_dir, "ALTER TABLE pages RENAME TO pages_away;");
    let root = call(
        &mut live,
        &[(
            "u",
            json!({"op": "object.update", "id": "root", "props": {"t": "dusk"}}),
        )],
    )
    .await;
    assert_eq!(root["header"]["rows_affected"], json!(1), "{root}");
    rig(&cell_dir, "ALTER TABLE pages_away RENAME TO pages;");
    assert!(
        !served(&live).await.contains("dusk"),
        "the rig did not make the re-render fail"
    );

    let next = call(
        &mut live,
        &[(
            "n",
            json!({"op": "object.update", "id": "a", "props": {"body": "next"}}),
        )],
    )
    .await;
    assert_eq!(next["header"]["rows_affected"], json!(1), "{next}");
    let page = served(&live).await;
    assert!(page.contains("<p>next</p>"), "{page}");
    assert!(page.contains(r#"data-t="dusk""#), "{page}");

    live.join.abort();
}

/// GH #1264: a re-render past `external_timeout_ms` is given up, keeps the pages
/// last published (none is dropped half-rendered), and is caught up the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1264_a_republish_past_the_query_timeout_is_given_up_and_caught_up() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);
    let mut live = start_with(
        &cell_dir,
        json!({ "mount": MOUNT, "external_timeout_ms": product_wait_ms() }),
    )
    .await;
    root_with_a_prop(&mut live).await;

    // Only the template is stuck, which an update does not read.
    rig(
        &cell_dir,
        &format!(
            "{ENDLESS} ALTER TABLE components RENAME TO components_away;
             CREATE VIEW components AS
               SELECT name, prop_schema, editable, layer,
                      CASE WHEN (SELECT count(*) FROM endless) > 0 THEN template END AS template
                 FROM components_away;"
        ),
    );
    // The update has to land under the timeout that cuts its re-render.
    set_timeout(&live, lands_then_cut_ms()).await;
    let root = call(
        &mut live,
        &[(
            "u",
            json!({"op": "object.update", "id": "root", "props": {"t": "dusk"}}),
        )],
    )
    .await;
    assert_eq!(root["header"]["rows_affected"], json!(1), "{root}");
    let page = served(&live).await;
    assert!(page.contains("<p>first</p>"), "the old page stays: {page}");
    assert!(!page.contains("dusk"), "{page}");

    rig(
        &cell_dir,
        "DROP VIEW components; ALTER TABLE components_away RENAME TO components;",
    );
    set_timeout(&live, product_wait_ms()).await;
    let next = call(
        &mut live,
        &[(
            "n",
            json!({"op": "object.update", "id": "a", "props": {"body": "next"}}),
        )],
    )
    .await;
    assert_eq!(next["header"]["rows_affected"], json!(1), "{next}");
    let page = served(&live).await;
    assert!(page.contains("<p>next</p>"), "{page}");
    assert!(page.contains(r#"data-t="dusk""#), "{page}");

    live.join.abort();
}
