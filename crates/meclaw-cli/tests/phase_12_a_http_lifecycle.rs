//! Phase-12-A E2E-Lifecycle: meclaw --api 127.0.0.1:0 startet,
//! GET /health → 200, GET /any → 404, Ctrl-C → Server stoppt
//! via axum with_graceful_shutdown.

use meclaw_cli::{Cli, run_with_hooks};
use reqwest::StatusCode;
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meclaw_api_starts_serves_health_and_shuts_down_on_signal() {
    let td = tempfile::TempDir::new().unwrap();
    // Since T12 `run_with_hooks` spawns a real colony and calls
    // `bootstrap_from_filesystem` — that requires a root cell directory with a
    // `config.json`. Minimal: an empty hive (registers no cells).
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
        validate_strict: false,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };

    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    let actual_addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    let health = client
        .get(format!("http://{actual_addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    // Phase 12-A asserted 404 on /colony/registry. Since phase 12-B T8.1 the
    // route is registered (200) — the lifecycle test should only prove that
    // unlisted paths return 404, so it was switched to a genuinely unknown
    // path here.
    let unknown = client
        .get(format!("http://{actual_addr}/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    shutdown_tx.send(()).unwrap();
    join.await.unwrap().unwrap();
}
