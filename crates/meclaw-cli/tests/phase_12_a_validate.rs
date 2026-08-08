//! Phase-12-A TDD anchor: --validate is a dry run, no HTTP bind even when
//! --api stands alongside it (a one-line stderr note, no error).
//!
//! Phase-12-close T28 note: --validate was hardened additively with
//! plan_bootstrap + probe_boot_state (see `phase_12_validate_hardening.rs`). The
//! bind-skip test therefore needs a valid fixture, otherwise the new
//! plan_bootstrap check fails (NoRootDir on an empty TempDir). The statement of
//! the test stays: --validate precedence skips the bind, even with --api
//! alongside.

use meclaw_cli::{Cli, run};
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_skips_http_bind_even_with_api_flag() {
    let td = tempfile::TempDir::new().unwrap();
    // A minimal valid fixture (single hive marker) that passes plan_bootstrap.
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
        stdio_format: meclaw_cli::StdioFormat::Text,
    };
    run(cli).await.unwrap();
}
