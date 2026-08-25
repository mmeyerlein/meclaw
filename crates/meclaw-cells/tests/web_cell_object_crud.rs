//! W8 Task 7 (GH #380): object CRUD over messages, and one diff per write.
//!
//! Two acceptance bullets of the issue meet here:
//!
//! * *A bundle of N `object.*` calls is answered by one reply with N results in
//!   call order* — and the counting is of `tool_call` turns, not of messages.
//! * *Joined viewers receive exactly one diff per write* — three writes, three
//!   frames, in order, and the final page shows the result of all three.
//!
//! The second is the one worth being careful about: a test that only checked
//! the final HTML would pass just as happily if the cell had sent one diff at
//! the end, which is the behaviour R-W8-4 exists to prevent. So the frames are
//! counted.

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

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

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

struct Live {
    port: u16,
    mailbox: mpsc::Sender<meclaw_core::Message>,
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

    let emission = tokio::time::timeout(Duration::from_secs(30), live.out_rx.recv())
        .await
        .expect("the cell must answer a tool call")
        .expect("an emission");
    emission.content
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a socket and join the root page.
async fn join(port: u16) -> Ws {
    let body = reqwest::get(format!("http://127.0.0.1:{port}/"))
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
    let html = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
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
async fn a_joined_viewer_gets_exactly_one_diff_per_write() {
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

    // Three writes, three frames. Counting them is the point: a cell that sent
    // one diff at the end of the bundle would still leave the right final
    // picture, and that is exactly the behaviour being ruled out.
    let mut frames = Vec::new();
    while let Some(f) = next_frame(&mut ws, Duration::from_secs(2)).await {
        frames.push(f);
    }
    assert_eq!(
        frames.len(),
        3,
        "expected one diff per write, got {}: {frames:#?}",
        frames.len()
    );
    for f in &frames {
        assert_eq!(f[3], json!("diff"), "a push is a diff frame: {f}");
        assert_eq!(f[1], Value::Null, "a push has no message ref: {f}");
        assert!(f[4]["diff"].is_object(), "the payload carries a diff: {f}");
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

    let html = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
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
