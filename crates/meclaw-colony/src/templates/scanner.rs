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
/// Missing `templates_root` returns an empty `Vec` (not an error).
pub fn scan_templates_dir(templates_root: &Path) -> Result<Vec<ScannedTemplate>, ScannerError> {
    if !templates_root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut stack = vec![templates_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| ScannerError::Io(dir.clone(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| ScannerError::Io(dir.clone(), e))?;
            let p = entry.path();
            if p.is_dir() {
                let tjson = p.join("template.json");
                if tjson.is_file() {
                    out.push(parse_template_json(&tjson)?);
                } else {
                    stack.push(p);
                }
            }
        }
    }
    Ok(out)
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
        write_template(&td, "b@1.0", r#"{"name":"b","version":"1.0"}"#);
        write_template(&td, "group/c", r#"{"name":"c"}"#);
        let mut found = scan_templates_dir(td.path()).unwrap();
        found.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(found.len(), 3);
        assert_eq!(
            found.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
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

    #[test]
    fn walk_propagates_parse_error_with_path() {
        let td = TempDir::new().unwrap();
        write_template(&td, "broken", "not json");
        let err = scan_templates_dir(td.path()).unwrap_err();
        assert!(matches!(err, ScannerError::Parse(_, _)));
    }
}
