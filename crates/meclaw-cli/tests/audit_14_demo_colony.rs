//! Post-Phase-14-Audit (#8): `tests/fixtures/demo-colony` als gebauter E2E-Test.
//!
//! The manual 8-point demo from `examples/README.md` had zero automated
//! coverage until now — bootstrap changes could silently turn the committed
//! demo fixtures red. This test boots the COMMITTED example tree (copied into a
//! TempDir — examples/ stays read-only, no-delete policy), applies
//! `tests/fixtures/demo-mutation.json` verbatim via `POST /colony/mutations`
//! and proves `/echo` through positive receipts: a registry entry (bash cell) +
//! a message-log row with `to_path == "/echo"`.
//!
//! Harness form 1:1 as in `phase_12_b_demo.rs` (production `run_with_hooks`, no
//! test `ColonyHandle` wrapper).

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

/// Repo-relative path to `examples/<name>` (the crate sits two levels deeper).
fn examples_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies the committed example tree recursively into the TempDir root —
/// never boot in place (`colony.db`/`cell.db` are created at runtime).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_colony_boots_mutates_and_reaches_echo() {
    let td = tempfile::TempDir::new().unwrap();
    copy_dir_recursive(&examples_path("demo-colony"), td.path());

    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
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
        api: Some(bind),
        daemon: false,
        validate: false,
        validate_strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    // Step 1: boot receipt — /health answers, the example tree is loaded.
    let resp = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Step 2: apply the committed mutation file VERBATIM (no inline JSON —
    // exactly the file the README demo posts is the subject under test).
    let mutation: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(examples_path("demo-mutation.json")).unwrap(),
    )
    .unwrap();
    let resp = client
        .post(format!("http://{addr}/colony/mutations"))
        .json(&mutation)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        json["mutation"]["outcome"], "committed",
        "demo-mutation.json must commit against the committed template tree: {json}"
    );

    // Step 3: positive mutation receipt — the registry lists /echo as a bash
    // cell (the `echo` template is a bash cell, see examples/README.md § Layout).
    let resp = client
        .get(format!("http://{addr}/colony/registry"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    let entries = json["registry"].as_array().unwrap();
    let echo = entries
        .iter()
        .find(|e| e["path"] == "/echo")
        .unwrap_or_else(|| panic!("mutation must register /echo, registry: {entries:?}"));
    assert_eq!(echo["cell_type"], "bash");

    // Step 4: /echo is reachable — fire-and-forget 202, then a positive routing
    // receipt: a message-log row with to_path == "/echo" via /colony/trace.
    let body = serde_json::json!({
        "target": "/echo",
        "body": {
            "messages": [
                { "origin": "user", "type": "text", "text": "demo ping" }
            ]
        }
    });
    let resp = client
        .post(format!("http://{addr}/messages"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    let routed = poll_for_trace_to_path(&client, addr, "/echo").await;
    assert!(
        routed,
        "POST /messages target=/echo must surface as message-log row (to_path=/echo) within 2s"
    );

    // Secondary check (loud, not a proof): /echo must not land in the DLQ as an
    // UNREACHABLE target — that would reveal e.g. a CellInactive regression at
    // the mutation spawn (external sender → /echo dead-lettered).
    //
    // W2d (substrate, ruling 2026-06-12): the bash cell answers the "demo ping"
    // (NO tool_call) with an op error reply. Without `reply_to` it has, since
    // W2d, emitted that to its OWN path (`msg.target` = /echo) instead of the
    // `/colony/dead_letters` READ endpoint; it matches no out-edge ⇒ it
    // dead-letters (sender_path == /echo). That is the announced W2 regression
    // (op echoes no longer reach the sender), NOT a spawn defect — the cell's
    // own emission (sender_path == /echo) is therefore tolerated here; the guard
    // sharpens on EXTERNAL senders.
    let resp = client
        .get(format!("http://{addr}/colony/dead_letters"))
        .send()
        .await
        .unwrap();
    let json: serde_json::Value = resp.json().await.unwrap();
    let dlq_hit = json["dead_letters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .any(|e| e["original_target"] == "/echo" && e["sender_path"] != "/echo");
    assert!(
        !dlq_hit,
        "message to /echo must not dead-letter as unreachable target: {json}"
    );

    // Step 5: shutdown chain as in phase_12_b_demo (T12 timeout form).
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("run_with_hooks must finish within 30s after shutdown")
        .expect("join handle must not panic")
        .expect("run_with_hooks must return Ok");
}

/// Polls `GET /colony/trace` until a row with `to_path == expected` appears
/// (the writer task writes asynchronously). Cap ~2s, 50-ms steps — same form as
/// `poll_for_dead_letter` in `phase_12_b_demo.rs`.
async fn poll_for_trace_to_path(
    client: &reqwest::Client,
    addr: SocketAddr,
    expected: &str,
) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let resp = client
            .get(format!("http://{addr}/colony/trace?limit=50"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let json: serde_json::Value = resp.json().await.unwrap();
        let hit = json["trace"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .any(|e| e["to_path"] == expected);
        if hit || tokio::time::Instant::now() >= deadline {
            return hit;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
