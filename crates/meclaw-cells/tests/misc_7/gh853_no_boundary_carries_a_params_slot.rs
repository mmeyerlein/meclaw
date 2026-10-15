//! GH #853: no boundary carries a `params` slot into the colony.
//!
//! A model changes by a params-only message from the operator's edge; a
//! conversation cannot change the model it talks to. The llm cell enforces the
//! second half itself (a turn's `params` is not applied, see the sibling lock).
//! This file pins the first half at the two surfaces a stranger can write to:
//!
//! - the peer mount (`proxy` on `platform: "meclaw"`): the lane projection
//!   refuses a top-level `params` slot even when a declaration names it —
//!   like `attachments`, it is refused before the allow-list is consulted;
//! - `web`: a visitor's event leaves as `header` + `messages` + `event`, and
//!   whatever the visitor typed stays inside `event.value` — never a
//!   top-level `params` slot.
//!
//! `POST /messages` is the operator's own edge (trusted, documented in
//! `docs/cell-types.md` § llm) and is deliberately not locked here.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::proxy::meclaw::lanes::project_body;
use meclaw_cells::proxy::meclaw::params::Lane;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, Path};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[test]
fn a_lane_that_names_params_still_lets_no_params_slot_cross() {
    let lane = Lane {
        route: "in_turn".into(),
        fields: vec!["params".into(), "messages[].text".into()],
        context: vec![],
        because: "a declaration that tries to open the model to a peer".into(),
    };
    let r = project_body(
        &lane,
        &json!({"params": {"model": "a-peer-picked-this"},
                "messages": [{"text": "hi"}]}),
    )
    .expect_err("a params slot never crosses a colony boundary");
    assert_eq!(r.error_code, "lane_body_unsupported");
    assert!(r.detail.contains("params"), "{}", r.detail);
}

#[test]
fn a_lane_without_params_refuses_it_as_an_unnamed_field() {
    let lane = Lane {
        route: "in_turn".into(),
        fields: vec!["messages[].text".into()],
        context: vec![],
        because: "a turn".into(),
    };
    assert!(project_body(&lane, &json!({"params": {}, "messages": []})).is_err());
}

fn seed(cell_dir: &std::path::Path) {
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
            r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#,
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

const MOUNT: &str = "screen";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_event_never_becomes_a_params_slot() {
    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    seed(&cell_dir);

    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({ "mount": MOUNT }),
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
        sender: _mailbox,
        stop_tx: _stop,
        ..
    } = spawned
    else {
        panic!("Active");
    };
    wait_for_mount(&surfaces, MOUNT).await;
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    let port = addr.port();
    let deadline = Instant::now() + Duration::from_secs(30);
    let page = loop {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/")).await
            && r.status().is_success()
        {
            break r.text().await.expect("text");
        }
        assert!(Instant::now() < deadline, "the cell never served its page");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let marker = "data-phx-session=\"";
    let start = page.find(marker).expect("token") + marker.len();
    let end = start + page[start..].find('"').expect("quote");
    let token = page[start..end].to_string();

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
    .expect("join");
    let _ = ws.next().await.expect("open").expect("frame");

    // The visitor names the slot outright, as the event and inside its value.
    ws.send(WsMessage::Text(
        json!(["1", "9", topic, "event", {"event": "params",
               "value": {"params": {"model": "visitor-picked-this"}}}])
        .to_string()
        .into(),
    ))
    .await
    .expect("send");

    let emission = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("the event reaches the out-edges")
        .expect("emission");
    let body = emission.content.as_object().expect("object body");
    assert!(
        !body.contains_key("params"),
        "a visitor's event never carries a top-level params slot: {body:?}"
    );
    assert_eq!(
        emission.content["event"]["value"]["params"]["model"], "visitor-picked-this",
        "what the visitor typed stays inside event.value"
    );
    join.abort();
}
