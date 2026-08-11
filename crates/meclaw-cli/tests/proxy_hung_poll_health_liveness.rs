//! Issue #7 fixture: a silently hung proxy must stop looking like an idle one.
//!
//! Two `proxy` cells boot in the same colony against two mock Telegram servers.
//! One answers its long polls; the other is a half-dead peer — it sends the
//! response HEAD and then never writes the announced body, holding the socket
//! open with no FIN and no RST. That is the shape that produced the incident
//! behind this issue: over 15 minutes with the service `active`, `/health`
//! answering 200 and an empty log, while not a single poll completed.
//!
//! The proof is the pair. Both lanes run in the same window, under the same
//! colony, and `/health` has to tell them apart: the healthy lane carries a
//! fresh last-successful-round-trip mark, the hung lane carries none at all.
//! Before this change `/health` could not say anything about either.

use meclaw_cli::{Cli, run_with_hooks};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing_hanging_body};
use std::net::SocketAddr;
use std::time::Duration;

/// Telegram's answer to `getUpdates` when nothing is waiting — a successful
/// round trip that carries no event. Exactly the case that makes event traffic
/// useless as a liveness signal.
const EMPTY_UPDATES: &[u8] = br#"{"ok":true,"result":[]}"#;

/// Writes a `proxy` cell directory pointed at `base_url`.
///
/// `long_poll_request_secs = 0` with `long_poll_timeout_ms = 500` keeps the W7
/// tripwire (client timeout > server wait) and gives the I/O task a 500 ms outer
/// hang deadline, so the hung lane cycles through its backoff several times
/// inside the test window instead of sitting in one 65 s poll.
fn write_proxy_cell(root: &std::path::Path, name: &str, base_url: &str) {
    let dir = root.join("main").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = serde_json::json!({
        "cell": { "type": "proxy", "timeout": -1 },
        "params": {
            "bot_token": "test-token",
            "emit_to": "/main",
            "base_url": base_url,
            "long_poll_request_secs": 0,
            "long_poll_timeout_ms": 500,
        },
        // Minimal contract: the bootstrap demands version + settings + consumes
        // on every non-hive cell.
        "contract": { "version": "1.0.0", "settings": {}, "consumes": {} }
    });
    std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
}

/// Reads `/health` and returns `(path, last_success_secs)` for every reported
/// I/O task.
async fn health_marks(addr: SocketAddr) -> Vec<(String, Option<u64>)> {
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .expect("/health answers")
        .json()
        .await
        .expect("/health speaks JSON");
    assert_eq!(body["status"], "ok");
    body["io_liveness"]
        .as_array()
        .expect("/health must carry the io_liveness section")
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_string(),
                e["last_success_secs"].as_u64(),
            )
        })
        .collect()
}

/// The mark reported for `path`. Panics when the cell is missing entirely —
/// an I/O task that never announced itself would be invisible, which is the
/// state this issue exists to end.
fn mark_for(marks: &[(String, Option<u64>)], path: &str) -> Option<u64> {
    marks
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("{path} must appear on /health at all, /health said {marks:?}"))
        .1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_proxy_poll_is_visible_on_health_while_a_healthy_one_stays_fresh() {
    // The healthy peer: answers every poll, with a small delay so the lane
    // cycles at a sane rate instead of spinning.
    let (live_addr, _live_join, _live_cap) = start_mock_server_capturing_hanging_body(
        0,
        vec![MockResponse::ok_json(EMPTY_UPDATES).with_delay(Duration::from_millis(50))],
    )
    .await;
    // The half-dead peer: EVERY connection gets the head and then silence.
    let (hung_addr, _hung_join, _hung_cap) = start_mock_server_capturing_hanging_body(
        usize::MAX,
        vec![MockResponse::ok_json(EMPTY_UPDATES)],
    )
    .await;

    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(main_dir.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    // No edges: an edge-less cell keeps the instantiation grace and boots
    // active, which is all these two need.
    write_proxy_cell(td.path(), "live", &format!("http://{live_addr}"));
    write_proxy_cell(td.path(), "hung", &format!("http://{hung_addr}"));

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let cli = Cli {
        root: td.path().into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse::<SocketAddr>().unwrap()),
        daemon: true,
        validate: false,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = addr_rx.await.unwrap();

    // Wait for the healthy lane to record its first successful round trip.
    // 30 s failure marker (convention); the healthy poll answers in ~50 ms.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let marks = health_marks(addr).await;
        if marks.iter().any(|(p, s)| p == "/live" && s.is_some()) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the healthy proxy never recorded a successful round trip; /health said {marks:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Semantic discriminator: three times the hung lane's 500 ms outer deadline.
    // Long enough that a lane which could recover would have, short enough that
    // the healthy lane's mark (refreshed every ~50 ms) is still nearly zero.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let marks = health_marks(addr).await;

    let live = mark_for(&marks, "/live");
    let hung = mark_for(&marks, "/hung");
    assert!(
        live.is_some_and(|secs| secs <= 2),
        "the healthy lane must keep its mark fresh — /health said {marks:?}"
    );
    assert!(
        hung.is_none(),
        "the hung lane completed no round trip in the whole window, so its mark \
         must stay null — /health said {marks:?}"
    );

    shutdown_tx.send(()).unwrap();
    join.await.unwrap().unwrap();
}
