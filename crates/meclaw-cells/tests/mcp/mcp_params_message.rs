//! Phase-16 β4: `mcp` runtime params-overlay.
//!
//! Mutable: `external_timeout_ms` (path A — handle side, the next `call_tool`
//! uses it; the I/O-task has no live-rereadable value, mcp structural subtlety)
//! and `query_timeout_ms` (path C — DbConn). Immutable: `endpoint` + `auth`
//! (credential/identity).
//!
//! Strong behavioral live receipt (path A): a cell built with a 60 s
//! `external_timeout_ms` gets a params-update lowering it to 100 ms; a
//! subsequent tool-call to a black-hole endpoint then returns `provider_timeout`
//! within a 2 s test deadline — only possible if the lowered timeout took
//! effect LIVE (otherwise the call would block ~60 s and the deadline fails).

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn params_msg(body: meclaw_core::serde_json::Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(body))
        .build()
}

fn tool_call_msg() -> meclaw_core::Message {
    let inner = json!({"name": "echo", "arguments": {}}).to_string();
    MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": inner, "id": "call_1"}
            ]
        })))
        .build()
}

fn sink_for(msg: &meclaw_core::Message, tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/mcp"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    )
}

/// Black-hole TCP listener: accepts connections but never writes a byte.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_timeout_ms_lowered_live_then_tool_call_times_out() {
    let addr = start_blackhole().await;
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));
    // Birth external_timeout_ms = 60 s — far beyond the 2 s test deadline.
    let client = McpClient::new(&format!("http://{addr}/"), None).unwrap();
    let mut cell = McpCell::new(client, 60_000, 5_000, "main_mcp".into());

    // 1) params-update lowers external_timeout_ms to 100 ms (path A, live).
    let pmsg = params_msg(json!({ "params": { "external_timeout_ms": 100 } }));
    let (ptx, mut prx) = mpsc::channel::<CellEmission>(8);
    let psink = sink_for(&pmsg, ptx);
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    cell.handle(pmsg, &psink, &mut db, &rc_tx).await;
    drop(psink);
    assert!(prx.recv().await.is_none(), "params-only must not emit");

    // 2) tool-call to the black-hole — must return provider_timeout within 2 s,
    //    which is only possible if the 100 ms timeout is now live (else ~60 s).
    let tmsg = tool_call_msg();
    let (ttx, mut trx) = mpsc::channel::<CellEmission>(8);
    let tsink = sink_for(&tmsg, ttx);
    let em = tokio::time::timeout(Duration::from_secs(2), async {
        cell.handle(tmsg, &tsink, &mut db, &rc_tx).await;
        trx.recv().await.expect("emission expected")
    })
    .await
    .expect("tool-call must finish within 2 s (lowered external_timeout_ms is live)");
    assert_eq!(
        em.content["header"]["error_code"], "provider_timeout",
        "expected provider_timeout, got header: {}",
        em.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_query_timeout_live_and_persisted() {
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_millis(5000)));
    let client = McpClient::new("http://127.0.0.1:1/", None).unwrap();
    let mut cell = McpCell::new(client, 30_000, 5_000, "main_mcp".into());

    let msg = params_msg(json!({ "params": { "query_timeout_ms": 1234 } }));
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink_for(&msg, tx);
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    assert_eq!(
        db.query_timeout(),
        Some(Duration::from_millis(1234)),
        "DbConn must carry the new query_timeout live (path C)"
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
    drop(sink);
    assert!(rx.recv().await.is_none(), "params-only must not emit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_immutable_endpoint_and_auth_rejected() {
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_millis(5000)));
    let client = McpClient::new("http://127.0.0.1:1/", None).unwrap();
    let mut cell = McpCell::new(client, 30_000, 5_000, "main_mcp".into());
    let (rc_tx, _rc_rx) = mpsc::channel(1);

    for body in [
        json!({ "params": { "endpoint": "http://evil/" } }),
        json!({ "params": { "auth": { "bearer": "leaked" } } }),
    ] {
        let msg = params_msg(body);
        let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
        let sink = sink_for(&msg, tx);
        cell.handle(msg, &sink, &mut db, &rc_tx).await;
        drop(sink);
        let em = rx.recv().await.expect("immutable reject must emit");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }
}

#[test]
fn restore_replays_timeout_overlay_over_birth() {
    use meclaw_cells::mcp::params::McpOverlay;
    use meclaw_cells::params_overlay;
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES ('external_timeout_ms', '111', 100)",
        [],
    )
    .unwrap();
    let birth =
        json!({ "endpoint": "http://x/", "external_timeout_ms": 30000, "query_timeout_ms": 5000 });
    let eff = params_overlay::restore::<McpOverlay>(&conn, &birth).unwrap();
    assert_eq!(eff.external_timeout_ms, 111);
    assert_eq!(eff.query_timeout_ms, 5000);
}
