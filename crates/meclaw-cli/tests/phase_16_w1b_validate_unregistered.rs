//! Phase-16 W1b A5b (Ruling 2026-06-12, test b): `--validate` reports unknown
//! (unregistered) cell directories on a **Reboot**. Registration is
//! instantiation/mutation-only; the reboot walk reports an unknown node, never
//! adopts it. Semantics mirror the W1a endpoint check: plain `--validate` emits
//! a WARNING (exit 0, nginx -t role), `--strict` promotes it to a hard error
//! (exit != 0). The operator decides.
//!
//! Setup: a populated colony.db (all three persistent tables non-empty →
//! `probe_boot_state` = Reboot) whose registry knows `/known` but NOT `/foreign`.
//! The FS tree carries both cell dirs. On the Reboot validate, `/known` is a
//! rehydrated node (planned) and `/foreign` is the unknown node (reported).

use meclaw_cli::{Cli, run};

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

/// Root hive + a registered cell (`/known`) + an unknown cell dir (`/foreign`).
fn write_tree_with_foreign(root: &std::path::Path) {
    let demo = root.join("demo");
    std::fs::create_dir_all(demo.join("known")).unwrap();
    std::fs::create_dir_all(demo.join("foreign")).unwrap();
    std::fs::write(demo.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        demo.join("known/config.json"),
        format!(r#"{{"cell":{{"type":"bash"}},{CONTRACT}}}"#),
    )
    .unwrap();
    std::fs::write(
        demo.join("foreign/config.json"),
        format!(r#"{{"cell":{{"type":"bash"}},{CONTRACT}}}"#),
    )
    .unwrap();
}

/// Seed colony.db so `probe_boot_state` returns Reboot and `/known` is a
/// persisted (overlay-hit) node while `/foreign` is absent from the registry.
/// Mirrors `phase_13_5_lifecycle_3a_validate_regression.rs::seed_populated_colony_db`.
fn seed_reboot_db_knowing_only_known(db_path: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    meclaw_colony::persist::setup_colony_db(&conn).unwrap();
    conn.execute(
        "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at) \
         VALUES ('/known', 'aaaaaaaa-0000-7000-8000-000000000001', 'bash', 'active', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO edges (id, from_path, to_path, created_at) \
         VALUES ('aaaaaaaa-0000-7000-8000-000000000002', '/known', '/known', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hive_scopes (path, created_at) VALUES ('/', 0)",
        [],
    )
    .unwrap();
}

fn cli_validate(root: &std::path::Path, strict: bool) -> Cli {
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
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

/// Plain `--validate` on a Reboot with an unknown cell dir: WARNS → exit 0 (Ok).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_warns_on_unregistered_node_exit_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree_with_foreign(td.path());
    seed_reboot_db_knowing_only_known(&td.path().join("colony.db"));

    run(cli_validate(td.path(), false))
        .await
        .expect("--validate must WARN (not fail) on an unregistered cell dir at reboot");
}

/// `--validate --strict` on the same state: the unknown cell dir is a hard error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_strict_fails_on_unregistered_node() {
    let td = tempfile::TempDir::new().unwrap();
    write_tree_with_foreign(td.path());
    seed_reboot_db_knowing_only_known(&td.path().join("colony.db"));

    let res = run(cli_validate(td.path(), true)).await;
    assert!(
        res.is_err(),
        "--validate --strict must FAIL on an unregistered cell dir at reboot"
    );
}
