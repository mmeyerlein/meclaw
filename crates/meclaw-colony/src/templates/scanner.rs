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
/// entries of one class (GH #664).
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
pub fn scan_templates_dir_with_skips(
    templates_root: &Path,
) -> Result<(Vec<ScannedTemplate>, Vec<SkippedTemplate>), ScannerError> {
    if !templates_root.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut out = Vec::new();
    let mut skipped: Vec<SkippedTemplate> = Vec::new();
    // Uniqueness is by `(name, version)` (GH #664). A bare-name `ref` still has
    // exactly one answer — the rule for it is the resolver (highest version,
    // R3), not an aborted scan, which is what ruling Q7 (GH #277) used to buy
    // here and what the resolver could always do. The DFS below stops
    // descending at a `template.json`, so a nested marker can only collide when
    // it sits in a *sibling* subtree — never as a child of another template.
    let mut seen: std::collections::HashMap<(String, Option<String>), PathBuf> =
        std::collections::HashMap::new();
    let mut stack = vec![templates_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| ScannerError::Io(dir.clone(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| ScannerError::Io(dir.clone(), e))?;
            let p = entry.path();
            if p.is_dir() {
                let tjson = p.join("template.json");
                if tjson.is_file() {
                    let t = parse_template_json(&tjson)?;
                    // GH #668: the same rule the registration door applies
                    // since GH #664 (`mutation::register::parse_entry`). A
                    // version `parse_simple_version` cannot read is a version
                    // no reference can name: `@<that string>` is not a
                    // reference `resolve` parses, and the bare name finds no
                    // parsable candidate either. Registering it puts a row in
                    // the library that answers to nothing, which is worse than
                    // not being there — so it is skipped, BY NAME, rather than
                    // silently taken in. A skip and not an abort, because the
                    // walk reads a directory anyone may write into and one
                    // hand-placed descriptor must not cost a colony its boot.
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
                    let key = (t.name.clone(), t.version.clone());
                    if let Some(first) = seen.get(&key) {
                        return Err(ScannerError::DuplicateVersion {
                            name: t.name,
                            version: t.version,
                            first: first.clone(),
                            second: t.filesystem_path,
                        });
                    }
                    seen.insert(key, t.filesystem_path.clone());
                    out.push(t);
                } else {
                    stack.push(p);
                }
            }
        }
    }
    Ok((out, skipped))
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

    #[test]
    fn walk_propagates_parse_error_with_path() {
        let td = TempDir::new().unwrap();
        write_template(&td, "broken", "not json");
        let err = scan_templates_dir(td.path()).unwrap_err();
        assert!(matches!(err, ScannerError::Parse(_, _)));
    }
}
