//! Step 5.1: Root-Hive-Guard test.
//!
//! Direct-Mode requires `/` to be a hive (`type: "hive"`). If the root
//! directory has no `config.json` at all, or if the root `config.json` is not
//! a hive, `run_with_hooks` must return `Err` with a diagnostic message.

use meclaw_cli::{Args, run_with_hooks};

fn direct_mode_cli(root: &std::path::Path) -> Args {
    Args {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: false,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
    }
}

/// Direct-Mode on a root with NO `config.json` (no hive) → `Err` containing
/// "root must be a hive for direct-mode".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_mode_no_root_hive_returns_err() {
    let td = tempfile::TempDir::new().unwrap();
    // Empty root — no config.json at all.
    let cli = direct_mode_cli(td.path());
    let result = run_with_hooks(cli, None, None).await;
    let err = result.expect_err("direct-mode on non-hive root must return Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("root must be a hive for direct-mode"),
        "error message must mention root-hive requirement, got: {msg}"
    );
}

/// Direct-Mode on a root where the root directory (`/`) is NOT a hive
/// (type: "echo") → `Err` with same diagnostic.
///
/// The meclaw root hive lives in a subdirectory of `<root>` (e.g. `main/`);
/// that subdirectory becomes meclaw path `/`. Here we create `main/` with
/// type "echo" (not a hive), so the plan has no hive at `/`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_mode_non_hive_root_config_returns_err() {
    let td = tempfile::TempDir::new().unwrap();
    // A subdirectory exists with a non-hive config — so there IS a root cell
    // dir, but it is not a hive.
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"echo"}}"#,
    )
    .unwrap();
    let cli = direct_mode_cli(td.path());
    let result = run_with_hooks(cli, None, None).await;
    let err = result.expect_err("direct-mode on non-hive root must return Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("root must be a hive for direct-mode"),
        "error message must mention root-hive requirement, got: {msg}"
    );
}
