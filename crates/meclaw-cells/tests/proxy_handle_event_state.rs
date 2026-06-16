//! Phase-10-C T11: handle_event persistiert Cursor VOR Emit (Phase-5-Kanon).
//! Header chat_id/user_id/platform/message_id (Spec cell-types.md Z.370).

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::{load_offset, setup_proxy_schema};
use meclaw_cells::proxy::io::ProxyEvent;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellEmission, OriginSink, Path};
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_persists_cursor_before_emit() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);

    let client = TelegramClient::new("http://x", "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        10000,
        5000,
        "https://api.telegram.org".into(),
    );
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/p"), 64);

    cell.handle_event(
        ProxyEvent::UserMessage {
            update_id: 42,
            chat_id: 100,
            user_id: Some(200),
            message_id: Some(7),
            text: "hello".into(),
        },
        &sink,
        &mut db,
    )
    .await;

    // 1. Cursor persistiert: load_offset == 43.
    let persisted = db.call(|c| load_offset(c)).await.unwrap();
    assert_eq!(persisted, 43);

    // 2. Emission abgesetzt: target=/dst, content trägt Header + UBF.
    let em = origin_rx.recv().await.expect("emission");
    assert_eq!(em.target.as_str(), "/dst");
    assert_eq!(em.parent_message_id, None, "Source-Emission via OriginSink");

    let content = em.content.as_object().expect("content object");
    let header = content
        .get("header")
        .and_then(|v| v.as_object())
        .expect("header");
    assert_eq!(header.get("chat_id").and_then(|v| v.as_i64()), Some(100));
    assert_eq!(header.get("user_id").and_then(|v| v.as_i64()), Some(200));
    assert_eq!(
        header.get("platform").and_then(|v| v.as_str()),
        Some("telegram")
    );
    assert_eq!(header.get("message_id").and_then(|v| v.as_i64()), Some(7));

    let messages = content
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages");
    assert_eq!(messages.len(), 1);
    let turn = &messages[0];
    assert_eq!(turn.get("origin").and_then(|v| v.as_str()), Some("user"));
    assert_eq!(turn.get("type").and_then(|v| v.as_str()), Some("text"));
    assert_eq!(turn.get("text").and_then(|v| v.as_str()), Some("hello"));
}
