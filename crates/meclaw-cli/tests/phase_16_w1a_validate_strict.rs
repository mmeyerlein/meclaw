//! Phase-16 W1a A8 (Ruling 2026-06-12): `--validate` endpoint-existence
//! semantics. A static run has no running colony, so it cannot see
//! runtime-spawned cells — a dangling `params.graph` endpoint is a WARNING
//! (exit 0), and the new `--strict` flag promotes it to a hard error
//! (exit != 0). The operator decides, nginx -t style.

use meclaw_cli::{Cli, run};

/// A root hive wiring `. → /sink` where `/sink` has no FS directory.
fn write_dangling_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"/sink"}]}}}"#,
    )
    .unwrap();
}

fn cli_for(root: &std::path::Path, strict: bool) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        strict,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
    }
}

/// Plain `--validate`: a dangling endpoint only WARNS → exit 0 (Ok).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_warns_on_dangling_endpoint_exit_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_dangling_topology(td.path());
    run(cli_for(td.path(), false))
        .await
        .expect("--validate must warn (not fail) on a dangling endpoint");
}

/// `--validate --strict`: the same dangling endpoint becomes a hard error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_strict_fails_on_dangling_endpoint() {
    let td = tempfile::TempDir::new().unwrap();
    write_dangling_topology(td.path());
    let res = run(cli_for(td.path(), true)).await;
    assert!(
        res.is_err(),
        "--validate --strict must FAIL on a dangling endpoint"
    );
}
