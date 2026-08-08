//! Templates subsystem (phase 11). Spec: docs/meclaw-overview.md § Template system.

pub mod registry;
pub mod scanner;
pub mod version;

pub use registry::{ResolveError, TemplateEntry, TemplatesRegistry};
pub use scanner::{ScannedTemplate, ScannerError, parse_template_json, scan_templates_dir};
pub use version::{SimpleVersion, VersionError, parse_simple_version};

use crate::persist::colony_db::{ColonyDb, TemplateRow};
use crate::persist::writer::ColonyWriteOp;
use meclaw_core::Uuid;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Increments `queue_depth` + sends `op` to the writer thread (bounded).
///
/// Phase-12-A helper: uses the cloned writer sender + the queue_depth Arc
/// (both Send+Sync) instead of holding `&ColonyDb` across `.await` — `ColonyDb`
/// contains a `rusqlite::Connection` (!Sync) → `&ColonyDb` is !Send → it would
/// make the surrounding `colony_task` future !Send and break `tokio::spawn`.
async fn send_op_via(
    writer_tx: &tokio::sync::mpsc::Sender<ColonyWriteOp>,
    queue_depth: &Arc<AtomicI64>,
    op: ColonyWriteOp,
) {
    queue_depth.fetch_add(1, Ordering::Relaxed);
    let depth = queue_depth.load(Ordering::Relaxed);
    if depth > 1000 {
        tracing::warn!(depth, "colony.db writer backlog > 1000");
    }
    writer_tx.send(op).await.expect("writer thread dead");
}

/// Async core of `apply_scan_result`: takes Send+Sync channels instead of
/// `&ColonyDb` → no !Send capture in the `colony_task` future.
async fn apply_scan_result_inner(
    scanned: Vec<scanner::ScannedTemplate>,
    existing: Vec<TemplateRow>,
    writer_tx: tokio::sync::mpsc::Sender<ColonyWriteOp>,
    queue_depth: Arc<AtomicI64>,
    now: i64,
) {
    let scanned_keys: std::collections::HashSet<(String, Option<String>)> = scanned
        .iter()
        .map(|s| (s.name.clone(), s.version.clone()))
        .collect();
    // Upsert all scanned templates.
    for s in &scanned {
        let template_id = Uuid::now_v7().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        send_op_via(
            &writer_tx,
            &queue_depth,
            ColonyWriteOp::UpsertTemplate {
                template_id,
                name: s.name.clone(),
                version: s.version.clone(),
                filesystem_path: s.filesystem_path.to_string_lossy().into_owned(),
                description_json: s.description_json.clone(),
                tags_json: s.tags_json.clone(),
                author: s.author.clone(),
                scanned_at: now,
                ack: Some(tx),
            },
        )
        .await;
        let _ = rx.recv();
    }
    // Remove entries whose (name, version) is not in scanned set.
    for e in existing {
        if !scanned_keys.contains(&(e.name.clone(), e.version.clone())) {
            let (tx, rx) = std::sync::mpsc::channel();
            send_op_via(
                &writer_tx,
                &queue_depth,
                ColonyWriteOp::RemoveTemplate {
                    template_id: e.template_id,
                    ack: Some(tx),
                },
            )
            .await;
            let _ = rx.recv();
        }
    }
}

/// Walk `templates_root`, persist findings, delete entries whose directory disappeared.
///
/// Idempotent: calling it twice against the same filesystem state yields the same DB state.
/// Seen entries are upserted; entries whose (name, version) are no longer on disk are removed
/// (lazy-remove path 1, overview Z.1163).
///
/// Phase-12-A: returns `impl Future + Send` instead of `async fn`, so that the
/// `&ColonyDb` borrow is not captured into the future — it only lives in the
/// synchronous prologue block. The resulting future holds exclusively Send
/// captures (channel clone + Arc clone + owned data); that keeps the
/// surrounding `colony_task` future Send (rusqlite::Connection is !Sync).
pub fn apply_scan_result<'a>(
    templates_root: &'a std::path::Path,
    db: &ColonyDb,
    now: i64,
) -> impl std::future::Future<Output = Result<(), scanner::ScannerError>> + Send + 'a {
    // Synchronous prologue: scan + db read; none of it outlives the function end.
    let scan_result = scanner::scan_templates_dir(templates_root);
    let existing = db.read_templates().unwrap_or_default();
    let writer_tx = db.writer_tx.clone();
    let queue_depth = db.queue_depth.clone();
    async move {
        let scanned = scan_result?;
        apply_scan_result_inner(scanned, existing, writer_tx, queue_depth, now).await;
        Ok(())
    }
}

/// Startup algorithm step 3 (overview Z.1368): load from DB; empty → auto-scan;
/// `force_rescan=true` → always scan.
///
/// Called after `ColonyDb::open`, before bootstrap walk. Idempotent.
///
/// Phase-12-A: `impl Future + Send` form analogous to `apply_scan_result` — the
/// `&ColonyDb` borrow does not outlive the synchronous prologue scope, so the
/// returned future is Send.
pub fn boot_load_or_scan<'a>(
    templates_root: &'a std::path::Path,
    db: &ColonyDb,
    force_rescan: bool,
    now: i64,
) -> impl std::future::Future<Output = Result<(), scanner::ScannerError>> + Send + 'a {
    let existing = db.read_templates().unwrap_or_default();
    let needs_scan = force_rescan || existing.is_empty();
    let scan_fut = if needs_scan {
        Some(apply_scan_result(templates_root, db, now))
    } else {
        None
    };
    async move {
        if let Some(fut) = scan_fut {
            fut.await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use tempfile::TempDir;

    fn make_template(td: &TempDir, dir: &str, name: &str, version: Option<&str>) {
        let p = td.path().join("templates").join(dir);
        std::fs::create_dir_all(&p).unwrap();
        let body = match version {
            Some(v) => format!(r#"{{"name":"{name}","version":"{v}"}}"#),
            None => format!(r#"{{"name":"{name}"}}"#),
        };
        std::fs::write(p.join("template.json"), body).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn initial_scan_writes_all_to_db() {
        let td = TempDir::new().unwrap();
        make_template(&td, "a", "a", None);
        make_template(&td, "b@1.0.0", "b", Some("1.0.0"));
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        apply_scan_result(&td.path().join("templates"), &db, 1234)
            .await
            .unwrap();
        let rows = db.read_templates().unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rescan_is_idempotent() {
        let td = TempDir::new().unwrap();
        make_template(&td, "a", "a", None);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        apply_scan_result(&td.path().join("templates"), &db, 100)
            .await
            .unwrap();
        apply_scan_result(&td.path().join("templates"), &db, 200)
            .await
            .unwrap();
        let rows = db.read_templates().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scanned_at, 200);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rescan_deletes_entries_whose_dir_disappeared() {
        let td = TempDir::new().unwrap();
        make_template(&td, "ghost", "ghost", None);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        apply_scan_result(&td.path().join("templates"), &db, 100)
            .await
            .unwrap();
        std::fs::remove_dir_all(td.path().join("templates/ghost")).unwrap();
        apply_scan_result(&td.path().join("templates"), &db, 200)
            .await
            .unwrap();
        assert!(
            db.read_templates().unwrap().is_empty(),
            "the ghost entry must be deleted"
        );
    }
}
