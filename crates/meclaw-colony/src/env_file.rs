//! Root-`.env`-Loader. Classic Key=Value format (Spec overview Z.1235).
//! This module only parses `.env` into a key→value map. The actual `${VAR}` /
//! POSIX `${VAR:-default}` substitution and the `$${...}` escape live in
//! `crate::mutation::substitute` (`parse_env_token` + `expand`), applied to
//! mutation diffs at instantiation — not here.

use std::collections::HashMap;
use std::path::Path;

/// Errors that can occur while loading a `.env` file.
#[derive(Debug, thiserror::Error)]
pub enum EnvFileError {
    /// I/O error reading the file.
    #[error("read .env: {0}")]
    Io(#[from] std::io::Error),
    /// Parse error on a specific line.
    #[error("invalid .env line {line}: {msg}")]
    Parse { line: usize, msg: String },
}

/// Load a `.env` file from `path` and return a map of key→value pairs.
///
/// If the file does not exist, returns an empty map (not an error).
/// Lines starting with `#` or blank lines are skipped.
/// Values surrounded by double-quotes have the quotes stripped.
pub fn load_env(path: &Path) -> Result<HashMap<String, String>, EnvFileError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)?;
    let mut out = HashMap::new();
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| EnvFileError::Parse {
            line: idx + 1,
            msg: "no '=' separator".into(),
        })?;
        let key = k.trim().to_string();
        let mut val = v.trim().to_string();
        if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
            val = val[1..val.len() - 1].to_string();
        }
        out.insert(key, val);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(td: &TempDir, content: &str) -> std::path::PathBuf {
        let p = td.path().join(".env");
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn load_env_missing_file_returns_empty_map() {
        let td = TempDir::new().unwrap();
        let map = load_env(&td.path().join(".env")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn load_env_parses_simple_key_value_lines() {
        let td = TempDir::new().unwrap();
        let p = write(&td, "FOO=bar\nBAZ=qux\n");
        let map = load_env(&p).unwrap();
        assert_eq!(map.get("FOO"), Some(&"bar".to_string()));
        assert_eq!(map.get("BAZ"), Some(&"qux".to_string()));
    }

    #[test]
    fn load_env_skips_comments_and_blank_lines() {
        let td = TempDir::new().unwrap();
        let p = write(&td, "# comment\n\nFOO=bar\n# trailing\n");
        let map = load_env(&p).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn load_env_rejects_line_without_equals() {
        let td = TempDir::new().unwrap();
        let p = write(&td, "FOO=bar\nINVALID_NO_EQUALS\n");
        let err = load_env(&p).unwrap_err();
        assert!(matches!(err, EnvFileError::Parse { line: 2, .. }));
    }

    #[test]
    fn load_env_strips_surrounding_double_quotes() {
        let td = TempDir::new().unwrap();
        let p = write(&td, r#"FOO="with spaces""#);
        let map = load_env(&p).unwrap();
        assert_eq!(map.get("FOO"), Some(&"with spaces".to_string()));
    }
}
