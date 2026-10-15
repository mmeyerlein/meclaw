//! GH #1043 (review Z-M3, finding 2): an outbound chat proxy books a message
//! id BEFORE the platform call (GH #1015, at-most-once). A replay of that
//! message must answer what the original answered — never less:
//!
//! - the original send failed → it said `send_failed`; the replay says it again
//!   (and sends nothing);
//! - the original was booked but its result never recorded (a crash between
//!   the booking and the platform's answer) → the replay cannot know whether
//!   a human got it, says `send_failed` with the reason `unconfirmed`, and
//!   sends nothing;
//! - the original send went out → the replay stays silent, like the original.
//!
//! Before: every replay was silent, so a message whose send failed, or whose
//! colony died before `send_failed` reached the log, was gone without anyone
//! hearing of it — against the promise in `proxy::consumed` ("visible to the
//! agent tree as `send_failed` or a gap").

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
use std::time::Duration;
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

/// The next emission's `(error_code, detail)`, or `None` when the cell stays
/// silent (100 ms: a discriminator for "nothing was emitted", the cell's
/// handler has already returned when this is called).
async fn next(rx: &mut mpsc::Receiver<CellEmission>) -> Option<(String, String)> {
    let em = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .ok()??;
    Some((
        em.content["header"]["error_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        em.content["meta"]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    ))
}

/// A crash between the booking and the platform's answer leaves the id
/// booked and nothing recorded about the send — the row an older binary
/// wrote looks the same.
async fn book_without_result(db: &mut DbConn, id: String) {
    db.call(move |c| {
        c.execute_batch(meclaw_cells::proxy::consumed::CONSUMED_DDL)?;
        c.execute("INSERT INTO consumed (message_id) VALUES (?1)", [id])
    })
    .await
    .unwrap();
}

fn telegram(addr: std::net::SocketAddr) -> ProxyCell {
    ProxyCell::new(
        TelegramClient::new(&format!("http://{addr}"), "T").unwrap(),
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://x".into(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1043_telegram_a_replay_answers_like_the_original() {
    let ok = || MockResponse::ok_json(br#"{"ok":true,"result":{}}"#);
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::server_error(), ok(), ok()]).await;
    let mut cell = telegram(addr);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let mut db = DbConn::wrap(rusqlite::Connection::open_in_memory().unwrap(), None);

    // 1. The send fails: the original says so, and so does its replay.
    let failed = reply("/p", json!(1), "fails");
    cell.handle(failed.clone(), &sink, &mut db, &rc_tx).await;
    let original = next(&mut rx)
        .await
        .expect("the original reports the failure");
    assert_eq!(original.0, "send_failed");
    cell.handle(failed, &sink, &mut db, &rc_tx).await;
    assert_eq!(
        next(&mut rx).await,
        Some(original),
        "the replay of a failed send reports what the original reported"
    );

    // 2. Booked, then the colony died before the answer was recorded.
    let unconfirmed = reply("/p", json!(1), "never confirmed");
    book_without_result(&mut db, unconfirmed.id.to_string()).await;
    cell.handle(unconfirmed, &sink, &mut db, &rc_tx).await;
    let (code, detail) = next(&mut rx)
        .await
        .expect("an unconfirmed send is reported, not swallowed");
    assert_eq!(code, "send_failed");
    assert!(detail.starts_with("unconfirmed"), "{detail}");

    // 3. Sent: the original is silent, and so is its replay.
    let sent = reply("/p", json!(1), "goes out");
    cell.handle(sent.clone(), &sink, &mut db, &rc_tx).await;
    assert_eq!(next(&mut rx).await, None, "a sent reply emits nothing");
    cell.handle(sent, &sink, &mut db, &rc_tx).await;
    assert_eq!(next(&mut rx).await, None, "nor does its replay");

    // Never a second platform call: one for the failure, one for the send.
    let texts: Vec<Value> = cap
        .lock()
        .await
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["text"].clone())
        .collect();
    assert_eq!(texts, vec![json!("fails"), json!("goes out")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1043_slack_a_replay_answers_like_the_original() {
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
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let chat = || json!("C1:100.000100");

    // 1. The post fails: the original says so, and so does its replay.
    server.fail_post_with(500, "internal_error").await;
    let failed = reply("/slack", chat(), "fails");
    cell.handle(failed.clone(), &sink, &mut db, &rc_tx).await;
    let original = next(&mut rx)
        .await
        .expect("the original reports the failure");
    assert_eq!(original.0, "send_failed");
    let posts_after_failure = server.posts().await.len();
    cell.handle(failed, &sink, &mut db, &rc_tx).await;
    assert_eq!(
        next(&mut rx).await,
        Some(original),
        "the replay of a failed post reports what the original reported"
    );
    assert_eq!(
        server.posts().await.len(),
        posts_after_failure,
        "the replay does not post"
    );

    // 2. Booked, then the colony died before the answer was recorded.
    let unconfirmed = reply("/slack", chat(), "never confirmed");
    book_without_result(&mut db, unconfirmed.id.to_string()).await;
    cell.handle(unconfirmed, &sink, &mut db, &rc_tx).await;
    let (code, detail) = next(&mut rx)
        .await
        .expect("an unconfirmed post is reported, not swallowed");
    assert_eq!(code, "send_failed");
    assert!(detail.starts_with("unconfirmed"), "{detail}");
    assert_eq!(server.posts().await.len(), posts_after_failure);
}
