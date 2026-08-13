//! Paket-1 (f) CLI boot half — `--validate` over a config.json carrying
//! `cell.mailbox_size: 0` fails: `run` returns `Err` (exit != 0) with a
//! validate diagnostic on stderr.
//!
//! `--validate` runs `plan_bootstrap`, which rejects `mailbox_size: 0` as an
//! `InvalidCellField` before any spawn (see `bootstrap.rs`). The mutation half
//! of demo (f) — `add_nodes` with `mailbox_size: 0` rejected pre-destructively
//! — is already proven in
//! `crates/meclaw-colony/tests/paket_1_mailbox_size_mutation.rs`
//! (`mutation_spawn_zero_mailbox_size_rejected_pre_destructive`); not duplicated.

use meclaw_cli::{Cli, run};
use std::net::SocketAddr;
use std::path::PathBuf;

fn cli_validate(root: PathBuf, api: Option<SocketAddr>) -> Cli {
    Cli {
        root,
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api,
        daemon: false,
        validate: true,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_rejects_mailbox_size_zero() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive + one child cell with the illegal mailbox_size: 0.
    std::fs::create_dir_all(td.path().join("main/c")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/c/config.json"),
        br#"{"cell":{"type":"echo","mailbox_size":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let cli = cli_validate(td.path().into(), None);
    let err = run(cli)
        .await
        .expect_err("mailbox_size:0 must fail --validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}
