//! Phase-12-close T28: hardening of --validate (additive, no corridor touch).
//!
//! Before T28, --validate validated ONLY the templates scan (boot_load_or_scan)
//! and returned exit 0 — even with an inconsistent colony.db or several
//! top-level cell dirs (see PROGRESS § phase-12 limitations, commit 5762e69,
//! the --validate scope gap).
//!
//! T28 extends --validate additively with:
//!   1. plan_bootstrap (Filesystem-Bootstrap-Plan: MultipleRootDirs / NoRootDir
//!      / InvalidParams / CorruptCellDb).
//!   2. probe_boot_state (colony.db-Consistency: registry/edges/hive_scopes
//!      mixed counts -> Inconsistent).
//!
//! Every error aggregates + plain text on stderr; exit !=0 when ANY check
//! fails. exit 0 only when all three (templates + plan + colony.db) are clean.
//!
//! NO colony/axum spawn (--validate returns before the spawn). probe_boot_state
//! is called DIRECTLY and Inconsistent is handled gracefully here (print +
//! return Err) — NOT via the colony_task panic path (colony.rs:386, phase-5
//! legacy). The panic site stays untouched (its own robustness pass after
//! phase 13/14).

use meclaw_cli::{Cli, run};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Helper: a minimal valid fixture in the TempDir (single hive marker, no
/// templates directory). Mirrors the pattern from
/// `phase_12_x_e2e_attachment_in_trace.rs:30` and `phase_12_b_demo.rs:41`.
fn write_valid_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
}

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
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_with_valid_fixture_exits_ok() {
    let td = tempfile::TempDir::new().unwrap();
    write_valid_fixture(td.path());
    let cli = cli_validate(td.path().into(), None);
    run(cli).await.expect("valid fixture must exit Ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_with_no_root_dir_returns_err() {
    // Leere TempDir → plan_bootstrap returns NoRootDir.
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_validate(td.path().into(), None);
    let err = run(cli).await.expect_err("empty root must fail validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_with_multiple_root_dirs_returns_err() {
    let td = tempfile::TempDir::new().unwrap();
    // Two top-level non-blacklisted dirs with config.json → MultipleRootDirs.
    write_valid_fixture(td.path());
    std::fs::create_dir_all(td.path().join("other")).unwrap();
    std::fs::write(
        td.path().join("other/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    let cli = cli_validate(td.path().into(), None);
    let err = run(cli)
        .await
        .expect_err("multiple root dirs must fail validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_with_inconsistent_colony_db_returns_err() {
    // Build a colony.db with mixed table counts → BootState::Inconsistent.
    // Direct rusqlite-write into the persistence tables; bypasses Colony.
    let td = tempfile::TempDir::new().unwrap();
    write_valid_fixture(td.path());
    let db_path = td.path().join("colony.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        // Only edges has data → mixed (registry=0, edges=1, hive_scopes=0).
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, created_at) VALUES ('id1', '/a', '/b', 0)",
            [],
        )
        .unwrap();
    }
    let cli = cli_validate(td.path().into(), None);
    let err = run(cli)
        .await
        .expect_err("inconsistent colony.db must fail validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error should mention validate; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_with_api_flag_skips_http_bind_on_valid_fixture() {
    // Regression test from phase_12_a_validate.rs but now with valid fixture:
    // --validate-precedence skips bind even when --api is set, on a fixture
    // that passes the new hardened checks.
    let td = tempfile::TempDir::new().unwrap();
    write_valid_fixture(td.path());
    let cli = cli_validate(
        td.path().into(),
        Some("127.0.0.1:1".parse().unwrap()), // Port 1 would permission-fail bind
    );
    run(cli).await.expect("validate precedence skips the bind");
}
