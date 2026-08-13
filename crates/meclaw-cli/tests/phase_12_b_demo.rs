//! Phase-12-B T14: Slice-Demo E2E — exercises the full HTTP → real-Colony loop
//! end-to-end. Uses the production `run_with_hooks` entry point (no test
//! `ColonyHandle`-Wrappers), brings up a TempDir-Colony with a single empty
//! hive at `/main`, drives a handful of read + write endpoints, and verifies
//! the graceful-shutdown chain unwinds.
//!
//! Coverage:
//! - Bootstrap incl. `colony.db` creation (the hive at `<tmp>/main` keeps
//!   `plan_bootstrap` from rejecting an empty root with `NoRootDir`).
//! - HTTP-layer ↔ real `ColonyHandle` wiring (T12 spawn-wiring).
//! - `GET /colony/registry` (T8.1) on a freshly-booted Colony → empty slot.
//! - `POST /messages` (T10) fire-and-forget → 202 + `{message_id}`.
//! - Unresolved-target routing → DeadLetter; `GET /colony/dead_letters`
//!   (T8.2) returns the entry.
//! - `GET /colony/trace?limit=10` (T8.6) → 200, slot present.
//! - `GET /colony/events` (T8 / T11) → 501 (Phase-14 deferred).
//! - Shutdown-chain: oneshot → axum drain → `ColonyMsg::Shutdown` → `colony_join`.
//!
//! NOT covered here (DONE_WITH_CONCERNS):
//! - `POST /colony/mutations` with `add_nodes` against a production template.
//!   The only production-cell-types that could be instantiated via mutation
//!   would need a `templates/<name>/` directory **plus** at least one matching
//!   factory in `built_in_factories()`. `echo` is test-only
//!   (`meclaw-testing/src/factories/echo.rs`), and the cells that live in
//!   `meclaw-cells` (`bash`, `web_fetch`, `edit`, `read`, `llm`, `store`,
//!   `code`) all require non-trivial `params` setup that would dilute this
//!   demo. The full mutation-via-HTTP path is exercised by T9
//!   (`crates/meclaw-api/tests/phase_12_b_mutations_post.rs`) against the
//!   testing Colony with the `echo`-factory registered — the loop is
//!   demonstrably wired.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slice_12b_demo_e2e_full_loop() {
    // Minimal bootstrap surface: a single hive at `<tmp>/main` satisfies
    // `plan_bootstrap`'s NoRootDir-Check and registers zero cells.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(main_dir.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
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
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };

    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    let actual_addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    // Step 1: GET /colony/registry — fresh state, the lone hive registers zero
    // cells, so the slot is an empty array.
    let resp = client
        .get(format!("http://{actual_addr}/colony/registry"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        json["registry"].as_array().unwrap().len(),
        0,
        "fresh hive-only root registers zero cells"
    );

    // Step 2: POST /messages to an unresolved target. Fire-and-forget contract:
    // 202 + `{message_id}` regardless of whether the target exists. The
    // unresolved-routing produces a DeadLetter downstream.
    let body = serde_json::json!({
        "target": "/nonexistent/cell",
        "body": {
            "messages": [
                { "origin": "user", "type": "text", "text": "hello demo" }
            ]
        }
    });
    let resp = client
        .post(format!("http://{actual_addr}/messages"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json["message_id"].is_string(),
        "POST /messages reply must carry message_id"
    );

    // Step 3: GET /colony/dead_letters — poll for the unresolved-routing-DL.
    // Polling instead of a fixed sleep keeps the test robust under varying
    // scheduler pressure (CI vs. local --workspace runs); cap at ~2s total.
    let dl_entries = poll_for_dead_letter(&client, actual_addr, "/nonexistent/cell").await;
    assert!(
        !dl_entries.is_empty(),
        "unresolved POST /messages must end up in /colony/dead_letters within 2s"
    );

    // Step 4: GET /colony/trace?limit=10 — slot is present, query parsing OK.
    // The exact entry count depends on substrate routing-log behaviour; the
    // load-bearing assertion for this demo is "endpoint reachable + 200".
    let resp = client
        .get(format!("http://{actual_addr}/colony/trace?limit=10"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(json["trace"].is_array(), "trace slot must be an array");

    // Step 5: GET /colony/events — 501 (Phase-14 deferred, T11).
    let resp = client
        .get(format!("http://{actual_addr}/colony/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_IMPLEMENTED);

    // Step 6: Shutdown-Signal → join. Same timeout shape as T12.
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("run_with_hooks must finish within 30s after shutdown")
        .expect("join handle must not panic")
        .expect("run_with_hooks must return Ok");
}

/// Polls `GET /colony/dead_letters` up to ~2s for an entry whose
/// `original_target` matches `expected_target`. Returns the matching subset
/// (possibly empty) once the deadline is hit.
async fn poll_for_dead_letter(
    client: &reqwest::Client,
    addr: SocketAddr,
    expected_target: &str,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let resp = client
            .get(format!("http://{addr}/colony/dead_letters"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let json: serde_json::Value = resp.json().await.unwrap();
        let matches: Vec<serde_json::Value> = json["dead_letters"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e["original_target"].as_str() == Some(expected_target))
            .collect();
        if !matches.is_empty() {
            return matches;
        }
        if tokio::time::Instant::now() >= deadline {
            return matches;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
