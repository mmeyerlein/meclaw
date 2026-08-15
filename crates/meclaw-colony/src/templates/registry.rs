//! In-memory view of the templates registry for lookup operations.
//! Data source: `ColonyDb::read_templates()`; consumers hold their own
//! snapshot instance (mutation validator, bootstrap, rescan handler).

use super::version::parse_simple_version;
use std::path::PathBuf;

/// A single entry in the in-memory templates registry.
#[derive(Debug, Clone)]
pub struct TemplateEntry {
    /// Stable surrogate key (UUID-v7 from scan).
    pub template_id: String,
    /// Template name as declared in `template.json`.
    pub name: String,
    /// Optional semantic version `<major>.<minor>.<patch>`.
    pub version: Option<String>,
    /// Absolute filesystem path to the template directory.
    pub filesystem_path: PathBuf,
}

/// Snapshot of all known templates for a single request/mutation cycle.
///
/// Built from `ColonyDb::read_templates()` at bootstrap or after a rescan.
#[derive(Debug, Clone, Default)]
pub struct TemplatesRegistry {
    entries: Vec<TemplateEntry>,
}

/// Error returned by [`TemplatesRegistry::resolve`].
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// No template matching the given name (and optional version) exists.
    #[error("template not found: {0}")]
    NotFound(String),
    /// The `@<version>` part of the reference is not a valid `major.minor.patch` version.
    #[error("invalid version in reference {0:?}: {1}")]
    InvalidVersionRef(String, String),
}

impl TemplatesRegistry {
    /// Construct a registry from a list of entries (e.g. from `ColonyDb::read_templates`).
    pub fn from_entries(entries: Vec<TemplateEntry>) -> Self {
        Self { entries }
    }

    /// Number of entries in this registry snapshot.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the registry contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over all entries in this registry snapshot.
    pub fn entries_iter(&self) -> impl Iterator<Item = &TemplateEntry> {
        self.entries.iter()
    }

    /// Resolve a template reference of the form `<name>` or `<name>@<version>`.
    ///
    /// Resolution rules (R3):
    /// - With `@<version>`: exact match required; malformed version → `InvalidVersionRef`.
    /// - Without version: highest versioned entry wins; unversioned entry is picked only
    ///   if no versioned entries exist ("unversioniert < versioniert").
    pub fn resolve(&self, reference: &str) -> Result<&TemplateEntry, ResolveError> {
        let (name, ver_opt) = match reference.split_once('@') {
            Some((n, v)) => {
                let parsed = parse_simple_version(v).map_err(|e| {
                    ResolveError::InvalidVersionRef(reference.into(), e.to_string())
                })?;
                (n, Some(parsed))
            }
            None => (reference, None),
        };
        let candidates: Vec<&TemplateEntry> =
            self.entries.iter().filter(|e| e.name == name).collect();
        if candidates.is_empty() {
            return Err(ResolveError::NotFound(reference.into()));
        }
        match ver_opt {
            Some(target) => candidates
                .into_iter()
                .find(|e| {
                    e.version
                        .as_deref()
                        .and_then(|v| parse_simple_version(v).ok())
                        == Some(target)
                })
                .ok_or_else(|| ResolveError::NotFound(reference.into())),
            None => {
                // R3: unversioned < versioned — pick highest versioned if any exist.
                let mut versioned: Vec<(&TemplateEntry, (u64, u64, u64))> = candidates
                    .iter()
                    .filter_map(|e| {
                        e.version
                            .as_deref()
                            .and_then(|v| parse_simple_version(v).ok())
                            .map(|p| (*e, p))
                    })
                    .collect();
                if !versioned.is_empty() {
                    versioned.sort_by_key(|(_, v)| *v);
                    return Ok(versioned.last().unwrap().0);
                }
                candidates
                    .into_iter()
                    .find(|e| e.version.is_none())
                    .ok_or_else(|| ResolveError::NotFound(reference.into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, version: Option<&str>) -> TemplateEntry {
        TemplateEntry {
            template_id: format!("{name}-{}", version.unwrap_or("none")),
            name: name.into(),
            version: version.map(str::to_string),
            filesystem_path: PathBuf::from(format!("/t/{name}")),
        }
    }

    #[test]
    fn resolves_exact_name_at_version() {
        let r = TemplatesRegistry::from_entries(vec![
            entry("echo", Some("1.0.0")),
            entry("echo", Some("2.0.0")),
        ]);
        let e = r.resolve("echo@2.0.0").unwrap();
        assert_eq!(e.version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn resolves_unversioned_ref_to_highest_semver() {
        let r = TemplatesRegistry::from_entries(vec![
            entry("echo", Some("1.0.0")),
            entry("echo", Some("2.5.3")),
            entry("echo", Some("2.0.0")),
        ]);
        let e = r.resolve("echo").unwrap();
        assert_eq!(e.version.as_deref(), Some("2.5.3"));
    }

    #[test]
    fn unversioned_entry_is_less_than_any_versioned() {
        // R3: "unversioned < versioned" — if versioned ones exist, the highest versioned wins.
        let r = TemplatesRegistry::from_entries(vec![
            entry("echo", None),
            entry("echo", Some("0.0.1")),
        ]);
        let e = r.resolve("echo").unwrap();
        assert_eq!(e.version.as_deref(), Some("0.0.1"));
    }

    #[test]
    fn unversioned_picked_when_no_versioned_exists() {
        let r = TemplatesRegistry::from_entries(vec![entry("echo", None)]);
        let e = r.resolve("echo").unwrap();
        assert_eq!(e.version, None);
    }

    #[test]
    fn returns_not_found_for_unknown_template() {
        let r = TemplatesRegistry::default();
        assert!(matches!(r.resolve("nope"), Err(ResolveError::NotFound(_))));
    }

    #[test]
    fn returns_not_found_for_known_name_unknown_version() {
        let r = TemplatesRegistry::from_entries(vec![entry("echo", Some("1.0.0"))]);
        assert!(matches!(
            r.resolve("echo@9.9.9"),
            Err(ResolveError::NotFound(_))
        ));
    }

    #[test]
    fn rejects_malformed_version_in_reference() {
        let r = TemplatesRegistry::from_entries(vec![entry("echo", Some("1.0.0"))]);
        let err = r.resolve("echo@1.x").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidVersionRef(_, _)));
    }
}
