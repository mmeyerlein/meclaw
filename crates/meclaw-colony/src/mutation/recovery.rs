//! Phase-6 recovery: on startup (before colony_task spawns the writer thread),
//! scan `mutation_log` for `in_flight` rows, clean up `.staging/<id>/` debris,
//! and transition them to `failed` with `failure_reason='crash_during_commit'`.
//!
//! Audit-Modell (Entscheidung 9): `failed` = "hat committed-Schritt nicht erreicht".
//! FS-Bootstrap (Startup Schritt 4) ist autoritativ für tatsächliche Cell-Existenz —
//! renamete Pfade aus dem crashed Apply-Lauf bleiben als Orphans im Live-Tree und
//! werden vom FS-Bootstrap adoptiert. KEINE Roll-Forward-Reconciliation.
//!
//! P3-D1: additionally sweeps `config.json.tmp` crash debris left in live cell
//! directories by `targeted_overwrite_config` (rename.rs). Only the exact file
//! `config.json.tmp` is targeted — no other `*.tmp` pattern, no other files.

#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    /// `mutation_log.id`-Werte, die auf `failed` transitioniert wurden.
    pub failed_mutation_ids: Vec<String>,
    /// `.staging/<id>/`-Verzeichnisse, die in dieser Recovery-Runde gelöscht wurden.
    /// Enthält auch Orphans (Sub-Dirs ohne mutation_log-Eintrag).
    pub staging_dirs_removed: Vec<String>,
    /// Paths of orphaned `config.json.tmp` side-files swept from live cell
    /// directories. These are the exact staging artifact left by
    /// `targeted_overwrite_config` when a process crashes after the `fs::copy`
    /// but before or during the `fs::rename(2)`.
    pub tmp_files_swept: Vec<std::path::PathBuf>,
}

/// Phase-6 startup-recovery. Synchron, läuft VOR `ColonyDb::open` (kein Writer-Thread).
/// Eigene rusqlite-Connection außerhalb der Writer-Owner-Lifetime (sauber per FIX 2).
pub fn recover_in_flight_mutations(
    root: &std::path::Path,
    colony_db_path: &std::path::Path,
) -> rusqlite::Result<RecoveryReport> {
    let conn = rusqlite::Connection::open(colony_db_path)?;
    crate::persist::setup_colony_db(&conn)?;

    // Read all in_flight ids first (don't hold the stmt across the UPDATE loop).
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM mutation_log WHERE status='in_flight'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut staging_removed: Vec<String> = Vec::new();

    for id in &ids {
        let staging_dir = root.join(".staging").join(id);
        if staging_dir.exists() {
            // Audit-Modell: cleanup is best-effort; an FS error here just leaves debris,
            // never blocks the DB transition.
            if std::fs::remove_dir_all(&staging_dir).is_ok() {
                staging_removed.push(id.clone());
            }
        }
        conn.execute(
            "UPDATE mutation_log SET status='failed', failure_reason='crash_during_commit', committed_at=? WHERE id=?",
            rusqlite::params![now, id],
        )?;
    }

    // Orphan-Sweep: .staging/<dir>/ ohne mutation_log-Eintrag.
    // Best-effort (Audit-Modell): FS-Fehler hier nicht in rusqlite::Error mappen —
    // einfach skippen, niemals DB-Transition blockieren.
    let staging_root = root.join(".staging");
    if staging_root.is_dir()
        && let Ok(read_dir) = std::fs::read_dir(&staging_root)
    {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !ids.contains(&name)
                && !staging_removed.contains(&name)
                && std::fs::remove_dir_all(entry.path()).is_ok()
            {
                staging_removed.push(name);
            }
        }
    }

    // P3-D1: sweep orphaned `config.json.tmp` from live cell directories.
    // Only the exact file produced by `targeted_overwrite_config` is removed —
    // NO other files, NO directories, NO other `*.tmp` names.
    // Best-effort (Audit-Modell): FS errors never block the DB transition.
    let mut tmp_swept: Vec<std::path::PathBuf> = Vec::new();
    sweep_config_json_tmp(root, &mut tmp_swept);

    Ok(RecoveryReport {
        failed_mutation_ids: ids,
        staging_dirs_removed: staging_removed,
        tmp_files_swept: tmp_swept,
    })
}

/// Walk the live tree under `root`, collecting and removing every file literally
/// named `config.json.tmp`. Top-level blacklisted directories (`templates/`,
/// `.staging/`, `blobs/`, and any dot-prefixed entry) are skipped — mirroring the
/// blacklist in `bootstrap::is_blacklisted_top_level`.
///
/// **No-Delete Invariant**: this function removes ONLY files named
/// `config.json.tmp`. It NEVER touches `config.json`, `cell.db`, directories,
/// or any other file. All removals are best-effort; FS errors are silently
/// skipped (Audit-Modell).
fn sweep_config_json_tmp(root: &std::path::Path, swept: &mut Vec<std::path::PathBuf>) {
    sweep_into(root, true, swept);
}

fn sweep_into(dir: &std::path::Path, top_level: bool, swept: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !(top_level && crate::bootstrap::is_blacklisted_top_level(&path)) {
                sweep_into(&path, false, swept);
            }
        } else if path.is_file()
            && path.file_name().and_then(|n| n.to_str()) == Some("config.json.tmp")
            && std::fs::remove_file(&path).is_ok()
        {
            swept.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_db(db_path: &std::path::Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        conn
    }

    #[test]
    fn recovery_marks_in_flight_as_failed_and_removes_staging() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = setup_db(&db_path);
        conn.execute(
            "INSERT INTO mutation_log (id, scope, payload_json, status, created_at) VALUES ('X', '/', '{}', 'in_flight', 100)",
            [],
        ).unwrap();
        drop(conn);

        let staging = td.path().join(".staging/X/foo");
        std::fs::create_dir_all(&staging).unwrap();

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();
        assert_eq!(report.failed_mutation_ids, vec!["X".to_string()]);
        assert!(report.staging_dirs_removed.contains(&"X".to_string()));
        assert!(
            !td.path().join(".staging/X").exists(),
            ".staging/X must be removed"
        );

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, failure_reason FROM mutation_log WHERE id='X'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(reason.as_deref(), Some("crash_during_commit"));
    }

    #[test]
    fn recovery_cleans_orphan_staging_dirs_without_db_entry() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        let orphan = td.path().join(".staging/orphan_mid/foo");
        std::fs::create_dir_all(&orphan).unwrap();

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();
        assert!(report.failed_mutation_ids.is_empty());
        assert!(
            !td.path().join(".staging/orphan_mid").exists(),
            "orphan .staging dir must be removed"
        );
    }

    #[test]
    fn recovery_noop_when_no_in_flight_and_no_staging() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();
        assert!(report.failed_mutation_ids.is_empty());
        assert!(report.staging_dirs_removed.is_empty());
    }

    #[test]
    fn recovery_leaves_committed_mutations_untouched() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = setup_db(&db_path);
        conn.execute(
            "INSERT INTO mutation_log (id, scope, payload_json, status, created_at, committed_at) VALUES ('OK', '/', '{}', 'committed', 100, 200)",
            [],
        ).unwrap();
        drop(conn);

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();
        assert!(report.failed_mutation_ids.is_empty());

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let status: String = conn
            .query_row("SELECT status FROM mutation_log WHERE id='OK'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "committed");
    }

    // --- P3-D1 tests: config.json.tmp sweep ---

    /// D1: a live cell dir with config.json + cell.db + an orphaned config.json.tmp.
    /// After recovery: config.json.tmp is gone; config.json, cell.db and the
    /// directory itself are completely untouched (No-Delete Invariant).
    #[test]
    fn recovery_sweeps_orphaned_config_json_tmp_from_live_cell_dir() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        // Simulate a live cell directory under {root}/main/cell_a/
        let cell_dir = td.path().join("main/cell_a");
        std::fs::create_dir_all(&cell_dir).unwrap();
        std::fs::write(cell_dir.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();
        std::fs::write(cell_dir.join("cell.db"), b"fake-db").unwrap();
        // The crash artifact left by targeted_overwrite_config.
        let tmp_path = cell_dir.join("config.json.tmp");
        std::fs::write(&tmp_path, r#"{"cell":{"type":"echo","version":"partial"}}"#).unwrap();

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();

        // config.json.tmp must be gone.
        assert!(!tmp_path.exists(), "config.json.tmp must be swept");
        // config.json and cell.db must be untouched.
        assert!(
            cell_dir.join("config.json").exists(),
            "config.json must survive"
        );
        assert!(cell_dir.join("cell.db").exists(), "cell.db must survive");
        // The directory itself must survive.
        assert!(cell_dir.exists(), "cell directory must survive");
        // The report must reflect the swept file.
        assert_eq!(report.tmp_files_swept.len(), 1);
        assert_eq!(report.tmp_files_swept[0], tmp_path);
    }

    /// D2 (No-Delete pin): a tree with multiple cells — some with config.json.tmp
    /// garbage, some without. Run the sweep; assert only the .tmp files are gone
    /// and the count of real config.json nodes is unchanged.
    #[test]
    fn recovery_sweep_removes_only_tmp_files_and_leaves_real_nodes_intact() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        // Two cell dirs: both have config.json, only one has config.json.tmp garbage.
        for name in &["cell_a", "cell_b"] {
            let d = td.path().join("main").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();
        }
        let tmp_a = td.path().join("main/cell_a/config.json.tmp");
        std::fs::write(&tmp_a, b"garbage").unwrap();

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();

        // Exactly one .tmp was swept.
        assert_eq!(
            report.tmp_files_swept.len(),
            1,
            "only one .tmp must be swept"
        );
        assert_eq!(report.tmp_files_swept[0], tmp_a);
        // config.json.tmp in cell_a is gone.
        assert!(!tmp_a.exists(), "config.json.tmp in cell_a must be swept");
        // Both real config.json files survive.
        assert!(
            td.path().join("main/cell_a/config.json").exists(),
            "cell_a config.json must survive"
        );
        assert!(
            td.path().join("main/cell_b/config.json").exists(),
            "cell_b config.json must survive"
        );
        // No directory was removed.
        assert!(
            td.path().join("main/cell_a").exists(),
            "cell_a dir must survive"
        );
        assert!(
            td.path().join("main/cell_b").exists(),
            "cell_b dir must survive"
        );
    }

    /// D2 negative: a file that is NOT config.json.tmp (e.g. config.json itself,
    /// or an arbitrary .tmp file with a different name) is never touched.
    #[test]
    fn recovery_sweep_does_not_remove_non_tmp_files_or_config_json() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        let cell_dir = td.path().join("main/cell_x");
        std::fs::create_dir_all(&cell_dir).unwrap();
        std::fs::write(cell_dir.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();
        // An arbitrary .tmp with a different name — must NOT be removed.
        let other_tmp = cell_dir.join("data.tmp");
        std::fs::write(&other_tmp, b"user-data").unwrap();
        // config.json itself — must NOT be removed.
        let config = cell_dir.join("config.json");

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();

        assert!(
            report.tmp_files_swept.is_empty(),
            "no config.json.tmp present — nothing swept"
        );
        assert!(other_tmp.exists(), "data.tmp must not be touched");
        assert!(config.exists(), "config.json must not be touched");
    }

    /// Blacklist guard: config.json.tmp inside `templates/` or `blobs/` (top-level
    /// blacklisted dirs) must NOT be swept by the tmp-sweep phase.
    /// Note: `.staging/` is handled separately by the orphan-staging sweep;
    /// this test focuses on the dirs that only the blacklist protects.
    #[test]
    fn recovery_sweep_skips_blacklisted_top_level_dirs() {
        let td = TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let _conn = setup_db(&db_path);

        // config.json.tmp inside templates/ — blacklisted, must not be walked.
        let templates_tmp = td.path().join("templates/foo/config.json.tmp");
        std::fs::create_dir_all(templates_tmp.parent().unwrap()).unwrap();
        std::fs::write(&templates_tmp, b"template-garbage").unwrap();

        // config.json.tmp inside blobs/ — blacklisted, must not be walked.
        let blobs_tmp = td.path().join("blobs/bar/config.json.tmp");
        std::fs::create_dir_all(blobs_tmp.parent().unwrap()).unwrap();
        std::fs::write(&blobs_tmp, b"blob-garbage").unwrap();

        let report = recover_in_flight_mutations(td.path(), &db_path).unwrap();

        assert!(
            report.tmp_files_swept.is_empty(),
            "blacklisted dirs must not be walked"
        );
        assert!(
            templates_tmp.exists(),
            "templates config.json.tmp must be untouched"
        );
        assert!(
            blobs_tmp.exists(),
            "blobs config.json.tmp must be untouched"
        );
    }
}
