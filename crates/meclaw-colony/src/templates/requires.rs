//! The `requires` block of a `template.json` (GH #292).
//!
//! A template declares the `ctx` keys and environment variables an instantiation
//! must supply, so that a builder learns a template's parameter surface by
//! reading it instead of by being rejected one key per attempt. The two
//! placeholder classes stay apart because they behave differently: `${ctx.X}`
//! resolves onto disk while staging, `${ENV}` stays a token and binds late.
//! Shape: `plans/composition-reference/spec.md` part 2 (:139-149).
//!
//! **Deliberate non-goal: no `templates` table column and no schema migration.**
//! The declaration is not indexed and not carried through `colony.db`. The
//! enforcement point (mutation validation, before staging) already reads the
//! template directory for `parse_subtree`, so it reads the declaration straight
//! from `template.json` there — a column would be a second copy that can drift.
//!
//! A template without a `requires` block requires nothing and keeps working
//! exactly as before; a malformed block is an error naming the file, and both
//! structs `deny_unknown_fields`, because a typo inside a contract block is the
//! very failure mode this declaration exists to remove.

use meclaw_core::JsonValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Error type for reading a template's `requires` declaration.
#[derive(Debug, thiserror::Error)]
pub enum RequiresError {
    /// The `template.json` could not be read.
    #[error("read {0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The `template.json` is not valid JSON.
    #[error("parse {0}: {1}")]
    Parse(PathBuf, String),
    /// The file parses, but its `requires` block does not have the declared shape
    /// (wrong type, or an unknown key — a typo in a contract block).
    #[error("malformed 'requires' block in {0}: {1}")]
    Malformed(PathBuf, String),
}

/// What a template needs at instantiation, by placeholder class.
///
/// Both maps are ordered so that error messages listing missing keys are stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateRequires {
    /// `${ctx.X}` keys the mutation must supply; resolved onto disk while staging.
    #[serde(default)]
    pub ctx: BTreeMap<String, RequiredKey>,
    /// Environment variables the instance needs; stays a token and binds late.
    #[serde(default)]
    pub env: BTreeMap<String, RequiredKey>,
}

/// One declared key: what it holds, whether it may be omitted, and why it exists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredKey {
    /// Declared value type (`"string"`, …). Absent means unconstrained.
    #[serde(rename = "type", default)]
    pub r#type: Option<String>,
    /// Whether the key must be supplied. Absent means `true` — a key is declared
    /// because it is needed, so requiring it is the default, not the exception.
    #[serde(default = "required_by_default")]
    pub required: bool,
    /// Why the template needs it; quoted verbatim when a mutation is refused.
    #[serde(default)]
    pub because: Option<String>,
}

/// The `required` default: a declared key is required unless it says otherwise.
fn required_by_default() -> bool {
    true
}

/// Read the `requires` declaration of the template rooted at `template_dir`.
///
/// Reads `<template_dir>/template.json`. An absent `requires` key yields
/// [`TemplateRequires::default()`] (empty maps, no error); a present but
/// malformed block yields [`RequiresError::Malformed`] naming the file.
pub fn read_requires(template_dir: &Path) -> Result<TemplateRequires, RequiresError> {
    let path = template_dir.join("template.json");
    let raw = std::fs::read_to_string(&path).map_err(|e| RequiresError::Io(path.clone(), e))?;
    let val: JsonValue = meclaw_core::serde_json::from_str(&raw)
        .map_err(|e| RequiresError::Parse(path.clone(), e.to_string()))?;
    let Some(block) = val.get("requires") else {
        return Ok(TemplateRequires::default());
    };
    meclaw_core::serde_json::from_value(block.clone())
        .map_err(|e| RequiresError::Malformed(path, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{RequiredKey, TemplateRequires, read_requires};
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn write_template(td: &TempDir, dir: &str, content: &str) -> PathBuf {
        let p = td.path().join(dir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("template.json"), content).unwrap();
        p
    }

    /// The exact JSON of GH #292 / `plans/composition-reference/spec.md` :139-149.
    #[test]
    fn reads_the_shape_the_issue_specifies() {
        let td = TempDir::new().unwrap();
        let dir = write_template(
            &td,
            "assistant",
            r#"{
            "name": "assistant",
            "requires": {
              "ctx": {
                "name": {"type": "string", "required": true,
                         "because": "the address segment and the name the assistant answers to"}
              },
              "env": {
                "OPENROUTER_API_KEY": {"because": "the assistant infers"}
              }
            }
        }"#,
        );
        let req = read_requires(&dir).unwrap();
        assert_eq!(req.ctx.len(), 1, "one declared ctx key");
        assert_eq!(req.env.len(), 1, "one declared env key");
        let name: &RequiredKey = req.ctx.get("name").expect("ctx key 'name'");
        assert_eq!(name.r#type.as_deref(), Some("string"));
        assert!(name.required);
        assert_eq!(
            name.because.as_deref(),
            Some("the address segment and the name the assistant answers to")
        );
        let key: &RequiredKey = req.env.get("OPENROUTER_API_KEY").expect("env key");
        assert_eq!(key.r#type, None);
        assert_eq!(key.because.as_deref(), Some("the assistant infers"));
    }

    /// Spec: "templates without a `requires` block keep working exactly as today."
    #[test]
    fn a_template_without_a_requires_block_requires_nothing() {
        let td = TempDir::new().unwrap();
        let dir = write_template(&td, "echo", r#"{"name":"echo","version":"1.0.0"}"#);
        let req = read_requires(&dir).unwrap();
        assert_eq!(req, TemplateRequires::default());
        assert!(req.ctx.is_empty() && req.env.is_empty());
    }

    /// A declared key is required unless it says otherwise — the `env` entry of
    /// the spec shape carries only a `because` and is still a requirement.
    #[test]
    fn a_key_without_required_defaults_to_required() {
        let td = TempDir::new().unwrap();
        let dir = write_template(
            &td,
            "assistant",
            r#"{"name":"assistant","requires":{"env":{"OPENROUTER_API_KEY":{"because":"the assistant infers"}},
                "ctx":{"model":{"required":false}}}}"#,
        );
        let req = read_requires(&dir).unwrap();
        assert!(
            req.env["OPENROUTER_API_KEY"].required,
            "absent `required` means required"
        );
        assert!(!req.ctx["model"].required, "an explicit false stays false");
    }

    #[test]
    fn a_requires_that_is_not_an_object_is_an_error_naming_the_file() {
        let td = TempDir::new().unwrap();
        let dir = write_template(&td, "broken", r#"{"name":"broken","requires":["ctx"]}"#);
        let err = read_requires(&dir).unwrap_err();
        let rendered = err.to_string();
        let named: &Path = &dir.join("template.json");
        assert!(
            rendered.contains(&named.display().to_string()),
            "the message must name the file it read, got {rendered}"
        );
    }
}
