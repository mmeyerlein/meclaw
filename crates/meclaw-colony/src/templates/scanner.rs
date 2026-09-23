//! `template.json`-Parser + `templates/`-Scanner (Spec overview Z.1102-1123, 1141-1145).

use meclaw_core::JsonValue;
use std::path::{Path, PathBuf};

/// Error type for template scanning and parsing.
#[derive(Debug, thiserror::Error)]
pub enum ScannerError {
    /// I/O error reading a file or directory.
    #[error("read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// JSON parse error or missing required field.
    #[error("parse {0}: {1}")]
    Parse(PathBuf, String),
    /// Two `template.json`s under `templates/` declare the same `name` AND the
    /// same `version` (GH #664). Two versions of one name are two entries of
    /// one class; the same version twice is an ambiguity.
    #[error(
        "duplicate template `{}`: {first} and {second} — a name and a version together must be unique so a pinned reference has one answer",
        class_ref(.name, .version)
    )]
    DuplicateVersion {
        /// The `name` both templates declare.
        name: String,
        /// The `version` both templates declare, `None` when both omit it.
        version: Option<String>,
        /// Directory of the template seen first.
        first: PathBuf,
        /// Directory of the template that collides with it.
        second: PathBuf,
    },
}

/// `name@version`, or the bare name when the entry declares none — the form a
/// reference is written in, so a refusal names what a caller would have typed.
fn class_ref(name: &str, version: &Option<String>) -> String {
    match version {
        Some(v) => format!("{name}@{v}"),
        None => name.to_string(),
    }
}

/// A `template.json` the scan READ and did not register, with the reason.
///
/// The scan walks a directory anyone may drop a file into, so a descriptor it
/// cannot use is a skip and not an aborted boot. A skip with no reason is the
/// thing this type exists to prevent (GH #668).
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedTemplate {
    /// Directory of the `template.json` that was skipped.
    pub filesystem_path: PathBuf,
    /// The `name` it declares.
    pub name: String,
    /// The `version` it declares, `None` when it declares none.
    pub version: Option<String>,
    /// Why the entry was not registered, in the words a reader gets.
    pub reason: String,
}

/// A successfully parsed `template.json` with its resolved filesystem path.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedTemplate {
    /// Template name (required field from `template.json`).
    pub name: String,
    /// Optional SemVer string (`major.minor.patch`).
    pub version: Option<String>,
    /// Parent directory of the `template.json` file.
    pub filesystem_path: PathBuf,
    /// Serialised JSON of the `description` field, or `"{}"` if absent.
    pub description_json: String,
    /// Serialised JSON of the `tags` array, or `"[]"` if absent.
    pub tags_json: String,
    /// Optional author string.
    pub author: Option<String>,
}

/// Walk `templates_root` recursively and return all [`ScannedTemplate`]s found.
///
/// Strategy: stack-based DFS. For each directory:
/// - If `template.json` is present → parse and collect it (do not recurse into sub-dirs).
/// - Otherwise → push all sub-directories onto the stack.
///
/// A `(name, version)` pair is unique across the whole tree: the second
/// `template.json` declaring an already-seen pair aborts the scan with
/// [`ScannerError::DuplicateVersion`]. Two versions of one name are two
/// entries of one class (GH #664). One exception (GH #811): when exactly one
/// of the two is the colony's kept copy `local/<name>@<version>/`, the kept
/// copy wins — an identical twin falls away, a changed one is skipped by name
/// under `template_version_immutable`.
///
/// A `version` the resolver cannot read is SKIPPED with a named reason rather
/// than registered (GH #668) — the same rule the registration door applies
/// since GH #664. Use [`scan_templates_dir_with_skips`] to read the skips.
///
/// Missing `templates_root` returns an empty `Vec` (not an error).
pub fn scan_templates_dir(templates_root: &Path) -> Result<Vec<ScannedTemplate>, ScannerError> {
    scan_templates_dir_with_skips(templates_root).map(|(found, _skipped)| found)
}

/// [`scan_templates_dir`], with the entries it did NOT register and why.
///
/// Same walk, same refusals; the second half of the pair is the scan's receipt
/// (GH #668). Every skip is also written to the log here, so a caller that
/// wants only the registry still leaves a trace behind.
///
/// The library alone, with `<templates_root>/local` as the colony's own
/// directory — the shape of a colony whose library is its own. A colony reads
/// its library through [`scan_library_with_skips`] with its
/// [`local_root`](crate::templates::keep::local_root).
pub fn scan_templates_dir_with_skips(
    templates_root: &Path,
) -> Result<(Vec<ScannedTemplate>, Vec<SkippedTemplate>), ScannerError> {
    scan_library_with_skips(templates_root, &templates_root.join("local"))
}

/// GH #811 — the library a colony was pointed at PLUS the directory it owns.
///
/// `local` is [`local_root`](crate::templates::keep::local_root): inside the
/// library when the library is the colony's own (then the one walk covers it),
/// `<root>/templates/local` when the library lies outside the colony root —
/// then it is walked as a second root, because the kept copies and the
/// registrations of that colony live there and nowhere in the library.
///
/// The order is deterministic (T2 review M1): every directory is read in name
/// order, and the colony's kept copies are decided FIRST, so a kept copy wins
/// against any number of shipped twins whichever the walk meets first, and two
/// shipped twins without a kept copy abort on the same pair every time.
pub fn scan_library_with_skips(
    templates_root: &Path,
    local: &Path,
) -> Result<(Vec<ScannedTemplate>, Vec<SkippedTemplate>), ScannerError> {
    let mut roots: Vec<&Path> = Vec::new();
    if templates_root.exists() {
        roots.push(templates_root);
    }
    if !local.starts_with(templates_root) && local.exists() {
        roots.push(local);
    }
    let mut out: Vec<ScannedTemplate> = Vec::new();
    let mut skipped: Vec<SkippedTemplate> = Vec::new();
    let mut found: Vec<ScannedTemplate> = Vec::new();
    for root in roots {
        walk_template_dirs(root, &mut found, &mut skipped)?;
    }
    // GH #811 / T2 review M1: the kept copies first, the rest in walk order.
    // A stable sort, so the walk order (names, depth-first) stays within each
    // half.
    found.sort_by_key(|t| !is_kept_copy(local, t));
    // Uniqueness is by `(name, version)` (GH #664). A bare-name `ref` still has
    // exactly one answer — the rule for it is the resolver (highest version,
    // R3), not an aborted scan, which is what ruling Q7 (GH #277) used to buy
    // here and what the resolver could always do. The DFS stops descending at
    // a `template.json`, so a nested marker can only collide when it sits in a
    // *sibling* subtree — never as a child of another template.
    let mut seen: std::collections::HashMap<(String, Option<String>), PathBuf> =
        std::collections::HashMap::new();
    for t in found {
        let key = (t.name.clone(), t.version.clone());
        if let Some(first) = seen.get(&key).cloned() {
            match kept_copy_decides(local, &t, &first) {
                KeptRule::Abort => {
                    return Err(ScannerError::DuplicateVersion {
                        name: t.name,
                        version: t.version,
                        first,
                        second: t.filesystem_path,
                    });
                }
                KeptRule::Keep { kept, other, skip } => {
                    // The kept copy stands in `out` whichever of the two came
                    // first (with the kept-first order above it always does).
                    if kept == t.filesystem_path {
                        if let Some(slot) = out.iter_mut().find(|o| o.filesystem_path == other) {
                            *slot = t.clone();
                        }
                        seen.insert(key, kept);
                    }
                    if let Some(reason) = skip {
                        tracing::warn!(
                            template = %class_ref(&t.name, &t.version),
                            path = %other.display(),
                            "templates scan skipped an entry: {reason}"
                        );
                        skipped.push(SkippedTemplate {
                            filesystem_path: other,
                            name: t.name,
                            version: t.version,
                            reason,
                        });
                    }
                    continue;
                }
            }
        }
        seen.insert(key, t.filesystem_path.clone());
        out.push(t);
    }
    Ok((out, skipped))
}

/// Is `t` the colony's kept copy of its own `name@version`?
fn is_kept_copy(local: &Path, t: &ScannedTemplate) -> bool {
    t.version
        .as_deref()
        .is_some_and(|v| t.filesystem_path == crate::templates::keep::kept_dir(local, &t.name, v))
}

/// Depth-first over `root`, every directory in name order: each directory
/// holding a `template.json` is parsed and not descended into. A version the
/// resolver cannot read is a named skip (GH #668), not an entry.
fn walk_template_dirs(
    root: &Path,
    found: &mut Vec<ScannedTemplate>,
    skipped: &mut Vec<SkippedTemplate>,
) -> Result<(), ScannerError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut dirs: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| ScannerError::Io(dir.clone(), e))? {
            let entry = entry.map_err(|e| ScannerError::Io(dir.clone(), e))?;
            let p = entry.path();
            if p.is_dir() {
                dirs.push((entry.file_name(), p));
            }
        }
        dirs.sort();
        // Reversed onto the stack, so the pop order is the name order.
        let mut descend: Vec<PathBuf> = Vec::new();
        for (name, p) in dirs {
            // GH #811: a copy the colony is assembling (or one a crash
            // interrupted) is not a template yet — `keep::execute_keep`
            // renames it onto `<local>/<name>@<version>/` when it is whole.
            if name
                .to_string_lossy()
                .starts_with(crate::templates::keep::KEEP_STAGING_PREFIX)
            {
                continue;
            }
            let tjson = p.join("template.json");
            if !tjson.is_file() {
                descend.push(p);
                continue;
            }
            let t = parse_template_json(&tjson)?;
            // GH #668: the same rule the registration door applies since GH
            // #664 (`mutation::register::parse_entry`). A version
            // `parse_simple_version` cannot read is a version no reference can
            // name: `@<that string>` is not a reference `resolve` parses, and
            // the bare name finds no parsable candidate either. Registering it
            // puts a row in the library that answers to nothing, which is
            // worse than not being there — so it is skipped, BY NAME, rather
            // than silently taken in. A skip and not an abort, because the
            // walk reads a directory anyone may write into and one hand-placed
            // descriptor must not cost a colony its boot.
            if let Some(v) = t.version.as_deref()
                && let Err(e) = crate::templates::parse_simple_version(v)
            {
                let reason = format!(
                    "declares the version '{v}', which is not a version a \
                     reference can name: {e}. It would sit in the library \
                     and answer to nothing"
                );
                tracing::warn!(
                    template = %t.name,
                    version = %v,
                    path = %t.filesystem_path.display(),
                    "templates scan skipped an entry: {reason}"
                );
                skipped.push(SkippedTemplate {
                    filesystem_path: t.filesystem_path,
                    name: t.name,
                    version: t.version,
                    reason,
                });
                continue;
            }
            found.push(t);
        }
        stack.extend(descend.into_iter().rev());
    }
    Ok(())
}

/// What the walk does with a second `template.json` of an already-seen
/// `(name, version)`.
enum KeptRule {
    /// Neither (or both) is the colony's kept copy: an ambiguity, the scan aborts.
    Abort,
    /// Exactly one is `local/<name>@<version>/`: it wins. `skip` carries the
    /// reason when the other tree differs; `None` when it is byte-identical and
    /// simply falls away.
    Keep {
        kept: PathBuf,
        other: PathBuf,
        skip: Option<String>,
    },
}

/// GH #811 — the kept copy wins.
///
/// A colony keeps every version it instantiated under
/// `local/<name>@<version>/` (`keep.rs`), so the shipped `templates/<name>/`
/// of the same version will stand beside it until the library moves on. That
/// is not an ambiguity: the kept copy is what the colony instantiated. If the
/// shipped tree is byte-identical it falls away silently; if it differs, the
/// library changed a version WITHOUT bumping it, and the shipped tree is
/// skipped by name under `template_version_immutable` — a skip and not an
/// abort, because one edited file in a library must not cost a colony its boot
/// (the GH #668 rule). Two duplicates of which neither is the kept copy still
/// abort: that is the ambiguity GH #664 pins.
fn kept_copy_decides(local: &Path, t: &ScannedTemplate, first: &Path) -> KeptRule {
    let Some(version) = t.version.as_deref() else {
        return KeptRule::Abort;
    };
    let kept_dir = crate::templates::keep::kept_dir(local, &t.name, version);
    let second = t.filesystem_path.as_path();
    let (kept, other) = match (first == kept_dir, second == kept_dir) {
        (true, false) => (first, second),
        (false, true) => (second, first),
        _ => return KeptRule::Abort,
    };
    let equal = crate::templates::keep::trees_equal(kept, other);
    let skip = match equal {
        Ok(true) => None,
        Ok(false) => Some(format!(
            "template_version_immutable: declares {}, which this colony keeps at {} \
             with different bytes. A stored version does not change; the kept copy \
             stays registered. Ship the change as a new version",
            class_ref(&t.name, &t.version),
            kept.display()
        )),
        Err(e) => Some(format!(
            "template_version_immutable: declares {}, which this colony keeps at {}; \
             the two trees could not be compared ({e}), so the kept copy stays registered",
            class_ref(&t.name, &t.version),
            kept.display()
        )),
    };
    KeptRule::Keep {
        kept: kept.to_path_buf(),
        other: other.to_path_buf(),
        skip,
    }
}

/// Parse a single `template.json` file into a [`ScannedTemplate`].
///
/// `path` must point to the `template.json` file itself; `filesystem_path` on the
/// returned struct is set to its parent directory.
pub fn parse_template_json(path: &Path) -> Result<ScannedTemplate, ScannerError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ScannerError::Io(path.into(), e))?;
    let val: JsonValue = meclaw_core::serde_json::from_str(&raw)
        .map_err(|e| ScannerError::Parse(path.into(), e.to_string()))?;
    let name = val
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ScannerError::Parse(path.into(), "'name' missing or not a string".into()))?
        .to_string();
    let version = val
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let author = val
        .get("author")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let description_json = val
        .get("description")
        .map(|v| meclaw_core::serde_json::to_string(v).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "{}".into());
    let tags_json = val
        .get("tags")
        .map(|v| meclaw_core::serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
        .unwrap_or_else(|| "[]".into());
    let filesystem_path = path
        .parent()
        .ok_or_else(|| ScannerError::Parse(path.into(), "no parent dir".into()))?
        .to_path_buf();
    Ok(ScannedTemplate {
        name,
        version,
        filesystem_path,
        description_json,
        tags_json,
        author,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_template(td: &TempDir, dir: &str, content: &str) -> PathBuf {
        let p = td.path().join(dir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("template.json"), content).unwrap();
        p.join("template.json")
    }

    #[test]
    fn parses_minimal_template_json() {
        let td = TempDir::new().unwrap();
        let p = write_template(&td, "echo", r#"{"name":"echo"}"#);
        let t = parse_template_json(&p).unwrap();
        assert_eq!(t.name, "echo");
        assert_eq!(t.version, None);
        assert_eq!(t.author, None);
    }

    #[test]
    fn parses_full_template_json() {
        let td = TempDir::new().unwrap();
        let p = write_template(
            &td,
            "llm@2.1.0",
            r#"{
            "name":"llm","version":"2.1.0",
            "description":{"purpose":"p","use_when":"u","not_in_scope":"n","examples":[]},
            "tags":["llm","openai"],
            "author":"alice"
        }"#,
        );
        let t = parse_template_json(&p).unwrap();
        assert_eq!(t.name, "llm");
        assert_eq!(t.version.as_deref(), Some("2.1.0"));
        assert_eq!(t.author.as_deref(), Some("alice"));
        assert!(t.tags_json.contains("openai"));
    }

    #[test]
    fn rejects_template_json_without_name() {
        let td = TempDir::new().unwrap();
        let p = write_template(&td, "broken", r#"{"version":"1.0"}"#);
        let err = parse_template_json(&p).unwrap_err();
        assert!(matches!(err, ScannerError::Parse(_, _)));
    }

    #[test]
    fn rejects_invalid_json() {
        let td = TempDir::new().unwrap();
        let p = write_template(&td, "broken", "not json");
        let err = parse_template_json(&p).unwrap_err();
        assert!(matches!(err, ScannerError::Parse(_, _)));
    }

    #[test]
    fn walk_finds_all_templates_in_subdirs() {
        let td = TempDir::new().unwrap();
        write_template(&td, "a", r#"{"name":"a"}"#);
        // Three digits on purpose since GH #668: a two-digit version is now
        // skipped, and this test is about depth, not about versions.
        write_template(&td, "b@1.0.0", r#"{"name":"b","version":"1.0.0"}"#);
        write_template(&td, "group/c", r#"{"name":"c"}"#);
        let mut found = scan_templates_dir(td.path()).unwrap();
        found.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(found.len(), 3);
        assert_eq!(
            found.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    /// GH #668 — the scan applies the same rule as the registration door.
    ///
    /// A `template.json` placed by hand with `"version": "1.0"` used to be
    /// scanned and registered, and was then unreachable: `resolve` parses a
    /// reference with `parse_simple_version`, so neither the bare name (which
    /// finds no parsable candidate) nor `name@1.0` (which is not a reference it
    /// can read) ever named the entry. Since #664 `add_templates` refuses such
    /// a version with `schema`; two doors must not give two answers.
    ///
    /// The scan SKIPS rather than aborts: a library is a directory anyone may
    /// put a file into, and one hand-written descriptor must not take the boot
    /// of a whole colony with it. What it may not do is disappear quietly.
    #[test]
    fn a_version_no_reference_can_name_is_skipped_with_its_reason() {
        let td = TempDir::new().unwrap();
        write_template(&td, "good", r#"{"name":"good","version":"1.0.0"}"#);
        write_template(&td, "half", r#"{"name":"half","version":"1.0"}"#);
        let (found, skipped) = scan_templates_dir_with_skips(td.path()).unwrap();
        assert_eq!(
            found.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["good"],
            "the entry no reference can name was registered anyway"
        );
        assert_eq!(skipped.len(), 1, "the skip left no trace: {skipped:?}");
        let s = &skipped[0];
        assert_eq!(s.name, "half");
        assert_eq!(s.version.as_deref(), Some("1.0"));
        assert!(
            s.reason.contains("1.0") && s.reason.contains("reference"),
            "the receipt does not say what was wrong: {}",
            s.reason
        );
        assert!(
            s.filesystem_path.ends_with("half"),
            "the receipt does not name the directory: {}",
            s.filesystem_path.display()
        );
        // And the plain door answers the same tree the same way.
        assert_eq!(scan_templates_dir(td.path()).unwrap().len(), 1);
    }

    /// A version that is absent stays absent — the unversioned entry is a
    /// documented shape (`local/<name>/`), not a broken one.
    #[test]
    fn a_template_without_a_version_is_still_scanned() {
        let td = TempDir::new().unwrap();
        write_template(&td, "bare", r#"{"name":"bare"}"#);
        let (found, skipped) = scan_templates_dir_with_skips(td.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn walk_empty_templates_dir_returns_empty() {
        let td = TempDir::new().unwrap();
        std::fs::create_dir(td.path().join("templates")).unwrap();
        let found = scan_templates_dir(&td.path().join("templates")).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn walk_missing_templates_dir_returns_empty() {
        let td = TempDir::new().unwrap();
        let found = scan_templates_dir(&td.path().join("nonexistent")).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn walk_skips_dirs_without_template_json() {
        let td = TempDir::new().unwrap();
        std::fs::create_dir_all(td.path().join("not_a_template/sub")).unwrap();
        write_template(&td, "real", r#"{"name":"real"}"#);
        let found = scan_templates_dir(td.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "real");
    }

    /// A `(name, version)` pair is an identity, not a label: a pinned `ref` must
    /// have exactly one answer, so two `template.json`s declaring the same pair
    /// abort the scan (GH #664; what Q7 in GH #277 really guarded).
    #[test]
    fn walk_refuses_two_templates_declaring_one_name_and_version() {
        let td = TempDir::new().unwrap();
        write_template(&td, "a", r#"{"name":"dup","version":"1.0.0"}"#);
        write_template(&td, "nested/b", r#"{"name":"dup","version":"1.0.0"}"#);
        let err = scan_templates_dir(td.path()).unwrap_err();
        assert!(
            matches!(err, ScannerError::DuplicateVersion { .. }),
            "expected DuplicateVersion, got {err:?}"
        );
        let rendered = err.to_string();
        let first = td.path().join("a").display().to_string();
        let second = td.path().join("nested/b").display().to_string();
        assert!(
            rendered.contains(&first) && rendered.contains(&second),
            "message must name both directories, got {rendered}"
        );
    }

    /// Two entries that both omit `version` are the same pair — `None` is a
    /// value of the key, not an exemption from it.
    #[test]
    fn walk_refuses_two_unversioned_templates_of_one_name() {
        let td = TempDir::new().unwrap();
        write_template(&td, "a", r#"{"name":"dup"}"#);
        write_template(&td, "nested/b", r#"{"name":"dup"}"#);
        let err = scan_templates_dir(td.path()).unwrap_err();
        assert!(
            matches!(err, ScannerError::DuplicateVersion { version: None, .. }),
            "expected DuplicateVersion with no version, got {err:?}"
        );
        assert!(
            err.to_string().contains("`dup`"),
            "an unversioned collision names the bare class, got {err}"
        );
    }

    /// Uniqueness is by `name@version`, exactly like the `templates` table's
    /// unique index `(name, COALESCE(version,''))`. The scan used to be stricter
    /// than that index; it no longer is (GH #664), because a bare name resolves
    /// through the resolver and not through the shape of the directory tree.
    #[test]
    fn walk_carries_two_versions_of_one_name_in_one_directory_pair() {
        let td = TempDir::new().unwrap();
        write_template(&td, "llm@1.0.0", r#"{"name":"llm","version":"1.0.0"}"#);
        write_template(&td, "llm@2.0.0", r#"{"name":"llm","version":"2.0.0"}"#);
        let found = scan_templates_dir(td.path()).unwrap();
        assert_eq!(found.len(), 2, "a class lost a version: {found:?}");
    }

    /// GH #811: the colony's kept copy `local/<name>@<version>/` and the
    /// shipped directory of the same version are one template when the bytes
    /// agree — the kept copy is the entry, the shipped twin falls away without
    /// a skip, whichever of the two the walk meets first.
    #[test]
    fn a_kept_copy_wins_over_an_identical_shipped_one() {
        for shipped_dir in ["screen", "zz-screen"] {
            let td = TempDir::new().unwrap();
            let body = r#"{"name":"screen","version":"1.0.0"}"#;
            let shipped = write_template(&td, shipped_dir, body);
            std::fs::write(shipped.parent().unwrap().join("config.json"), "{}").unwrap();
            let kept = write_template(&td, "local/screen@1.0.0", body);
            std::fs::write(kept.parent().unwrap().join("config.json"), "{}").unwrap();
            let (found, skipped) = scan_templates_dir_with_skips(td.path()).unwrap();
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(
                found[0].filesystem_path,
                td.path().join("local/screen@1.0.0")
            );
            assert!(
                skipped.is_empty(),
                "an identical twin is no skip: {skipped:?}"
            );
        }
    }

    /// GH #811: a shipped tree changed under a version the colony keeps is
    /// skipped BY NAME, not registered and not a reason to abort the boot.
    #[test]
    fn a_changed_shipped_tree_under_a_kept_version_is_skipped_by_name() {
        let td = TempDir::new().unwrap();
        let body = r#"{"name":"screen","version":"1.0.0"}"#;
        let shipped = write_template(&td, "screen", body);
        std::fs::write(shipped.parent().unwrap().join("config.json"), "{\"v\":2}").unwrap();
        let kept = write_template(&td, "local/screen@1.0.0", body);
        std::fs::write(kept.parent().unwrap().join("config.json"), "{}").unwrap();
        // A copy interrupted half way is not read at all.
        write_template(&td, "local/.keep-0000", body);
        let (found, skipped) = scan_templates_dir_with_skips(td.path()).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].filesystem_path,
            td.path().join("local/screen@1.0.0")
        );
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].filesystem_path, td.path().join("screen"));
        assert!(
            skipped[0].reason.contains("template_version_immutable")
                && skipped[0].reason.contains("screen@1.0.0"),
            "{}",
            skipped[0].reason
        );
    }

    /// T2 review M1: two shipped twins and the kept copy. The kept copy wins
    /// and BOTH twins fall away, whatever order the directory lists them in —
    /// before, a walk that met the two shipped ones first aborted the scan.
    #[test]
    fn a_kept_copy_wins_over_two_shipped_twins_in_any_order() {
        for (a, b) in [("aa-screen", "bb-screen"), ("zz-screen", "zy-screen")] {
            let td = TempDir::new().unwrap();
            let body = r#"{"name":"screen","version":"1.0.0"}"#;
            write_template(&td, a, body);
            write_template(&td, b, body);
            write_template(&td, "local/screen@1.0.0", body);
            let (found, skipped) = scan_templates_dir_with_skips(td.path()).unwrap();
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(
                found[0].filesystem_path,
                td.path().join("local/screen@1.0.0")
            );
            assert!(skipped.is_empty(), "{skipped:?}");
        }
    }

    /// GH #811: a colony whose library is not its own keeps its copies under
    /// its own root; the scan reads that directory as a second root, and the
    /// kept copy wins over the library's twin exactly as it does inside.
    #[test]
    fn a_local_root_outside_the_library_is_scanned_beside_it() {
        let lib = TempDir::new().unwrap();
        let colony = TempDir::new().unwrap();
        let local = colony.path().join("templates/local");
        let body = r#"{"name":"screen","version":"1.0.0"}"#;
        write_template(&lib, "screen", body);
        let old = r#"{"name":"screen","version":"0.9.0"}"#;
        let kept_old = local.join("screen@0.9.0");
        std::fs::create_dir_all(&kept_old).unwrap();
        std::fs::write(kept_old.join("template.json"), old).unwrap();
        let kept_new = local.join("screen@1.0.0");
        std::fs::create_dir_all(&kept_new).unwrap();
        std::fs::write(kept_new.join("template.json"), body).unwrap();
        let (found, skipped) = scan_library_with_skips(lib.path(), &local).unwrap();
        let mut paths: Vec<_> = found.iter().map(|t| t.filesystem_path.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec![kept_old, kept_new]);
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn walk_propagates_parse_error_with_path() {
        let td = TempDir::new().unwrap();
        write_template(&td, "broken", "not json");
        let err = scan_templates_dir(td.path()).unwrap_err();
        assert!(matches!(err, ScannerError::Parse(_, _)));
    }
}
