//! Phase-13.5 Slice-6 (A7) — `--validate` covers `colony.json` (Demo g).
//!
//! Spec § `--validate` (docs/meclaw-overview.md Z.430 ff.) + § `colony.json`
//! (Z.361-401). `colony.json` is **optional**: absence → defaults (no error). A
//! present-but-broken file is a **hard error** (strict-fail ruling), and
//! `--validate` must surface it as a non-zero exit (Err).
//!
//! The FS fixture is a valid hive-marker so `plan_bootstrap` passes — otherwise an
//! FS-plan error would mask the colony.json check. The colony.json check is the
//! third `--validate` step (after plan_bootstrap + probe_boot_state).

use meclaw_cli::{Cli, run};
use std::path::PathBuf;

fn write_valid_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
}

fn cli_validate(root: PathBuf) -> Cli {
    Cli {
        root,
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

/// Absent colony.json → defaults → `--validate` passes (Ok).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_passes_when_colony_json_absent() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_fixture(dir.path());
    assert!(run(cli_validate(dir.path().to_path_buf())).await.is_ok());
}

/// Valid colony.json → `--validate` passes (Ok).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_passes_when_colony_json_valid() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_fixture(dir.path());
    std::fs::write(
        dir.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 2048}"#,
    )
    .unwrap();
    assert!(run(cli_validate(dir.path().to_path_buf())).await.is_ok());
}

/// Broken colony.json → `--validate` fails (Err, non-zero exit) — Demo g.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_fails_when_colony_json_broken() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_fixture(dir.path());
    std::fs::write(dir.path().join("colony.json"), "{ not valid json").unwrap();
    assert!(run(cli_validate(dir.path().to_path_buf())).await.is_err());
}
