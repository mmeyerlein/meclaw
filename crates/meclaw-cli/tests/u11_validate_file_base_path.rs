//! U11 — `file`/`edit`-Factory Validate/Spawn-Asymmetrie (pre-16 fix).
//!
//! Before the fix: `validate_params` checked only `base_path.is_absolute()`,
//! while `spawn_cell` canonicalized + `is_dir`-checked it. A non-existent
//! `base_path` therefore passed `--validate` (plan-phase) but its `spawn_cell`
//! `Err` was unwrapped by `bootstrap_apply.rs`
//! `.expect("validated in plan-phase: invalid params cannot reach apply")` →
//! Boot-PANIC mid-apply (Run-5 repro 2026-06-11).
//!
//! After the fix: `validate_params` runs the SAME canonicalize + `is_dir` check
//! (parser-invariant restored), so `--validate` rejects the bad `base_path` in
//! the plan-phase and apply is never reached — a clean validate error, not a
//! panic.

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
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_rejects_file_cell_nonexistent_base_path() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive + one `file` child whose base_path does NOT exist.
    std::fs::create_dir_all(td.path().join("main/f")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/f/config.json"),
        br#"{"cell":{"type":"file"},"params":{"base_path":"/nonexistent/u11/definitely/not/here"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let cli = cli_validate(td.path().into(), None);
    // Must be a clean validate Err — NOT a panic. (A panic mid-apply would
    // abort the test process, so reaching this assertion at all already proves
    // no Boot-PANIC; the validate-diagnostic assert pins the plan-phase reject.)
    let err = run(cli)
        .await
        .expect_err("non-existent file base_path must fail --validate (U11)");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_rejects_edit_cell_nonexistent_base_path() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main/e")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/e/config.json"),
        br#"{"cell":{"type":"edit"},"params":{"base_path":"/nonexistent/u11/definitely/not/here"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let cli = cli_validate(td.path().into(), None);
    let err = run(cli)
        .await
        .expect_err("non-existent edit base_path must fail --validate (U11)");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}
