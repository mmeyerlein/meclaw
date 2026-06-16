//! Phase-10-C T5/T6: TelegramClient::get_updates + send_message gegen
//! Mock-Server. 200-OK mit gueltigem `result`-Array → Vec<ProxyEvent::
//! UserMessage>; sendMessage POST mit chat_id/text + Fehler-Klassifikation.

use meclaw_cells::proxy::io::ProxyEvent;
use meclaw_cells::proxy::telegram::{TelegramClient, TelegramError};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_updates_parses_text_messages_into_proxy_events() {
    let response_body = br#"{
      "ok": true,
      "result": [
        {"update_id": 42,
         "message": {"message_id": 7, "chat": {"id": 100}, "from": {"id": 200},
                     "text": "hello"}}
      ]
    }"#;
    let (addr, _join, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(response_body)]).await;
    let base_url = format!("http://{addr}");

    let client = TelegramClient::new(&base_url, "TOKEN").unwrap();
    let events = client
        .get_updates(0, 30, Duration::from_millis(5000))
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProxyEvent::UserMessage {
            update_id,
            chat_id,
            user_id,
            message_id,
            text,
        } => {
            assert_eq!(*update_id, 42);
            assert_eq!(*chat_id, 100);
            assert_eq!(*user_id, Some(200));
            assert_eq!(*message_id, Some(7));
            assert_eq!(text, "hello");
        }
    }

    // Sanity: Request hatte `offset=0&timeout=30`.
    let cap = captured.lock().await;
    assert!(cap[0].path.contains("offset=0"));
    assert!(cap[0].path.contains("timeout=30"));
    assert!(cap[0].path.contains("/botTOKEN/getUpdates"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_updates_skips_non_text_updates() {
    // Update ohne `message.text` (z.B. `edited_message` oder Sticker) → leer.
    let body = br#"{"ok": true, "result": [
        {"update_id": 99, "edited_message": {"chat":{"id":1}, "text":"x"}}
    ]}"#;
    let (addr, _j, _c) = start_mock_server_capturing(vec![MockResponse::ok_json(body)]).await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let events = c
        .get_updates(0, 30, Duration::from_millis(5000))
        .await
        .unwrap();
    assert!(events.is_empty(), "non-text updates must be filtered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_message_happy_path_posts_chat_id_and_text() {
    let (addr, _j, cap) = start_mock_server_capturing(vec![MockResponse::ok_json(
        br#"{"ok": true, "result": {}}"#,
    )])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    c.send_message(12345, "hallo welt", Duration::from_millis(5000))
        .await
        .unwrap();

    let cap = cap.lock().await;
    assert_eq!(cap[0].method, "POST");
    assert!(cap[0].path.contains("/botT/sendMessage"));
    let body: serde_json::Value = serde_json::from_slice(&cap[0].body).unwrap();
    assert_eq!(body["chat_id"], 12345);
    assert_eq!(body["text"], "hallo welt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_message_5xx_classified_transient() {
    let (addr, _j, _c) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    match c.send_message(1, "x", Duration::from_millis(5000)).await {
        Err(TelegramError::Transient(_)) => {}
        other => panic!("expected Transient, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_message_403_classified_permanent() {
    let (addr, _j, _c) = start_mock_server_capturing(vec![MockResponse {
        status: 403,
        body: b"forbidden".to_vec(),
        content_type: "text/plain".into(),
        delay: None,
    }])
    .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    match c.send_message(1, "x", Duration::from_millis(5000)).await {
        Err(TelegramError::Permanent(_)) => {}
        other => panic!("expected Permanent, got {other:?}"),
    }
}
