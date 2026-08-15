//! Phase-16 β5: `proxy` runtime params-overlay — the richest surface, all three
//! propagation ways.
//!
//! Path A: `send_timeout_ms` — handle side; the next `sendMessage` uses it.
//! Path B: `long_poll_timeout_ms` / `long_poll_request_secs` — signalled to the
//! I/O-task via `ProxyReconfig::SetPolling` (next poll uses them).
//! Path C: `query_timeout_ms` — DbConn (cell.db ops).
//! Immutable: `bot_token`, `emit_to`, `base_url` (credential/identity/endpoint).
//!
//! Behavioral live receipt (path A): a cell built with a 60 s `send_timeout_ms`
//! gets a params-update lowering it to 100 ms; a subsequent inbound message
//! whose `sendMessage` hits a black-hole endpoint then emits `send_failed`
//! within a 2 s deadline — only possible if the lowered timeout is live.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::setup_proxy_schema;
use meclaw_cells::proxy::io::ProxyReconfig;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::persist::open_or_create_cell_db;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
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

fn params_msg(body: meclaw_core::serde_json::Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .body(Body::Inline(body))
        .build()
}

fn inbound_msg(chat_id: i64) -> meclaw_core::Message {
    let mut ctx = Map::new();
    ctx.insert("chat_id".into(), json!(chat_id));
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(ctx)
        .body(Body::Inline(json!({
            "messages": [
                { "origin": "assistant", "type": "text", "text": "reply" }
            ]
        })))
        .build()
}

async fn start_blackhole() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                use tokio::io::AsyncReadExt;
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
            });
        }
    });
    addr
}

fn cell_db(tmp: &TempDir) -> DbConn {
    let conn = open_or_create_cell_db(&tmp.path().join("cell.db")).unwrap();
    setup_proxy_schema(&conn).unwrap();
    DbConn::wrap(conn, Some(Duration::from_millis(5000)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_timeout_ms_lowered_live_then_send_fails_fast() {
    let addr = start_blackhole().await;
    let tmp = TempDir::new().unwrap();
    let mut db = cell_db(&tmp);
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    // Birth send_timeout_ms = 60 s.
    let mut cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        60_000,
        5000,
        "https://api.telegram.org".into(),
    );
    let (rc_tx, _rc_rx) = mpsc::channel::<ProxyReconfig>(8);

    // 1) params-update lowers send_timeout_ms to 100 ms (path A, live).
    let (ptx, mut prx) = mpsc::channel::<CellEmission>(8);
    let psink = sink(ptx);
    cell.handle(
        params_msg(json!({ "params": { "send_timeout_ms": 100 } })),
        &psink,
        &mut db,
        &rc_tx,
    )
    .await;
    drop(psink);
    assert!(prx.recv().await.is_none(), "params-only must not emit");

    // 2) inbound → sendMessage to black-hole must emit send_failed within 2 s.
    let (otx, mut orx) = mpsc::channel::<CellEmission>(8);
    let osink = sink(otx);
    let em = tokio::time::timeout(Duration::from_secs(2), async {
        cell.handle(inbound_msg(12345), &osink, &mut db, &rc_tx)
            .await;
        orx.recv().await.expect("send_failed expected")
    })
    .await
    .expect("send must fail fast (lowered send_timeout_ms is live)");
    assert_eq!(em.content["header"]["error_code"], "send_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_signals_io_reconfig_setpolling() {
    let tmp = TempDir::new().unwrap();
    let mut db = cell_db(&tmp);
    let client = TelegramClient::new("http://127.0.0.1:1", "T").unwrap();
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
    let (rc_tx, mut rc_rx) = mpsc::channel::<ProxyReconfig>(8);

    let (otx, _orx) = mpsc::channel::<CellEmission>(8);
    let s = sink(otx);
    cell.handle(
        params_msg(
            json!({ "params": { "long_poll_timeout_ms": 40000, "long_poll_request_secs": 20 } }),
        ),
        &s,
        &mut db,
        &rc_tx,
    )
    .await;

    // Path B: the handler signalled the I/O task with the new poll config.
    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("SetPolling within 1 s")
        .unwrap();
    let ProxyReconfig::SetPolling {
        long_poll_timeout_ms,
        long_poll_request_secs,
        ..
    } = rc;
    assert_eq!(long_poll_timeout_ms, 40000);
    assert_eq!(long_poll_request_secs, 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_url_update_live_signals_new_url_to_io() {
    // Path B: base_url is mutable (a config URL, not a credential). A params update
    // changing it makes the handler signal the I/O-task with the new URL (which
    // rebuilds its client via with_base_url, reholding the immutable bot_token).
    let tmp = TempDir::new().unwrap();
    let mut db = cell_db(&tmp);
    let client = TelegramClient::new("https://api.telegram.org", "T").unwrap();
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
    let (rc_tx, mut rc_rx) = mpsc::channel::<ProxyReconfig>(8);

    let (otx, _orx) = mpsc::channel::<CellEmission>(8);
    let s = sink(otx);
    cell.handle(
        params_msg(json!({ "params": { "base_url": "http://127.0.0.1:9999" } })),
        &s,
        &mut db,
        &rc_tx,
    )
    .await;

    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("SetPolling within 1 s")
        .unwrap();
    let ProxyReconfig::SetPolling { base_url, .. } = rc;
    assert_eq!(
        base_url, "http://127.0.0.1:9999",
        "the new base_url must reach the I/O-task live (path B)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_query_timeout_live_and_persisted() {
    let tmp = TempDir::new().unwrap();
    let mut db = cell_db(&tmp);
    let client = TelegramClient::new("http://127.0.0.1:1", "T").unwrap();
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
    let (rc_tx, _rc_rx) = mpsc::channel::<ProxyReconfig>(8);

    let (otx, mut orx) = mpsc::channel::<CellEmission>(8);
    let s = sink(otx);
    cell.handle(
        params_msg(json!({ "params": { "query_timeout_ms": 1234 } })),
        &s,
        &mut db,
        &rc_tx,
    )
    .await;

    assert_eq!(
        db.query_timeout(),
        Some(Duration::from_millis(1234)),
        "path C live"
    );
    let persisted: String = db
        .call(|c| {
            c.query_row(
                "SELECT value FROM params WHERE key='query_timeout_ms'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await;
    assert_eq!(persisted, "1234");
    drop(s);
    assert!(orx.recv().await.is_none(), "params-only must not emit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_immutable_and_w7_rejected() {
    let tmp = TempDir::new().unwrap();
    let mut db = cell_db(&tmp);
    let client = TelegramClient::new("http://127.0.0.1:1", "T").unwrap();
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
    let (rc_tx, _rc_rx) = mpsc::channel::<ProxyReconfig>(8);

    for body in [
        json!({ "params": { "bot_token": "leaked" } }),
        json!({ "params": { "emit_to": "/evil" } }),
        // W7-tripwire: 1000 ms <= 30 s * 1000 → reject on merge.
        json!({ "params": { "long_poll_timeout_ms": 1000 } }),
    ] {
        let (otx, mut orx) = mpsc::channel::<CellEmission>(8);
        let s = sink(otx);
        cell.handle(params_msg(body), &s, &mut db, &rc_tx).await;
        drop(s);
        let em = orx.recv().await.expect("reject must emit");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }
}

#[test]
fn restore_replays_overlay_over_birth() {
    use meclaw_cells::params_overlay;
    use meclaw_cells::proxy::params::ProxyOverlay;
    let tmp = TempDir::new().unwrap();
    let conn = open_or_create_cell_db(&tmp.path().join("cell.db")).unwrap();
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES ('send_timeout_ms', '222', 100)",
        [],
    )
    .unwrap();
    let birth = json!({
        "bot_token": "T", "emit_to": "/dst",
        "long_poll_timeout_ms": 35000, "long_poll_request_secs": 30,
        "send_timeout_ms": 10000, "query_timeout_ms": 5000
    });
    let eff = params_overlay::restore::<ProxyOverlay>(&conn, &birth).unwrap();
    assert_eq!(eff.send_timeout_ms, 222);
    assert_eq!(eff.long_poll_timeout_ms, 35000);
}
