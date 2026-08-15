//! Phase-10-C Demo Test 1. End-to-End: Mock-Telegram liefert ein Update →
//! ProxyCell pollt via Long-Poll → handle_event persistiert Cursor →
//! OriginSink emittiert an `params.emit_to` (= `/sink`) → CaptureCell
//! receives. Anti-cascade discipline (phase-6.5 lesson): `/sink` MUST be
//! registered before the probe / before the proxy registration.

use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10c_demo_outbound_end_to_end() {
    let h = ColonyHandle::new();
    let (recv_tx, mut recv_rx) = mpsc::channel::<Message>(32);

    // 1. Anti-cascade: /sink FIRST.
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    // 2. Mock Telegram: first response 1 update, second empty (the long-poll loop ticks).
    let r1 = MockResponse::ok_json(
        br#"{"ok":true,"result":[
        {"update_id":42,"message":{"message_id":7,"chat":{"id":100},"from":{"id":200},"text":"hallo"}}
    ]}"#,
    );
    let r2 = MockResponse::ok_json(br#"{"ok":true,"result":[]}"#);
    let (addr, _mock_join, _cap) = start_mock_server_capturing(vec![r1, r2]).await;

    // 3. ProxyCell registrieren via Factory.
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("proxy");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(ProxyCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/proxy"),
            json!({
                "bot_token": "T", "emit_to": "/sink",
                "base_url": format!("http://{addr}"),
                "long_poll_timeout_ms": 2000, "long_poll_request_secs": 1,
            }),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/proxy"), spawned).await;
    // W2 (A1): /proxy emission to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/proxy"),
        Path::new("/sink"),
    )
    .await;

    // 4. Wait for the receipt (long poll → event → OriginSink → routing → /sink).
    let m = tokio::time::timeout(Duration::from_secs(30), recv_rx.recv())
        .await
        .expect("no user turn at /sink")
        .expect("recv");

    // 5. Header (chat_id/user_id/platform/message_id) + UBF-Body (User-Turn).
    assert_eq!(
        m.headers.hop.get("chat_id").and_then(|v| v.as_i64()),
        Some(100)
    );
    assert_eq!(
        m.headers.hop.get("user_id").and_then(|v| v.as_i64()),
        Some(200)
    );
    assert_eq!(
        m.headers.hop.get("platform").and_then(|v| v.as_str()),
        Some("telegram")
    );
    assert_eq!(
        m.headers.hop.get("message_id").and_then(|v| v.as_i64()),
        Some(7)
    );

    match &m.body {
        meclaw_core::Body::Inline(v) => {
            let messages = v
                .get("messages")
                .and_then(|x| x.as_array())
                .expect("messages");
            assert_eq!(messages.len(), 1);
            assert_eq!(
                messages[0].get("origin").and_then(|x| x.as_str()),
                Some("user")
            );
            assert_eq!(
                messages[0].get("type").and_then(|x| x.as_str()),
                Some("text")
            );
            assert_eq!(
                messages[0].get("text").and_then(|x| x.as_str()),
                Some("hallo")
            );
        }
        _ => panic!("expected inline body"),
    }
    // Source-Emission: parent_message_id == None.
    assert_eq!(m.parent_message_id, None);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_10c_demo_inbound_end_to_end() {
    let h = ColonyHandle::new();

    // Mock Telegram: an empty getUpdates result (no user updates) + one 200 OK
    // for the inbound probe's sendMessage call. We do not know in which order
    // getUpdates and sendMessage arrive — so we queue N=4 empty getUpdates
    // responses and one 200 OK; the mock picks FIFO; the test deadline is 3s.
    let r_empty = || MockResponse::ok_json(br#"{"ok":true,"result":[]}"#);
    let r_ok = MockResponse::ok_json(br#"{"ok":true,"result":{}}"#);
    let (addr, _mock_join, cap) =
        start_mock_server_capturing(vec![r_empty(), r_empty(), r_empty(), r_empty(), r_ok]).await;

    // ProxyCell with emit_to=/sink (for hypothetical outbounds — we only test
    // inbound though). Anti-cascade all the same: /sink exists.
    let (recv_tx, _recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("proxy");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(ProxyCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/proxy"),
            json!({
                "bot_token": "T", "emit_to": "/sink",
                "base_url": format!("http://{addr}"),
                "long_poll_timeout_ms": 2000, "long_poll_request_secs": 1,
                "send_timeout_ms": 5000,
            }),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/proxy"), spawned).await;
    // W2 (A1): /proxy emission to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/proxy"),
        Path::new("/sink"),
    )
    .await;

    // Inbound probe: an assistant turn with chat_id in the `context` compartment
    // sent to /proxy (finding 1 — reply routing reads chat_id from the persistent
    // context, not from a body `header` block, which would be stripped into the
    // hop compartment on emission).
    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("chat_id".into(), json!(9999));
    h.send(
        MessageBuilder::new(Path::new("/proxy"))
            .context(ctx)
            .body(Body::Inline(json!({
                "messages": [
                    { "origin": "user", "type": "text", "text": "ignored" },
                    { "origin": "assistant", "type": "text", "text": "antwort an telegram" }
                ]
            })))
            .build(),
    )
    .await;

    // Wait for the sendMessage POST — we poll the capture vec until a POST is in
    // it (3s deadline).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut send_msg_seen = false;
    while tokio::time::Instant::now() < deadline && !send_msg_seen {
        let snap = cap.lock().await.clone();
        if let Some(req) = snap
            .iter()
            .find(|r| r.method == "POST" && r.path.contains("sendMessage"))
        {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["chat_id"], 9999);
            assert_eq!(body["text"], "antwort an telegram");
            send_msg_seen = true;
        } else {
            drop(snap);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    assert!(send_msg_seen, "no sendMessage received at the mock");

    h.shutdown().await;
}
