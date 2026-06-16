//! Phase-13.5 Lifecycle-3a Demo (e) — validate-path unchanged after rehydration
//! (Auflage A5 regression).
//!
//! Spec § `--validate` (docs/meclaw-overview.md Z.430 ff.):
//! `--validate` is a **dry-run** path: it runs `plan_bootstrap` +
//! `probe_boot_state` and returns BEFORE `colony_task::spawn` and BEFORE
//! `bootstrap_from_filesystem`. No cells are ever spawned.
//!
//! This test proves that the `--validate` path yields the **same exit-codes**
//! after a colony.db with a persisted registry exists (Reboot state) as it
//! does on a fresh first-boot (FirstBoot state):
//!
//!   - valid FS tree + populated colony.db → exit 0 / Ok
//!   - invalid FS tree (NoRootDir) + populated colony.db → exit !=0 / Err
//!   - multiple root dirs + populated colony.db → exit !=0 / Err
//!
//! No-spawn proof: `--validate` returns `Ok(())` at line ~227 of
//! `meclaw-cli/src/lib.rs`, which is unconditionally BEFORE the
//! `tokio::spawn(meclaw_colony::colony_task(...))` call at line ~252.
//! There is no `bootstrap_from_filesystem` call in the `--validate` branch.
//! This is a structural guarantee — the test documents and relies on it via
//! comment; no runtime counter is needed.
//!
//! Pattern mirrors `phase_12_validate_hardening.rs` (T28) — the existing
//! validate tests; this file adds the "after populated colony.db" dimension.

use meclaw_cli::{Cli, run};
use std::path::PathBuf;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Write a minimal valid fixture: a single hive-marker directory with one
/// known built-in cell type (`bash`) so `plan_bootstrap` with
/// `built_in_factories()` accepts it. Mirrors the pattern from
/// `phase_12_validate_hardening.rs` but adds a cell to make the FS topology
/// non-trivial (the pre-seeded colony.db already provides the Reboot state).
fn write_valid_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
}

/// Seed a colony.db with all three non-empty persistent tables so that
/// `probe_boot_state` returns `BootState::Reboot` (all-non-zero check).
///
/// Mirrors the seeding pattern from `bootstrap.rs` unit-tests (Z.1199-1210).
/// This bypass of the Colony is intentional: we need a pre-existing colony.db
/// WITHOUT having run a full boot cycle, to keep the test hermetic and fast.
fn seed_populated_colony_db(db_path: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    meclaw_colony::persist::setup_colony_db(&conn).unwrap();
    conn.execute(
        "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at) \
         VALUES ('/a', 'aaaaaaaa-0000-7000-8000-000000000001', 'echo', 'active', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO edges (id, from_path, to_path, created_at) \
         VALUES ('aaaaaaaa-0000-7000-8000-000000000002', '/a', '/a', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hive_scopes (path, created_at) VALUES ('/demo', 0)",
        [],
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
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Valid FS tree + populated colony.db (Reboot state) → `--validate` returns
/// Ok (exit 0). Proves that the presence of a persisted registry does NOT break
/// the validate path and that exit-codes are unchanged vs. a fresh colony.
///
/// No-spawn proof: `--validate` in `meclaw-cli/src/lib.rs` returns `Ok(())` at
/// the validate-branch (before `colony_task::spawn`). The call to `run(cli)`
/// completing without spawning a background colony is inherent to the code path
/// — `bootstrap_from_filesystem` is never reached in the `--validate` branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_path_unchanged_after_rehydration_valid_tree_exits_ok() {
    let td = tempfile::TempDir::new().unwrap();
    write_valid_fixture(td.path());
    // Pre-seed colony.db so probe_boot_state sees Reboot (populated tables).
    seed_populated_colony_db(&td.path().join("colony.db"));

    let cli = cli_validate(td.path().into());
    run(cli)
        .await
        .expect("--validate with valid tree + populated colony.db must exit Ok");
}

/// Invalid FS tree (NoRootDir — empty root) + populated colony.db → `--validate`
/// returns Err (exit !=0). Proves that the validate-path failure modes are
/// unchanged even after a colony.db with a persisted registry exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_path_unchanged_after_rehydration_no_root_dir_returns_err() {
    let td = tempfile::TempDir::new().unwrap();
    // Empty root → plan_bootstrap returns NoRootDir (no hive config.json found).
    // Pre-seed colony.db so probe_boot_state sees Reboot.
    seed_populated_colony_db(&td.path().join("colony.db"));

    let cli = cli_validate(td.path().into());
    let err = run(cli)
        .await
        .expect_err("NoRootDir with populated colony.db must still fail --validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error must mention validate; got: {msg}"
    );
}

/// Multiple root dirs + populated colony.db → `--validate` returns Err.
/// Proves that the MultipleRootDirs failure case is equally unaffected by the
/// presence of a persisted registry in colony.db.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_path_unchanged_after_rehydration_multiple_root_dirs_returns_err() {
    let td = tempfile::TempDir::new().unwrap();
    // Two top-level non-blacklisted dirs with config.json → MultipleRootDirs.
    std::fs::create_dir_all(td.path().join("root_a")).unwrap();
    std::fs::write(
        td.path().join("root_a/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(td.path().join("root_b")).unwrap();
    std::fs::write(
        td.path().join("root_b/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    // Pre-seed colony.db so probe_boot_state sees Reboot.
    seed_populated_colony_db(&td.path().join("colony.db"));

    let cli = cli_validate(td.path().into());
    let err = run(cli)
        .await
        .expect_err("MultipleRootDirs with populated colony.db must still fail --validate");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("validate"),
        "error must mention validate; got: {msg}"
    );
}
