//! GH #1015 (OR-HV-61): after a crash the colony replays open deliveries, so
//! an outbound proxy can be handed the same message (same `Message.id`) twice.
//! A human must never get the same answer twice: the platform fake must see
//! exactly ONE send for two deliveries of one message — and a second, distinct
//! message must still go out.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::slack::cell::SlackCell;
use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::db::setup_slack_schema;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::mock_slack::MockSlack;
use tokio::sync::mpsc;

fn sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    )
}

fn reply(target: &str, chat_id: Value, text: &str) -> Message {
    let mut ctx = Map::new();
    ctx.insert("chat_id".into(), chat_id);
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/sender"))
        .context(ctx)
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": text }]
        })))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1015_telegram_replayed_reply_is_sent_once() {
    let ok = || MockResponse::ok_json(br#"{"ok":true,"result":{}}"#);
    let (addr, _j, cap) = start_mock_server_capturing(vec![ok(), ok(), ok()]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://x".into(),
    );
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    // A bare connection on purpose: the record must not depend on the factory's
    // schema setup having run.
    let mut db = DbConn::wrap(rusqlite::Connection::open_in_memory().unwrap(), None);

    let first = reply("/p", json!(12345), "only once");
    cell.handle(first.clone(), &sink, &mut db, &rc_tx).await;
    cell.handle(first, &sink, &mut db, &rc_tx).await; // the replay
    cell.handle(reply("/p", json!(12345), "next"), &sink, &mut db, &rc_tx)
        .await;

    let cap = cap.lock().await;
    let texts: Vec<Value> = cap
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["text"].clone())
        .collect();
    assert_eq!(
        texts,
        vec![json!("only once"), json!("next")],
        "one send per message id"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1015_slack_replayed_reply_is_posted_once() {
    let server = MockSlack::start().await.expect("fake");
    let params = SlackParams::parse(&json!({
        "app_token": "xapp-x", "bot_token": "xoxb-x",
        "emit_to": "/agent", "base_url": server.base_url()
    }))
    .unwrap();
    let mut cell = SlackCell::new(&params, SlackClient::new(&params).unwrap());
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_slack_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    let first = reply("/slack", json!("C1:100.000100"), "only once");
    cell.handle(first.clone(), &sink, &mut db, &rc_tx).await;
    cell.handle(first, &sink, &mut db, &rc_tx).await; // the replay
    cell.handle(
        reply("/slack", json!("C1:100.000100"), "next"),
        &sink,
        &mut db,
        &rc_tx,
    )
    .await;

    let posts = server.posts().await;
    let texts: Vec<Option<&str>> = posts.iter().map(|p| p.text()).collect();
    assert_eq!(
        texts,
        vec![Some("only once"), Some("next")],
        "one post per message id"
    );
}
