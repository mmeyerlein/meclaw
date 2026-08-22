//! Templates subsystem (phase 11). Spec: docs/meclaw-overview.md § Template system.

pub mod registry;
pub mod requires;
pub mod scanner;
pub mod version;

pub use registry::{ResolveError, TemplateEntry, TemplatesRegistry};
pub use requires::{RequiredKey, RequiresError, TemplateRequires, read_requires};
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
    // GH #62: `template_id` is a STABLE surrogate key. A rescan re-uses the id
    // an entry already had for its `(name, version)` — re-minting it on every
    // scan made the column useless as a reference: anything that recorded a
    // template id would be pointing at a row that no longer exists after the
    // next `--rescan-templates`. Only a genuinely new `(name, version)` mints.
    let existing_ids: std::collections::HashMap<(String, Option<String>), String> = existing
        .iter()
        .map(|e| ((e.name.clone(), e.version.clone()), e.template_id.clone()))
        .collect();
    // Upsert all scanned templates.
    for s in &scanned {
        let template_id = existing_ids
            .get(&(s.name.clone(), s.version.clone()))
            .cloned()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
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

/// Is the persisted template index anchored somewhere other than `templates_root`?
///
/// `templates.filesystem_path` is an absolute path captured at scan time, so a
/// `colony.db` restored to a new location keeps the SOURCE machine's paths
/// (issue #61). A row that does not sit under the booted templates root is the
/// evidence; the plain prefix check covers the ordinary case and the
/// canonicalised one absorbs symlinked or non-normalised roots. A row whose
/// directory does not exist at all cannot be canonicalised and counts as
/// foreign — which is exactly what a restore without the `templates/` tree is.
///
/// An empty index is never foreign; the empty-table branch already scans.
fn index_is_foreign_to_root(templates_root: &std::path::Path, existing: &[TemplateRow]) -> bool {
    let canonical_root = templates_root
        .canonicalize()
        .unwrap_or_else(|_| templates_root.to_path_buf());
    existing.iter().any(|row| {
        let p = std::path::Path::new(&row.filesystem_path);
        !p.starts_with(templates_root)
            && !p
                .canonicalize()
                .map(|c| c.starts_with(&canonical_root))
                .unwrap_or(false)
    })
}

/// Startup algorithm step 3 (overview Z.1368): load from DB; empty → auto-scan;
/// `force_rescan=true` → always scan.
///
/// Issue #61 adds a third trigger: an index whose `filesystem_path` entries do
/// not sit under `templates_root` belongs to another root (a restore to a new
/// location) and is re-anchored by a full scan on the first boot, without an
/// explicit `--rescan-templates`. A mixed index — some rows matching, some not —
/// is treated like a wholly foreign one, so the outcome is deterministic.
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
    let foreign_index = index_is_foreign_to_root(templates_root, &existing);
    if foreign_index {
        tracing::info!(
            templates_root = %templates_root.display(),
            rows = existing.len(),
            "templates index points outside the booted root — re-anchoring by a full rescan"
        );
    }
    let needs_scan = force_rescan || existing.is_empty() || foreign_index;
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

    /// GH #62: a rescan keeps `template_id` stable for an unchanged
    /// `(name, version)`. Re-minting it made the surrogate key unusable as a
    /// reference — a recorded id would dangle after the next rescan.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rescan_keeps_the_template_id_stable() {
        let td = TempDir::new().unwrap();
        make_template(&td, "a", "a", Some("1.0.0"));
        make_template(&td, "b", "b", None);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let root = td.path().join("templates");
        apply_scan_result(&root, &db, 100).await.unwrap();
        let before: std::collections::HashMap<String, String> = db
            .read_templates()
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.template_id))
            .collect();
        apply_scan_result(&root, &db, 200).await.unwrap();
        let after: std::collections::HashMap<String, String> = db
            .read_templates()
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.template_id))
            .collect();
        assert_eq!(
            before, after,
            "a rescan of unchanged templates must not re-mint their ids"
        );

        // A genuinely new template still mints a fresh id. It must carry a new
        // *name*: a second version of "a" is a duplicate name and would abort the
        // scan (GH #277, ruling Q7).
        make_template(&td, "c@2.0.0", "c", Some("2.0.0"));
        apply_scan_result(&root, &db, 300).await.unwrap();
        let rows = db.read_templates().unwrap();
        let ids: std::collections::HashSet<String> =
            rows.iter().map(|r| r.template_id.clone()).collect();
        assert_eq!(ids.len(), 3, "three distinct templates, three distinct ids");
        assert!(
            ids.contains(before.get("a").unwrap()),
            "the \"a\" entry keeps its id when an unrelated template appears"
        );
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

    /// Issue #61: a restored `colony.db` carries the SOURCE machine's absolute
    /// template paths. The first boot under the new root must re-anchor them
    /// without an explicit `--rescan-templates`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_rescans_when_the_index_points_outside_the_booted_root() {
        // Source root: one scan, so the index carries the source's paths.
        let src = TempDir::new().unwrap();
        make_template(&src, "a", "a", None);
        let db = crate::ColonyDb::open(&src.path().join("c.db")).unwrap();
        boot_load_or_scan(&src.path().join("templates"), &db, false, 100)
            .await
            .unwrap();

        // The tree is restored under a different root and booted there — same
        // colony.db content, new location, no `--rescan-templates`.
        let dst = TempDir::new().unwrap();
        make_template(&dst, "a", "a", None);
        boot_load_or_scan(&dst.path().join("templates"), &db, false, 200)
            .await
            .unwrap();

        let rows = db.read_templates().unwrap();
        assert_eq!(rows.len(), 1, "got {rows:?}");
        assert!(
            std::path::Path::new(&rows[0].filesystem_path).starts_with(dst.path()),
            "a foreign index must be re-anchored to the booted root, got {:?}",
            rows[0].filesystem_path
        );
        assert_eq!(
            rows[0].scanned_at, 200,
            "the re-anchoring is a full rescan, not a patch"
        );
    }

    /// Issue #61, mixed state: some rows already sit under the booted root,
    /// others do not. One deterministic answer — a full rescan, same as for a
    /// wholly foreign index.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_rescans_when_only_part_of_the_index_is_foreign() {
        let td = TempDir::new().unwrap();
        make_template(&td, "a", "a", None);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let root = td.path().join("templates");
        boot_load_or_scan(&root, &db, false, 100).await.unwrap();

        // A second row from a foreign root, written through the production
        // write op — the mixed state a partial restore leaves behind.
        let (tx, rx) = std::sync::mpsc::channel();
        send_op_via(
            &db.writer_tx,
            &db.queue_depth,
            ColonyWriteOp::UpsertTemplate {
                template_id: Uuid::now_v7().to_string(),
                name: "b".into(),
                version: None,
                filesystem_path: "/former-root/templates/b".into(),
                description_json: "{}".into(),
                tags_json: "[]".into(),
                author: None,
                scanned_at: 100,
                ack: Some(tx),
            },
        )
        .await;
        let _ = rx.recv();
        assert_eq!(
            db.read_templates().unwrap().len(),
            2,
            "precondition: the index is mixed"
        );

        boot_load_or_scan(&root, &db, false, 200).await.unwrap();
        let rows = db.read_templates().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a mixed index is rescanned like a foreign one, got {rows:?}"
        );
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[0].scanned_at, 200);
    }

    /// The counter-case: same root, non-empty index → no scan. Guards the
    /// re-anchoring against turning every boot into a rescan.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_does_not_rescan_when_the_index_matches_the_booted_root() {
        let td = TempDir::new().unwrap();
        make_template(&td, "a", "a", None);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let root = td.path().join("templates");
        boot_load_or_scan(&root, &db, false, 100).await.unwrap();
        boot_load_or_scan(&root, &db, false, 200).await.unwrap();
        assert_eq!(
            db.read_templates().unwrap()[0].scanned_at,
            100,
            "an index that already sits under the booted root is left alone"
        );
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
