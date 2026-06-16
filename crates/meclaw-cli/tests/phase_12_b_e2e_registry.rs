//! Phase-12-B T12: end-to-end — `run_with_hooks` spawns real Colony,
//! `GET /colony/registry` returns 200 with empty `registry` slot.
//!
//! Verifies that the stub-`ColonyHandle` (T5) has been replaced by a real
//! `colony_task` spawn + filesystem bootstrap + graceful Colony-shutdown.
//! TempDir has no `config.json`, so bootstrap reports zero cells; the empty
//! registry-slot is the assertion.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_get_registry_via_real_colony_returns_200_empty() {
    let td = tempfile::TempDir::new().unwrap();
    // `plan_bootstrap` rejects an empty root with `NoRootDir`. The minimum
    // contract is a single root-cell directory (here `main/`) holding a hive
    // `config.json`. A hive registers ZERO cells in Colony's registry — that
    // is exactly the assertion the test makes below.
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
    };

    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    let actual_addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

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
        "empty TempDir → bootstrap registers zero cells → empty registry slot"
    );

    shutdown_tx.send(()).unwrap();
    // Give the shutdown chain time to: axum drain → ColonyMsg::Shutdown → ack.await → colony_join.
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
