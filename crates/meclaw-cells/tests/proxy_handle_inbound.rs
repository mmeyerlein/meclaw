//! Phase-10-C T12: handle (Inbound-Sink Happy-Path). Pure Sink — kein
//! OutputSink-Emit bei Erfolg. T13 deckt die Fehlerpfade ab
//! (`missing_chat_id`/`send_failed`/`missing_assistant_turn`) plus den
//! `/colony/dead_letters`-Fallback bei fehlendem `reply_to`.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::Map;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::time::Duration;
use tokio::sync::mpsc;

/// Build a `context` compartment carrying `chat_id` (the standard-header
/// convention slot per overview § Standard-Header-Konvention).
fn context_with_chat_id(chat_id: i64) -> Map<String, meclaw_core::serde_json::Value> {
    let mut m = Map::new();
    m.insert("chat_id".into(), json!(chat_id));
    m
}

fn empty_sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_happy_path_calls_send_message_no_emit() {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":{}}"#)])
            .await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );

    // Befund 1: chat_id arrives in the `context` compartment (the promotion edge
    // `proxy → persona` lifts it there per the bot-basic LIFT design); the body
    // carries NO `header` block — colony's `split_content_header` strips any
    // emitted `content.header` into `hop`, which dies at the next emission, so a
    // routed reply leg only ever sees `context`.
    let msg = MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context_with_chat_id(12345))
        .body(Body::Inline(json!({
            "messages": [
                { "origin": "user", "type": "text", "text": "ignored" },
                { "origin": "assistant", "type": "text", "text": "echo reply" }
            ]
        })))
        .build();

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    // 1. sendMessage am Mock empfangen mit chat_id + text.
    let cap = cap.lock().await;
    assert_eq!(cap.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&cap[0].body).unwrap();
    assert_eq!(body["chat_id"], 12345);
    assert_eq!(
        body["text"], "echo reply",
        "extrahierter letzter assistant-Turn"
    );

    // 2. Pure Sink: KEIN OutputSink-Emit im Happy-Path.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err(),
        "Pure-Sink-Disziplin: kein Emit bei Erfolg"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_without_chat_id_emits_missing_chat_id_error_reply() {
    // Kein mock_http nötig — handle erreicht den sendMessage-Call nicht.
    let client = TelegramClient::new("http://unused", "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    // No chat_id anywhere (empty context) → missing_chat_id.
    let msg = MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .body(Body::Inline(json!({
            "messages": [
                { "origin": "assistant", "type": "text", "text": "reply" }
            ]
        })))
        .build();

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_millis(500), out_rx.recv())
        .await
        .expect("expected error-reply")
        .expect("emission");
    assert_eq!(em.target.as_str(), "/sender", "Target = reply_to");
    let header = em
        .content
        .get("header")
        .and_then(|v| v.as_object())
        .unwrap();
    assert_eq!(
        header.get("error_code").and_then(|v| v.as_str()),
        Some("missing_chat_id")
    );

    // Nicht-Konversations-Origin: messages[] ist leer (kein user/assistant-Turn).
    let messages = em
        .content
        .get("messages")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(
        messages.is_empty(),
        "Error-Reply trägt KEINEN Konversations-Turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_send_failed_emits_send_failed_error_reply() {
    let (addr, _j, _c) = start_mock_server_capturing(vec![MockResponse::server_error()]).await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let msg = MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context_with_chat_id(12345))
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": "x" }]
        })))
        .build();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("expected error-reply")
        .expect("emission");
    assert_eq!(em.target.as_str(), "/sender");
    let header = em
        .content
        .get("header")
        .and_then(|v| v.as_object())
        .unwrap();
    assert_eq!(
        header.get("error_code").and_then(|v| v.as_str()),
        Some("send_failed")
    );
    let messages = em
        .content
        .get("messages")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(messages.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_without_reply_to_emits_to_own_target_not_colony_dead_letters() {
    // W2 (Ruling A1): missing_chat_id ohne reply_to emittiert NICHT mehr an
    // den READ-Endpoint /colony/dead_letters, sondern als normale Emission an
    // das eigene `msg.target` (/p) — matcht keine Out-Edge ⇒ no_route in der DLQ.
    let client = TelegramClient::new("http://unused", "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let msg = MessageBuilder::new(Path::new("/p"))
        .body(Body::Inline(json!({
            "messages": [{"origin":"assistant","type":"text","text":"x"}]
        })))
        .build();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_millis(500), out_rx.recv())
        .await
        .expect("error-reply")
        .expect("emission");
    assert_eq!(
        em.target.as_str(),
        "/p",
        "no reply_to → emits to own msg.target (no /colony/dead_letters READ-endpoint)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_without_assistant_turn_emits_missing_assistant_turn_error_reply() {
    // W12: chat_id vorhanden, aber messages[] hat KEINEN assistant-text-
    // Turn. Kein silent drop (frühere Plan-Version war falsch) — stattdessen
    // Error-Reply analog W5/W6.
    let client = TelegramClient::new("http://unused", "T").unwrap();
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let msg = MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context_with_chat_id(12345))
        .body(Body::Inline(json!({
            "messages": [
                // Nur user-Turn (z.B. Topologie hat den assistant-Turn vergessen)
                { "origin": "user", "type": "text", "text": "no assistant here" }
            ]
        })))
        .build();
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_millis(500), out_rx.recv())
        .await
        .expect("expected W12 error-reply, got silent-drop (Bug!)")
        .expect("emission");
    assert_eq!(em.target.as_str(), "/sender");
    let header = em
        .content
        .get("header")
        .and_then(|v| v.as_object())
        .unwrap();
    assert_eq!(
        header.get("error_code").and_then(|v| v.as_str()),
        Some("missing_assistant_turn")
    );
    let messages = em
        .content
        .get("messages")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(
        messages.is_empty(),
        "W12-Reply trägt KEINEN Konversations-Turn"
    );
}
