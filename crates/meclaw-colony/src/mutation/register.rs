//! GH #440 — `add_templates`: a reusable template enters a RUNNING colony.
//!
//! The operation touches no cell, no edge and no path in the tree. That is why
//! it lives here instead of in `stage.rs`: everything there is about putting a
//! cell somewhere, and this puts a CLASS in the library.
//!
//! Two invariants carry the whole module:
//!
//! 1. **The target path is built, never taken.** `{templates_root}/local/<name>/`
//!    is composed from the resolved `--templates` root and the clamped name. No
//!    field of the body becomes a path segment, so there is nothing to escape
//!    from and nothing to sanitise.
//! 2. **The shipped library is out of reach.** Writing under `local/` and only
//!    there is what "the shipped library is off limits" means concretely — a
//!    declaration can never overwrite `talky`, because it never addresses the
//!    directory `talky` lives in.

use std::collections::BTreeMap;

use meclaw_core::serde_json::Value;

use crate::mutation::MutationError;

/// `^[a-z][a-z0-9-]{1,63}$`, hand-rolled: the workspace carries no regex crate
/// on this path and the pattern is small enough that a dependency would be the
/// larger change (AGENTS.md rule 6).
fn name_is_well_formed(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 2 || bytes.len() > 64 {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// A file path inside the template: relative, no climbing, no absolute form,
/// no empty segment.
fn file_path_is_contained(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && !rel.starts_with('\\')
        && rel
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// One `add_templates[]` entry, parsed and clamped. Nothing here touches the
/// filesystem — construction is the whole refusal surface.
#[derive(Debug, Clone)]
pub struct TemplateRegistration {
    /// The template's own name, `^[a-z][a-z0-9-]{1,63}$`. It is also the
    /// directory name under `{templates_root}/local/`, which is why the
    /// pattern forbids `/`, `.` and `..` rather than filtering them later.
    pub name: String,
    /// Relative path inside the template → file content, verbatim.
    pub files: BTreeMap<String, String>,
}

/// Parse and clamp one entry. Pre-destructive: every refusal that does not need
/// the registry happens here, before a single byte is written.
pub fn parse_entry(entry: &Value) -> Result<TemplateRegistration, MutationError> {
    let obj = entry
        .as_object()
        .ok_or_else(|| MutationError::Schema("add_templates[] entry must be an object".into()))?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MutationError::Schema("add_templates[].name missing".into()))?;
    if !name_is_well_formed(name) {
        return Err(MutationError::InvalidTemplateName(format!(
            "add_templates[].name '{name}' is not a template name. It becomes the \
             directory under the colony's local template root, so it must match \
             ^[a-z][a-z0-9-]{{1,63}}$ — the colony builds that path and never takes \
             one from the body"
        )));
    }
    let files_obj = obj
        .get("files")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            MutationError::Schema("add_templates[].files missing — a template is its files".into())
        })?;
    let mut files = BTreeMap::new();
    for (rel, body) in files_obj {
        if !file_path_is_contained(rel) {
            return Err(MutationError::InvalidTemplateName(format!(
                "add_templates[].files['{rel}'] climbs out of the template \
                 directory. Paths inside a template are relative and contain no \
                 '.', '..' or leading separator"
            )));
        }
        let text = body.as_str().ok_or_else(|| {
            MutationError::Schema(format!(
                "add_templates[].files['{rel}'] must be a string — a file is its bytes"
            ))
        })?;
        files.insert(rel.clone(), text.to_string());
    }
    if !files.contains_key("template.json") {
        return Err(MutationError::Schema(format!(
            "add_templates[] '{name}' carries no template.json. Without it the \
             scan registers nothing: the directory would land, the registry row \
             would be a claim about a template that is not one, and the next \
             rescan would drop it silently"
        )));
    }
    Ok(TemplateRegistration {
        name: name.to_string(),
        files,
    })
}

/// Refuse a name the registry already answers to — at this entry's position.
///
/// Separate from [`parse_entry`] because it needs the registry snapshot, which
/// a later manifest entry sees differently from an earlier one.
pub fn refuse_if_taken(
    reg: &TemplateRegistration,
    templates: &crate::templates::TemplatesRegistry,
) -> Result<(), MutationError> {
    if templates.resolve(&reg.name).is_ok() {
        return Err(MutationError::TemplateNameTaken(format!(
            "a template named '{}' is already registered. A duplicate name is a \
             hard scan failure (GH #277), so writing it would break the NEXT \
             rescan for everybody and nothing at this moment would say so — the \
             entry is refused here instead",
            reg.name
        )));
    }
    Ok(())
}

/// Write every registration atomically and return the registry rows to upsert.
///
/// Staging plus one `rename(2)` per entry, never a recursive copy into the live
/// library: a half-written `template.json` is exactly what a concurrent rescan
/// would pick up.
pub fn apply_registrations(
    regs: &[TemplateRegistration],
    templates_root: &std::path::Path,
    root: &std::path::Path,
    mutation_id: &str,
) -> Result<Vec<crate::templates::ScannedTemplate>, MutationError> {
    let staging = root.join(".staging-templates").join(mutation_id);
    let local = templates_root.join("local");
    let mut scanned = Vec::with_capacity(regs.len());

    let cleanup = |staging: &std::path::Path| {
        let _ = std::fs::remove_dir_all(staging);
    };

    for reg in regs {
        let stage_dir = staging.join(&reg.name);
        let write_all = || -> std::io::Result<()> {
            std::fs::create_dir_all(&stage_dir)?;
            for (rel, body) in &reg.files {
                let p = stage_dir.join(rel);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(p, body)?;
            }
            Ok(())
        };
        if let Err(e) = write_all() {
            cleanup(&staging);
            return Err(MutationError::Schema(format!(
                "add_templates[] '{}' could not be staged: {e}",
                reg.name
            )));
        }

        // The registry row is derived from the template.json that was just
        // written, by the SAME parser the scan uses — so a registration and a
        // rescan cannot disagree about what was registered.
        let parsed = match crate::templates::parse_template_json(&stage_dir.join("template.json")) {
            Ok(p) => p,
            Err(e) => {
                cleanup(&staging);
                return Err(MutationError::Schema(format!(
                    "add_templates[] '{}' carries a template.json the scanner refuses: {e:?}",
                    reg.name
                )));
            }
        };
        if parsed.name != reg.name {
            cleanup(&staging);
            return Err(MutationError::Schema(format!(
                "add_templates[] '{}' ships a template.json that declares the name \
                 '{}'. The entry name is the directory and the declared name is what \
                 a reference resolves — two names is one of them being wrong",
                reg.name, parsed.name
            )));
        }

        if let Err(e) = std::fs::create_dir_all(&local) {
            cleanup(&staging);
            return Err(MutationError::Schema(format!(
                "the local template root could not be created: {e}"
            )));
        }
        let target = local.join(&reg.name);
        if target.exists() {
            // Not a registry hit (that is `template_name_taken` from
            // `refuse_if_taken`) but a directory no row names: the residue of an
            // aborted run, or something placed by hand. Refused by name rather
            // than overwritten — No-Delete.
            cleanup(&staging);
            return Err(MutationError::TemplateNameTaken(format!(
                "'{}' already lies in the local template root and no registry row \
                 names it. It is refused rather than overwritten (No-Delete); \
                 clear it by hand if it is residue",
                reg.name
            )));
        }
        if let Err(e) = std::fs::rename(&stage_dir, &target) {
            cleanup(&staging);
            return Err(MutationError::Schema(format!(
                "add_templates[] '{}' could not be moved into the library: {e}",
                reg.name
            )));
        }
        scanned.push(crate::templates::ScannedTemplate {
            filesystem_path: target,
            ..parsed
        });
    }
    cleanup(&staging);
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn entry(name: &str) -> meclaw_core::serde_json::Value {
        json!({"name": name, "files": {"template.json": "{}"}})
    }

    /// Every value here would have become a directory name under the colony's
    /// local template root.
    fn refusal_of(value: &meclaw_core::serde_json::Value) -> MutationError {
        match parse_entry(value) {
            Ok(ok) => panic!("not refused — the clamp leaks: {ok:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn a_name_that_could_become_a_path_is_refused_by_the_colony() {
        // The clamp is the substrate's, not the caller's:
        // `{templates_root}/local/<name>/` is BUILT here, never taken from a
        // field, so a name that could escape it is refused rather than
        // sanitised. Sanitising would silently register a template under a name
        // nobody asked for.
        for bad in [
            "../escape",
            "/etc/x",
            "with/slash",
            ".",
            "..",
            "",
            "Upper",
            "-lead",
            "x",
        ] {
            let err = refusal_of(&entry(bad));
            assert_eq!(
                err.error_code(),
                "invalid_template_name",
                "{bad} refused under the wrong code: {}",
                err.message(),
            );
            assert!(
                err.message().contains(bad) || bad.is_empty(),
                "the refusal must name the offending value: {}",
                err.message(),
            );
        }
    }

    #[test]
    fn a_well_formed_name_survives_with_its_files_verbatim() {
        let reg = parse_entry(&json!({
            "name": "note-unit",
            "files": {"template.json": "{\"name\":\"note-unit\"}", "config.json": "{}"}
        }))
        .expect("well formed");
        assert_eq!(reg.name, "note-unit");
        assert_eq!(reg.files.len(), 2);
        assert_eq!(reg.files["template.json"], "{\"name\":\"note-unit\"}");
    }

    #[test]
    fn a_file_path_that_climbs_out_is_refused_too() {
        // The name is not the only way out of the directory.
        for bad in ["../x.json", "a/../../x", "/abs.json"] {
            let err = refusal_of(&json!({"name": "note-unit", "files": {bad: "{}"}}));
            assert_eq!(
                err.error_code(),
                "invalid_template_name",
                "{}",
                err.message()
            );
        }
    }

    #[test]
    fn a_registration_without_a_template_json_is_refused() {
        // Without it `scan_templates_dir` would not register the directory at
        // all: the write would succeed, the registry row would be a lie, and
        // the next rescan would quietly drop it.
        let err = refusal_of(&json!({"name": "note-unit", "files": {"config.json": "{}"}}));
        assert_eq!(err.error_code(), "schema", "{}", err.message());
        assert!(err.message().contains("template.json"), "{}", err.message());
    }

    /// The registry, not the filesystem, decides whether a name is taken —
    /// the check needs the snapshot and therefore cannot live in `parse_entry`.
    #[test]
    fn a_name_the_registry_already_answers_is_refused() {
        let reg = parse_entry(&entry("note-unit")).expect("well formed");
        let taken = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "note-unit".into(),
                version: Some("1.0.0".into()),
                filesystem_path: std::path::PathBuf::from("/t/note-unit"),
            },
        ]);
        let err = refuse_if_taken(&reg, &taken).expect_err("the name is taken");
        assert_eq!(err.error_code(), "template_name_taken", "{}", err.message());
        assert!(err.message().contains("note-unit"), "{}", err.message());

        let empty = crate::templates::TemplatesRegistry::default();
        refuse_if_taken(&reg, &empty).expect("a free name is not a refusal");
    }
}
