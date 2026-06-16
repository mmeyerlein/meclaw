//! Phase-12-A TDD-Anker: --validate ist Dry-Run, no HTTP-bind selbst wenn
//! --api parallel steht (einzeilige stderr-Notiz, kein Error).
//!
//! Phase-12-Close T28 Hinweis: --validate wurde additiv um plan_bootstrap +
//! probe_boot_state gehärtet (siehe `phase_12_validate_hardening.rs`). Der
//! bind-skip-Test braucht daher eine valide Fixture, sonst failt der neue
//! plan_bootstrap-Check (NoRootDir auf leerer TempDir). Die Aussage des Tests
//! bleibt: --validate-Vorrang skipt den bind, auch wenn --api parallel steht.

use meclaw_cli::{Cli, run};
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_skips_http_bind_even_with_api_flag() {
    let td = tempfile::TempDir::new().unwrap();
    // Minimale valid-fixture (single hive-marker), passt durch plan_bootstrap.
    std::fs::create_dir_all(td.path().join("demo")).unwrap();
    std::fs::write(
        td.path().join("demo/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    let cli = Cli {
        root: td.path().into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:1".parse::<SocketAddr>().unwrap()), // Port 1: bind permission-denied → only succeeds because validate skips bind
        daemon: false,
        validate: true,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
    };
    run(cli).await.unwrap();
}
