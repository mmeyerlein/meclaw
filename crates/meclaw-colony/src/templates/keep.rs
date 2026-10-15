//! GH #811 — a colony keeps its own copy of every template version it has
//! instantiated.
//!
//! The `templates` table is keyed by `(name, version)` since GH #664, and
//! `add_templates` registers a new version beside the old one. What was still
//! missing was the FILE: a shipped class lives in one directory
//! (`templates/<name>/`) that holds whichever version the library currently
//! ships. An instance build that swaps the library replaces that directory, the
//! next rescan deletes the row of the old version (`apply_scan_result`, lazy
//! remove), and the way back of a lift needs the old bytes again — which nobody
//! kept. Measured on a live colony: 55 rows for 55 names after a library swap,
//! every older version gone.
//!
//! So the colony copies what it instantiated into the same place a versioned
//! registration writes to — `<local>/<name>@<version>/`, where `<local>` is
//! [`local_root`], a directory the COLONY owns — and repoints the row at that
//! copy under the SAME `template_id`. From then on the
//! row no longer depends on the shipped directory at all, and the scanner lets
//! the copy win over a shipped directory of the same version
//! (`scanner::scan_templates_dir_with_skips`).
//!
//! Three moments call [`keep_referenced_versions`] / [`keep_all`]:
//! after a committed mutation (what it instantiated), at boot (the migration
//! of a colony that instantiated before this existed) and before every rescan
//! (so a rescan after a library swap cannot delete a row before its bytes are
//! safe). A refused mutation keeps nothing: the copy is made after the commit.

use std::path::{Path, PathBuf};

use meclaw_core::Uuid;

use super::registry::TemplateEntry;
use super::scanner::{ScannedTemplate, parse_template_json};
use crate::persist::colony_db::ColonyDb;

/// Prefix of the directory a copy is assembled in before its `rename(2)` onto
/// [`kept_dir`]. The scanner skips it: a copy interrupted half way must not be
/// read as a second template of the same version.
pub(crate) const KEEP_STAGING_PREFIX: &str = ".keep-";

/// GH #811 — the directory a colony keeps its own templates in: its kept
/// copies and what `add_templates` registers.
///
/// "A colony keeps its OWN copy" is a statement about ownership, and the
/// library a colony is pointed at is not always its own. Measured in the
/// integration pass of 2026-09-23: the scenario runner boots throwaway
/// colonies with `--templates templates` (the repository's library, outside
/// every colony root), and the keep wrote `templates/local/memory-hive@3.4.1/`
/// into the repository. So the rule is the colony root's: a library that lies
/// under the colony root (`<root>/templates`, or any `--templates` inside it)
/// is the colony's own and keeps `<library>/local` — unchanged for every
/// colony whose library is its own; a library outside it (a repository, a
/// shared checkout) is never written, and the colony keeps
/// `<root>/templates/local` instead, which the scanner reads beside the
/// library ([`scanner::scan_library_with_skips`](super::scanner)).
///
/// The inside case keeps the library's own spelling (`templates_root.join`),
/// so a path the scan walks and a path this function builds compare equal.
pub fn local_root(templates_root: &Path, colony_root: &Path) -> PathBuf {
    if lies_within(templates_root, colony_root) {
        templates_root.join("local")
    } else {
        colony_root.join("templates").join("local")
    }
}

/// Is `path` at or below `root`? Lexically first; then canonicalised, which
/// absorbs a relative `--templates` (the scenario runner passes `templates`
/// with the repository as its working directory) and symlinked roots. A path
/// that does not exist yet and is not lexically inside counts as outside —
/// the conservative answer, since the other one writes into it.
fn lies_within(path: &Path, root: &Path) -> bool {
    if path.is_absolute() == root.is_absolute() && path.starts_with(root) {
        return true;
    }
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => p.starts_with(r),
        _ => false,
    }
}

/// GH #811 — where a colony keeps its own copy of a version it instantiated:
/// the same formula `add_templates` uses (`mutation::register`), pulled into
/// one place so the two cannot drift apart. `local` is [`local_root`].
pub(crate) fn kept_dir(local: &Path, name: &str, version: &str) -> PathBuf {
    local.join(format!("{name}@{version}"))
}

/// Every regular file below `root`, as sorted paths relative to it.
fn relative_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(root.join(&rel))? {
            let entry = entry?;
            let child = rel.join(entry.file_name());
            let meta = std::fs::metadata(entry.path())?;
            if meta.is_dir() {
                stack.push(child);
            } else if meta.is_file() {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Do two template directories hold the same files with the same bytes?
///
/// Sorted relative file list plus a byte comparison, recursive. No hash: the
/// crate carries no digest dependency and a template is a handful of small
/// JSON files, so reading both is cheaper than adding one (AGENTS.md rule 6).
pub(crate) fn trees_equal(a: &Path, b: &Path) -> std::io::Result<bool> {
    let files_a = relative_files(a)?;
    let files_b = relative_files(b)?;
    if files_a != files_b {
        return Ok(false);
    }
    for rel in &files_a {
        if std::fs::read(a.join(rel))? != std::fs::read(b.join(rel))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One copy the colony has to make: the row, where its bytes are now, and
/// where the colony keeps them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KeepStep {
    /// The row's stable id — the copy takes it over, the row is repointed.
    pub template_id: String,
    /// Template name.
    pub name: String,
    /// Template version (always set: see [`plan_keep`]).
    pub version: String,
    /// The directory the row points at today.
    pub from: PathBuf,
    /// [`kept_dir`] of the row.
    pub to: PathBuf,
}

/// Pure: which rows need a copy, from where, to where.
///
/// Only versioned rows: an unversioned template has no address a way back
/// could name (`name@version` is the only pinned form), so there is nothing a
/// copy would make reachable (OR-T5). A referenced version with no row is
/// left alone — there are no bytes to keep. A row that already sits on its
/// kept directory needs nothing.
pub(crate) fn plan_keep(
    local: &Path,
    rows: &[TemplateEntry],
    referenced: &[(String, String)],
) -> Vec<KeepStep> {
    let mut seen = std::collections::BTreeSet::new();
    let mut steps = Vec::new();
    for (name, version) in referenced {
        if !seen.insert((name.clone(), version.clone())) {
            continue;
        }
        let Some(row) = rows
            .iter()
            .find(|r| &r.name == name && r.version.as_deref() == Some(version.as_str()))
        else {
            continue;
        };
        let to = kept_dir(local, name, version);
        if row.filesystem_path == to {
            continue;
        }
        steps.push(KeepStep {
            template_id: row.template_id.clone(),
            name: name.clone(),
            version: version.clone(),
            from: row.filesystem_path.clone(),
            to,
        });
    }
    steps
}

/// Copy `from` into `staging`, `template.json` LAST: a directory without its
/// marker is not a template to any reader, so a half-written copy is never
/// taken for one.
fn copy_tree(from: &Path, staging: &Path) -> std::io::Result<()> {
    let files = relative_files(from)?;
    std::fs::create_dir_all(staging)?;
    let marker = Path::new("template.json");
    for rel in files.iter().filter(|r| r.as_path() != marker) {
        let dst = staging.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from.join(rel), dst)?;
    }
    std::fs::copy(from.join(marker), staging.join(marker))?;
    Ok(())
}

fn invalid(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

/// Make one copy. Returns the row as it must now be persisted (on the kept
/// directory), or `None` when a DIFFERENT tree already lies at the kept
/// directory — then the row is left alone and the scanner's rule decides at
/// the next rescan (the kept copy wins, the other is skipped by name).
///
/// The source is checked first: if the directory the row points at no longer
/// declares this `name@version` (the library was swapped underneath it), its
/// bytes are another version's and copying them under this version's name
/// would be the one thing worse than keeping nothing.
pub(crate) fn execute_keep(step: &KeepStep) -> std::io::Result<Option<ScannedTemplate>> {
    let source = parse_template_json(&step.from.join("template.json"))
        .map_err(|e| invalid(format!("{e}")))?;
    if source.name != step.name || source.version.as_deref() != Some(step.version.as_str()) {
        return Err(invalid(format!(
            "{} no longer holds {}@{} (it declares {}@{}); nothing to keep",
            step.from.display(),
            step.name,
            step.version,
            source.name,
            source.version.as_deref().unwrap_or("(no version)")
        )));
    }
    if step.to.exists() {
        if trees_equal(&step.from, &step.to)? {
            return parse_template_json(&step.to.join("template.json"))
                .map(Some)
                .map_err(|e| invalid(format!("{e}")));
        }
        tracing::warn!(
            template = %step.name,
            version = %step.version,
            kept = %step.to.display(),
            row = %step.from.display(),
            "templates keep: a different tree already lies at the kept directory; \
             the row is left alone and the next rescan keeps the kept copy"
        );
        return Ok(None);
    }
    let local = step
        .to
        .parent()
        .ok_or_else(|| invalid("kept directory has no parent".into()))?;
    std::fs::create_dir_all(local)?;
    let staging = local.join(format!("{KEEP_STAGING_PREFIX}{}", Uuid::now_v7()));
    let made = copy_tree(&step.from, &staging).and_then(|()| std::fs::rename(&staging, &step.to));
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    parse_template_json(&step.to.join("template.json"))
        .map(Some)
        .map_err(|e| invalid(format!("{e}")))
}

/// Plan and make every copy `referenced` asks for; return `(template_id, row)`
/// for each row that has to be repointed. Failures are logged and skipped:
/// keeping is a guarantee about the NEXT library swap, and nothing that is
/// running right now depends on it.
pub(crate) fn keep_all(
    local: &Path,
    rows: &[TemplateEntry],
    referenced: &[(String, String)],
) -> Vec<(String, ScannedTemplate)> {
    keep_all_with(local, rows, referenced, &|_| {})
}

/// [`keep_all`], calling `before_step` before each copy is made (GH #1172:
/// the committed-mutation keep beats the watchdog there, one beat per kept
/// version).
pub(crate) fn keep_all_with(
    local: &Path,
    rows: &[TemplateEntry],
    referenced: &[(String, String)],
    before_step: &dyn Fn(&KeepStep),
) -> Vec<(String, ScannedTemplate)> {
    let mut out = Vec::new();
    for step in plan_keep(local, rows, referenced) {
        before_step(&step);
        match execute_keep(&step) {
            Ok(Some(scanned)) => {
                tracing::info!(
                    template = %step.name,
                    version = %step.version,
                    kept = %step.to.display(),
                    "templates keep: the colony holds its own copy of an instantiated version"
                );
                out.push((step.template_id, scanned));
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(
                template = %step.name,
                version = %step.version,
                from = %step.from.display(),
                error = %e,
                "templates keep: the version could not be kept"
            ),
        }
    }
    out
}

/// GH #811: the keep that follows a committed mutation -- copy every version
/// it instantiated into `local` and repoint each row, durably, before the
/// mutation's verdict goes back (the next mutation reads its template snapshot
/// from `colony.db`). Failures are logged by [`keep_all`] and the mutation
/// stands.
pub(crate) async fn keep_after_commit(
    local: PathBuf,
    rows: Vec<TemplateEntry>,
    instantiated: Vec<(String, String)>,
    log_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    now: i64,
    pulse: &crate::watchdog::WorkPulse,
) {
    keep_after_commit_paced(local, rows, instantiated, log_tx, now, pulse, |_| {}).await
}

/// [`keep_after_commit`] with `pace` run inside every copy step -- the seam a
/// test uses to make a copy as slow as a contended disk makes it.
async fn keep_after_commit_paced(
    local: PathBuf,
    rows: Vec<TemplateEntry>,
    instantiated: Vec<(String, String)>,
    log_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    now: i64,
    pulse: &crate::watchdog::WorkPulse,
    pace: impl Fn(&KeepStep) + Send + Sync + 'static,
) {
    // GH #1172: the copies are synchronous file work, minutes on a contended
    // disk for a large build order -- off the colony's worker (AGENTS.md rule
    // 13), with one beat per kept version under a label that names it. The
    // beat comes before each copy, so a copy that never returns is still a
    // silent, stuck work item.
    let step_pulse = pulse.clone();
    let copies = tokio::task::spawn_blocking(move || {
        keep_all_with(&local, &rows, &instantiated, &|step| {
            step_pulse
                .with_label(crate::watchdog::WorkItem::new(format!(
                    "{} op=keep template={}@{}",
                    step_pulse.label(),
                    step.name,
                    step.version
                )))
                .tick();
            pace(step);
        })
    })
    .await;
    let kept = match copies {
        Ok(kept) => kept,
        Err(e) => {
            tracing::warn!(error = %e, "templates keep: the copy task ended early; no row was repointed");
            return;
        }
    };
    for (template_id, kept) in kept {
        pulse.tick();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = log_tx
            .send(super::upsert_op(template_id, &kept, now, Some(tx)))
            .await;
        // Durable before the verdict goes back: the next mutation reads its
        // template snapshot from `colony.db`. The wait does not beat, because
        // a writer that never acknowledges is a wedge.
        let _ = rx.await;
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A copy a crash interrupted: `.keep-<uuid>` directories directly under
/// `local`. The scanner never reads them, and nobody else would remove them
/// (T2 review M2). Only this function's callers run it — boot and the rescan
/// door, inside the colony task or before it exists — so no copy of this
/// colony is being assembled at the same moment.
fn sweep_interrupted_copies(local: &Path) {
    let Ok(entries) = std::fs::read_dir(local) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(KEEP_STAGING_PREFIX) {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => tracing::info!(
                path = %entry.path().display(),
                "templates keep: removed a copy an earlier run left half-made"
            ),
            Err(e) => tracing::warn!(
                path = %entry.path().display(),
                error = %e,
                "templates keep: a half-made copy could not be removed"
            ),
        }
    }
}

/// Keep every version a registry row names (its own stamp and every hop of its
/// chain), and repoint the rows. Boot and the rescan door call it; the
/// mutation door calls [`keep_all`] with what the mutation itself stamped.
/// `local` is the colony's own directory, [`local_root`].
///
/// Same `impl Future + Send` shape as `apply_scan_result`: the `&ColonyDb`
/// borrow and all filesystem work stay in the synchronous prologue, the
/// returned future captures only the writer channel and owned rows.
pub fn keep_referenced_versions(
    local: &Path,
    db: &ColonyDb,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    sweep_interrupted_copies(local);
    let referenced = db.read_referenced_template_versions().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "templates keep: registry provenance unreadable");
        Vec::new()
    });
    let rows: Vec<TemplateEntry> = db
        .read_templates()
        .unwrap_or_default()
        .into_iter()
        .map(|r| TemplateEntry {
            template_id: r.template_id,
            name: r.name,
            version: r.version,
            filesystem_path: PathBuf::from(r.filesystem_path),
        })
        .collect();
    let kept = keep_all(local, &rows, &referenced);
    let writer_tx = db.writer_tx.clone();
    let queue_depth = db.queue_depth.clone();
    async move {
        let now = unix_now();
        for (template_id, scanned) in kept {
            let (tx, rx) = tokio::sync::oneshot::channel();
            super::send_op_via(
                &writer_tx,
                &queue_depth,
                super::upsert_op(template_id, &scanned, now, Some(tx)),
            )
            .await;
            let _ = rx.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn entry(id: &str, name: &str, version: Option<&str>, path: &Path) -> TemplateEntry {
        TemplateEntry {
            template_id: id.into(),
            name: name.into(),
            version: version.map(str::to_string),
            filesystem_path: path.to_path_buf(),
        }
    }

    #[test]
    fn trees_equal_compares_file_lists_and_bytes() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a");
        let b = td.path().join("b");
        for d in [&a, &b] {
            write(d, "template.json", "{}");
            write(d, "x/config.json", "1");
        }
        assert!(trees_equal(&a, &b).unwrap());
        write(&b, "x/config.json", "2");
        assert!(!trees_equal(&a, &b).unwrap(), "same files, other bytes");
        write(&b, "x/config.json", "1");
        write(&b, "y/config.json", "1");
        assert!(!trees_equal(&a, &b).unwrap(), "one file more");
    }

    #[test]
    fn plan_keep_takes_versioned_rows_off_their_kept_directory_only() {
        let root = Path::new("/lib");
        let rows = vec![
            entry("t1", "a", Some("1.0.0"), Path::new("/lib/a")),
            entry("t2", "b", Some("2.0.0"), &kept_dir(root, "b", "2.0.0")),
            entry("t3", "c", None, Path::new("/lib/c")),
        ];
        let referenced = vec![
            ("a".to_string(), "1.0.0".to_string()),
            ("a".to_string(), "1.0.0".to_string()),
            ("b".to_string(), "2.0.0".to_string()),
            ("z".to_string(), "9.9.9".to_string()),
        ];
        let steps = plan_keep(root, &rows, &referenced);
        assert_eq!(
            steps,
            vec![KeepStep {
                template_id: "t1".into(),
                name: "a".into(),
                version: "1.0.0".into(),
                from: PathBuf::from("/lib/a"),
                to: kept_dir(root, "a", "1.0.0"),
            }]
        );
    }

    #[test]
    fn execute_keep_copies_reuses_and_refuses_a_swapped_source() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        let shipped = root.join("a");
        write(
            &shipped,
            "template.json",
            r#"{"name":"a","version":"1.0.0"}"#,
        );
        write(&shipped, "config.json", "{}");
        write(&shipped, "inner/config.json", "{}");
        let local = root.join("local");
        let step = plan_keep(
            &local,
            &[entry("t1", "a", Some("1.0.0"), &shipped)],
            &[("a".into(), "1.0.0".into())],
        )
        .remove(0);

        let kept = execute_keep(&step).unwrap().expect("copied");
        assert_eq!(kept.filesystem_path, kept_dir(&local, "a", "1.0.0"));
        assert!(trees_equal(&shipped, &kept.filesystem_path).unwrap());
        let residue: Vec<_> = std::fs::read_dir(root.join("local"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(KEEP_STAGING_PREFIX)
            })
            .collect();
        assert!(residue.is_empty(), "staging residue: {residue:?}");

        // Idempotent: an identical copy is reused.
        assert!(execute_keep(&step).unwrap().is_some());

        // A different tree at the kept directory: the row is left alone.
        write(&shipped, "config.json", r#"{"changed":true}"#);
        assert!(execute_keep(&step).unwrap().is_none());

        // The library swapped underneath the row: nothing is copied.
        std::fs::remove_dir_all(kept_dir(&local, "a", "1.0.0")).unwrap();
        write(
            &shipped,
            "template.json",
            r#"{"name":"a","version":"1.1.0"}"#,
        );
        assert!(execute_keep(&step).is_err());
        assert!(!kept_dir(&local, "a", "1.0.0").exists());
    }

    #[test]
    fn a_library_the_colony_does_not_own_is_never_its_local_root() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("colony");
        let own = root.join("templates");
        let inside = root.join("library");
        let outside = td.path().join("shared/templates");
        for d in [&own, &inside, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(local_root(&own, &root), own.join("local"));
        assert_eq!(local_root(&inside, &root), inside.join("local"));
        assert_eq!(
            local_root(&outside, &root),
            root.join("templates").join("local"),
            "a library outside the colony root is never written"
        );
        // Not there yet and not lexically inside: outside.
        assert_eq!(
            local_root(&td.path().join("nowhere"), &root),
            root.join("templates").join("local")
        );
    }

    #[test]
    fn a_half_made_copy_is_swept_and_nothing_else() {
        let td = TempDir::new().unwrap();
        let local = td.path().join("local");
        write(&local, ".keep-0190/template.json", "{}");
        write(&local, "a@1.0.0/template.json", "{}");
        sweep_interrupted_copies(&local);
        assert!(!local.join(".keep-0190").exists());
        assert!(local.join("a@1.0.0/template.json").is_file());
    }

    /// GH #1172 -- `n` shipped versions under `lib/`, their rows and the
    /// `(name, version)` list a committed mutation would have stamped.
    fn shipped_versions(root: &Path, n: usize) -> (Vec<TemplateEntry>, Vec<(String, String)>) {
        let mut rows = Vec::new();
        let mut referenced = Vec::new();
        for i in 0..n {
            let name = format!("t{i}");
            let dir = root.join("lib").join(&name);
            write(
                &dir,
                "template.json",
                &format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            );
            write(&dir, "config.json", "{}");
            rows.push(entry(&format!("id-{i}"), &name, Some("1.0.0"), &dir));
            referenced.push((name, "1.0.0".to_string()));
        }
        (rows, referenced)
    }

    /// GH #1172 -- the colony writer, reduced to what the keep needs from it:
    /// every template upsert is acknowledged once it has been taken. A tokio
    /// task, so the keep's wait for the ack stays on the paused clock.
    fn acking_writer(
        mut rx: tokio::sync::mpsc::Receiver<crate::persist::writer::ColonyWriteOp>,
    ) -> tokio::task::JoinHandle<usize> {
        tokio::spawn(async move {
            let mut upserts = 0;
            while let Some(op) = rx.recv().await {
                if let crate::persist::writer::ColonyWriteOp::UpsertTemplate { ack, .. } = op {
                    upserts += 1;
                    if let Some(ack) = ack {
                        let _ = ack.send(());
                    }
                }
            }
            upserts
        })
    }

    /// The step of the virtual clock while a copy is held.
    const STEP: std::time::Duration = std::time::Duration::from_millis(1);

    /// Copies the test holds (welle-val2 v2-fix-b, #1199): the pace of a held
    /// copy reports its `name@version`, then blocks until the test lets it go
    /// -- a copy as slow as the test says, never a guessed sleep. Dropping the
    /// [`Held`] lets every held copy go, so a failed test does not keep the
    /// runtime's blocking pool.
    ///
    /// Virtual time (AGENTS.md rule 11, time logic): the keep waits on one
    /// `spawn_blocking` and tokio channels, the supervisor on tokio timers and
    /// channels, and a running blocking task holds the paused clock -- so
    /// while a copy is held the clock moves only by [`advance_by`] and
    /// [`run_clock`], tick by tick of the supervisor.
    struct Held {
        begun: tokio::sync::mpsc::UnboundedReceiver<String>,
        release: Option<std::sync::mpsc::Sender<()>>,
    }

    /// A pace that holds every copy `which` names, and the test's side of it.
    fn hold(
        which: impl Fn(&KeepStep) -> bool + Send + Sync + 'static,
    ) -> (impl Fn(&KeepStep) + Send + Sync + 'static, Held) {
        let (begun_tx, begun) = tokio::sync::mpsc::unbounded_channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = std::sync::Mutex::new(released);
        let pace = move |step: &KeepStep| {
            if which(step) {
                let _ = begun_tx.send(format!("{}@{}", step.name, step.version));
                // `Err`: the test has let go of every copy.
                let _ = released.lock().unwrap().recv();
            }
        };
        let held = Held {
            begun,
            release: Some(release),
        };
        (pace, held)
    }

    impl Held {
        /// The next held copy that has begun, driving `keep` meanwhile.
        async fn begun_under<F: std::future::Future + Unpin>(&mut self, keep: &mut F) -> String {
            tokio::select! {
                biased;
                _ = keep => panic!("the keep ended before the held copy began"),
                copy = self.begun.recv() => copy.expect("the pace lives as long as the keep"),
            }
        }

        /// The next held copy that has begun, for a keep another task drives.
        async fn begun(&mut self) -> String {
            self.begun
                .recv()
                .await
                .expect("the keep ended before the held copy began")
        }

        /// Lets the one copy that is held now go.
        fn release_one(&self) {
            if let Some(tx) = &self.release {
                tx.send(()).expect("a held copy waits for its release");
            }
        }

        /// Lets every held copy go, now and from here on.
        fn release_all(&mut self) {
            self.release = None;
        }
    }

    /// Lets `by` of virtual time pass in [`STEP`]s while `keep` runs beside
    /// it -- a held copy that takes `by`. The keep must not end meanwhile.
    async fn advance_by<F: std::future::Future + Unpin>(keep: &mut F, by: std::time::Duration) {
        let mut ran = std::time::Duration::ZERO;
        while ran < by {
            tokio::select! {
                biased;
                _ = &mut *keep => panic!("the keep ended while a copy was held"),
                () = std::future::ready(()) => {}
            }
            tokio::time::advance(STEP).await;
            ran += STEP;
        }
    }

    /// Drives `fut` on the paused clock in [`STEP`]s while a copy is held. A
    /// future still pending after [`meclaw_testing::deadline::HANG`] of
    /// virtual time is a hang, never a verdict on speed.
    async fn run_clock<F: std::future::Future>(what: &str, fut: F) -> F::Output {
        let mut ran = std::time::Duration::ZERO;
        tokio::pin!(fut);
        loop {
            tokio::select! {
                biased;
                out = &mut fut => return out,
                () = std::future::ready(()) => {}
            }
            assert!(
                ran < meclaw_testing::deadline::HANG,
                "HANG: {what} (virtual {ran:?})"
            );
            tokio::time::advance(STEP).await;
            ran += STEP;
        }
    }

    /// GH #1172 -- a watchdog at a 10 ms period with the shipped threshold of
    /// five: a 50 ms window and a 500 ms work-item budget. The returned pulse
    /// has already declared the mutation as the work item -- after the
    /// supervisor armed, because it drops the beats from before its arming.
    ///
    /// The supervisor and the pulse read the CPU denial through the seam,
    /// from a host that denies nothing (GH #1199): read from the kernel, the
    /// denial of the machine running the test was deducted from a silence on
    /// the virtual clock, and the verdict was that machine's (80 % quota on
    /// build01: the hung copy judged with `cpu_denied` > 0 in 2 of 5 runs).
    async fn supervised() -> (
        crate::watchdog::WorkPulse,
        tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
    ) {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel(8);
        let (trip_tx, trip_rx) = tokio::sync::mpsc::channel(1024);
        let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
        let _ = armed_tx.send(());
        let host: crate::watchdog::DenialSource =
            std::sync::Arc::new(|| crate::watchdog::RunDelays::from_rows([(1, 0)]));
        let read = host.clone();
        tokio::spawn(crate::watchdog::run_watchdog_reading_denial(
            hb_rx,
            trip_tx,
            5,
            std::time::Duration::from_millis(10),
            armed_rx,
            crate::watchdog::WatchdogOnTrip::Exit,
            None,
            None,
            move || read(),
        ));
        let pulse = crate::watchdog::WorkPulse::new(
            Some(hb_tx),
            crate::watchdog::WorkItem::new("mutation test scope=/t"),
        )
        .with_denial(host);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        pulse.tick();
        (pulse, trip_rx)
    }

    /// GH #1172, the defect: the keep after a committed mutation is the longest
    /// stretch of a mutation after its last cell, and it was silent. 24 versions
    /// at 40 ms a copy -- a contended disk, each copy shorter than one window --
    /// are 960 ms of honest work, almost twice the budget. The lab trip that
    /// opened the issue sat in exactly this stretch: the label still said
    /// `add_nodes[76/76]` while the `templates keep` lines were logged.
    ///
    /// Each copy is held for 40 ms of virtual time ([`Held`]), so the
    /// supervisor sees exactly that lag, tick by tick.
    #[tokio::test(start_paused = true)]
    async fn a_long_keep_after_a_commit_is_not_a_stuck_work_item() {
        let td = TempDir::new().unwrap();
        let (rows, referenced) = shipped_versions(td.path(), 24);
        let local = td.path().join("local");
        let (log_tx, log_rx) = tokio::sync::mpsc::channel(64);
        let writer = acking_writer(log_rx);
        let (pulse, mut trips) = supervised().await;
        let (pace, mut held) = hold(|_| true);

        {
            let keep = keep_after_commit_paced(
                local.clone(),
                rows,
                referenced.clone(),
                &log_tx,
                0,
                &pulse,
                pace,
            );
            tokio::pin!(keep);
            for _ in 0..24 {
                held.begun_under(&mut keep).await;
                advance_by(&mut keep, std::time::Duration::from_millis(40)).await;
                held.release_one();
            }
            keep.await;
        }

        // Everything the supervisor said while the keep ran, read before the
        // heartbeat is dropped (a closed channel is a gone colony task).
        while let Ok(trip) = trips.try_recv() {
            assert!(
                !trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit),
                "a keep that makes progress must not end the process: {trip}"
            );
        }
        drop(log_tx);
        assert_eq!(writer.await.unwrap(), 24, "every kept row is repointed");
        for (name, version) in &referenced {
            assert!(
                kept_dir(&local, name, version)
                    .join("template.json")
                    .is_file()
            );
        }
        drop(pulse);
    }

    /// GH #1172, the guard: the beats are per kept version, so a copy that
    /// never returns is still a stuck work item -- fatal under `exit`, and
    /// charged to the version whose copy hangs.
    ///
    /// The copy of `t1` is held until the test has seen the fatal trip, so the
    /// trip can only come from the silence INSIDE that copy: anything that
    /// beats while it hangs (a background ticker for the copy task, a beat
    /// inside the copy) leaves no fatal trip and turns this red -- the wait
    /// for it ends at the `HANG` marker of the virtual clock. The label pins
    /// which version was charged.
    #[tokio::test(start_paused = true)]
    async fn a_hung_copy_in_the_keep_is_still_a_stuck_work_item() {
        let td = TempDir::new().unwrap();
        let (rows, referenced) = shipped_versions(td.path(), 4);
        let (log_tx, log_rx) = tokio::sync::mpsc::channel(64);
        let writer = acking_writer(log_rx);
        let (pulse, mut trips) = supervised().await;
        let local = td.path().join("local");
        let (pace, mut held) = hold(|step| step.name == "t1");

        let keep = tokio::spawn(async move {
            keep_after_commit_paced(local, rows, referenced, &log_tx, 0, &pulse, pace).await;
            pulse
        });

        assert_eq!(held.begun().await, "t1@1.0.0");
        let fatal = run_clock("a fatal trip while the copy of t1 hangs", async {
            loop {
                let trip = trips
                    .recv()
                    .await
                    .expect("the supervisor returned without a fatal trip");
                if trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit) {
                    return trip;
                }
            }
        })
        .await;
        held.release_all();
        assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
        assert_eq!(
            fatal.cpu_denied,
            std::time::Duration::ZERO,
            "the copy is judged on a host that denies nothing -- the verdict of every machine: {fatal}"
        );
        assert_eq!(
            fatal.work_item.as_ref().map(|w| w.as_str()),
            Some("mutation test scope=/t op=keep template=t1@1.0.0"),
            "the trip must be charged to the hung copy of t1: {fatal}"
        );
        let pulse = keep.await.unwrap();
        drop(pulse);
        assert_eq!(writer.await.unwrap(), 4);
    }
}
