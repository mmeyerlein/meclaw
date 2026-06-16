//! Backwards-compat re-export for existing tests.
//!
//! The canonical apply implementation now lives in `meclaw-colony::bootstrap_apply`.

pub use meclaw_colony::{BootstrapReport, apply_bootstrap_plan, bootstrap_from_filesystem};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColonyHandle;
    use crate::factories::EchoCellFactory;
    use meclaw_colony::{CellFactory, CellFactoryRegistry};
    use meclaw_core::Path;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn factories() -> CellFactoryRegistry {
        let mut r: CellFactoryRegistry = HashMap::new();
        let arc: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
        r.insert("echo".to_string(), arc);
        r
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn apply_plan_registers_all_in_atomic_pass() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let colony = ColonyHandle::new();
        let report = bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
            .await
            .unwrap();
        assert_eq!(report.hive_count, 1);
        assert_eq!(report.cell_count, 2);
        assert_eq!(report.edge_count, 1);
        colony.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn first_boot_persists_edges_and_scopes_in_single_transaction() {
        use meclaw_core::Path;
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/main/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/main/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let h = ColonyHandle::new();
        let _report = bootstrap_from_filesystem(td.path(), &factories(), &h.runtime())
            .await
            .unwrap();
        let db_path = h.tempdir_path().join("colony.db");
        h.shutdown().await; // BARRIER
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let edge_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        let scope_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM hive_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_cnt, 1, "1 edge persisted (atomar mit hive_scopes)");
        assert_eq!(scope_cnt, 1, "1 hive_scope persisted (atomar mit edges)");
        // Suppress unused-import warning when Path isn't used further.
        let _ = Path::new("/");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reboot_hydrates_edges_from_colony_db_and_ignores_hints() {
        use meclaw_core::Path;
        let td = TempDir::new().unwrap();
        // FirstBoot mit a→b
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let h1 = ColonyHandle::new();
        let _ = bootstrap_from_filesystem(td.path(), &factories(), &h1.runtime())
            .await
            .unwrap();
        let db_path = h1.tempdir_path().join("colony.db");
        h1.shutdown().await;

        // Reboot mit anderen Hints (c→d) — config.json wird modifiziert.
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./c","to":"./d"}]}}}"#,
        );
        write(
            td.path(),
            "main/c/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/d"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/d/config.json",
            r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let h2 = ColonyHandle::with_db_path(&db_path);
        let _ = bootstrap_from_filesystem(td.path(), &factories(), &h2.runtime())
            .await
            .unwrap();
        h2.shutdown().await;

        // edges-Tabelle nach Re-Boot: UNVERÄNDERT (a→b, NICHT c→d).
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT from_path, to_path FROM edges")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1, "exactly 1 edge persisted (from FirstBoot)");
        assert_eq!(
            rows[0],
            ("/a".to_string(), "/b".to_string()),
            "edge unchanged after reboot — hints c→d ignored, NOT persisted"
        );

        // Suppress unused import warning.
        let _ = Path::new("/");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bootstrap_returns_errors_without_spawning() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", "not valid json");

        let colony = ColonyHandle::new();
        let err = bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
            .await
            .unwrap_err();
        assert!(!err.is_empty(), "errors must be reported");
        // Probe that no cells were registered: send to /main → UnresolvedPath in DLQ.
        let msg = crate::MessageBuilder::new("/main").build();
        colony.send_from(Path::new("/"), msg).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let dls = colony.drain_dead_letters().await;
        assert!(!dls.is_empty(), "no /main spawned — message DLQs");
        colony.shutdown().await;
    }
}
