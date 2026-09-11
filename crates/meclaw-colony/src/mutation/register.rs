//! GH #440 — `add_templates`: a reusable template enters a RUNNING colony.
//!
//! The operation touches no cell, no edge and no path in the tree. That is why
//! it lives here instead of in `stage.rs`: everything there is about putting a
//! cell somewhere, and this puts a CLASS in the library.
//!
//! Two invariants carry the whole module:
//!
//! 1. **The target path is built, never taken.**
//!    `{templates_root}/local/<name>@<version>/` is composed from the resolved
//!    `--templates` root, the clamped name and the version the entry's own
//!    `template.json` declares; `{templates_root}/local/<name>/` when it
//!    declares none. No field of the body becomes a path segment, so there is
//!    nothing to escape from and nothing to sanitise — `@` cannot arrive from
//!    the body either, because `name_is_well_formed` forbids it and the version
//!    is the parsed one. Siblings, not `local/<name>/<version>/` as a child
//!    (GH #664): the scan stops descending at a `template.json`, so under the
//!    child form a `local/<name>/` that is already live would hide every
//!    version beneath it.
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
    /// The template's own name, `^[a-z][a-z0-9-]{1,63}$`. Together with the
    /// version it is also the directory name under `{templates_root}/local/`,
    /// which is why the pattern forbids `/`, `.` and `..` rather than
    /// filtering them later.
    pub name: String,
    /// The `version` the shipped `template.json` declares, `None` when it
    /// declares none. Always a version `resolve` can read — [`parse_entry`]
    /// refuses any other. The library is keyed by `(name, version)` since
    /// GH #664, so this is half of the entry's identity, not decoration.
    pub version: Option<String>,
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
    if !files.contains_key("config.json") {
        // GH #509. `template.json` says a directory IS a template; `config.json`
        // at the root says what an instance of it is — the cell for a
        // single-cell class, the hive with its `params.graph` for a composite.
        // Every template in the tree carries one, and a class without one
        // registers a directory nothing can be an instance OF. Refused HERE,
        // at the entry's own position, because the alternative is what was
        // measured: the class stages without complaint and the manifest is
        // refused one phase later with `template_missing` against the node,
        // naming a class the same diff had just registered — true about
        // nothing the author did wrong, and repairable from nothing it says.
        return Err(MutationError::Schema(format!(
            "add_templates[] '{name}' carries no config.json at its root. A              template.json says the directory is a template; the root              config.json says what an instance of it IS — one cell, or the              hive whose params.graph wires the rest. Files in subdirectories              are the cells INSIDE that hive and cannot stand in for it"
        )));
    }
    // The version is read from the entry's OWN `template.json` — the bytes
    // `stage_registrations` hands to the scanner's parser one step later, so a
    // registration and a rescan cannot disagree about which version arrived.
    // A `template.json` that is not JSON at all is refused there, by that
    // parser, with the message it writes; here it simply carries no version.
    let version = files
        .get("template.json")
        .and_then(|raw| meclaw_core::serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| {
            v.get("version")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
    // A version the resolver cannot read is a version nobody can reach. It used
    // to register into `local/<name>/` with the unreadable string in its
    // registry row, and after that NO form answered: `@<that string>` is not a
    // reference `resolve` parses, and the bare name finds neither a parsable
    // versioned candidate nor an unversioned one. An unreachable registration
    // is worse than a refusal, so it is refused here, before anything is
    // written (ruling of 2026-09-11).
    if let Some(v) = version.as_deref()
        && let Err(e) = crate::templates::parse_simple_version(v)
    {
        return Err(MutationError::Schema(format!(
            "add_templates[] '{name}' ships a template.json declaring the \
             version '{v}', which is not a version a reference can name: \
             {e}. It would register into the library and answer to nothing"
        )));
    }
    Ok(TemplateRegistration {
        name: name.to_string(),
        version,
        files,
    })
}

/// Refuse a `name@version` the registry already answers to — at this entry's
/// position.
///
/// Separate from [`parse_entry`] because it needs the registry snapshot, which
/// a later manifest entry sees differently from an earlier one.
///
/// Since GH #664 the library is keyed by `(name, version)`, so a NEW version of
/// a registered class is taken rather than refused. Two refusals remain, both
/// under `template_name_taken` — a narrower refusal under one code is additive,
/// a second code would be a second answer to one question:
///
/// 1. the identical `name@version` is already registered;
/// 2. an entry without a version arrives while versioned entries of that name
///    exist, or the mirror case. A version `resolve` cannot read never reaches
///    here — [`parse_entry`] refuses it before this.
pub fn refuse_if_taken(
    reg: &TemplateRegistration,
    templates: &crate::templates::TemplatesRegistry,
) -> Result<(), MutationError> {
    let existing: Vec<&crate::templates::TemplateEntry> = templates
        .entries_iter()
        .filter(|e| e.name == reg.name)
        .collect();
    if existing.is_empty() {
        return Ok(());
    }
    let known: Vec<&str> = existing
        .iter()
        .map(|e| e.version.as_deref().unwrap_or("(no version)"))
        .collect();
    match reg.version.as_deref() {
        Some(v) => {
            if templates.resolve(&format!("{}@{v}", reg.name)).is_ok() {
                return Err(MutationError::TemplateNameTaken(format!(
                    "'{}@{v}' is already registered. A new version registers \
                     beside it; the same one would be two answers to one \
                     reference",
                    reg.name
                )));
            }
            if existing.iter().any(|e| e.version.is_none()) {
                return Err(MutationError::TemplateNameTaken(format!(
                    "'{}' is registered without a version, so a second entry of \
                     that class cannot name its own. An unversioned entry beside \
                     a versioned one is a reference nobody can pin",
                    reg.name
                )));
            }
            Ok(())
        }
        None => {
            if existing.iter().any(|e| e.version.is_some()) {
                return Err(MutationError::TemplateNameTaken(format!(
                    "'{}' is registered with versions ({}), so a second entry of \
                     that class names its own. An unversioned entry beside a \
                     versioned one is a reference nobody can pin",
                    reg.name,
                    known.join(", ")
                )));
            }
            Err(MutationError::TemplateNameTaken(format!(
                "'{}' is already registered. A new version registers beside it; \
                 the same one would be two answers to one reference",
                reg.name
            )))
        }
    }
}

/// Every registration of one mutation, written but not yet visible.
///
/// The declaration lives here for the whole mutation and enters the library
/// with one `rename(2)` per entry immediately before the commit flush
/// ([`StagedRegistrations::commit`]). That ordering is the whole point of GH
/// #443: `add_templates` has to run FIRST, because a later entry of the same
/// diff must be able to resolve what it declared — but until the commit stands,
/// nothing about it may be visible in the library, or every refusal below it
/// leaves a directory `colony.db` has no row for.
///
/// The `Drop` impl removes the mutation's own staging area on every path out of
/// `handle_mutation`, committed or refused. It touches nothing but the bytes
/// this mutation wrote itself — no compensating delete under `local/`, which is
/// exactly the second write path the issue rejected.
pub struct StagedRegistrations {
    /// `{root}/.staging-templates/{mutation_id}` — owned by this mutation alone.
    staging: std::path::PathBuf,
    /// Per entry: where it lies now, where it belongs, and the parsed row.
    pending: Vec<(
        std::path::PathBuf,
        std::path::PathBuf,
        crate::templates::ScannedTemplate,
    )>,
}

impl std::fmt::Debug for StagedRegistrations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedRegistrations")
            .field("staging", &self.staging)
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl Drop for StagedRegistrations {
    fn drop(&mut self) {
        // Best-effort by construction: a mutation that is already returning
        // must not be held up by a failed cleanup, and what stays behind is
        // under `.staging-templates`, which no reader of the library walks.
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

impl StagedRegistrations {
    /// The registrations as a later entry of the SAME diff resolves them:
    /// `filesystem_path` points into the staging area, because that is where
    /// the bytes are until the commit. They are the same bytes the library
    /// directory will hold — the move is a `rename(2)`, not a rewrite.
    pub fn snapshot(&self) -> Vec<crate::templates::ScannedTemplate> {
        self.pending
            .iter()
            .map(|(stage_dir, _, parsed)| crate::templates::ScannedTemplate {
                filesystem_path: stage_dir.clone(),
                ..parsed.clone()
            })
            .collect()
    }

    /// Move every staged registration into the library and return the rows as
    /// they must be persisted — `filesystem_path` on the final directory.
    ///
    /// Called immediately before the commit flush. If a `rename(2)` fails part
    /// way through the list, the entries that already moved are renamed BACK
    /// into their own staging directories: the mutation only ever moves bytes
    /// it wrote itself, so there is no compensating delete and nothing that
    /// could touch a pre-existing template.
    pub fn commit(&mut self) -> Result<Vec<crate::templates::ScannedTemplate>, MutationError> {
        let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
        let mut out = Vec::with_capacity(self.pending.len());
        for (stage_dir, target, parsed) in &self.pending {
            if let Err(e) = std::fs::rename(stage_dir, target) {
                for (back_stage, back_target) in moved.iter().rev() {
                    if let Err(undo) = std::fs::rename(back_target, back_stage) {
                        tracing::error!(
                            target = %back_target.display(),
                            error = %undo,
                            "a registered template could not be taken back out of the library — \
                             the mutation is refused and the directory stays"
                        );
                    }
                }
                return Err(MutationError::Schema(format!(
                    "add_templates[] '{}' could not be moved into the library: {e}",
                    parsed.name
                )));
            }
            moved.push((stage_dir.clone(), target.clone()));
            out.push(crate::templates::ScannedTemplate {
                filesystem_path: target.clone(),
                ..parsed.clone()
            });
        }
        self.pending.clear();
        Ok(out)
    }
}

/// Write every registration into the mutation's own staging area.
///
/// Everything that can be refused about a declaration is refused HERE — a
/// template.json the scanner rejects, a declared name that disagrees with the
/// entry name, a target the library already holds. What is deliberately NOT
/// here is the `rename(2)` into the library: it belongs to
/// [`StagedRegistrations::commit`], so that a refusal anywhere below in the
/// same mutation leaves the library exactly as it found it.
///
/// Staging plus one `rename(2)` per entry, never a recursive copy into the live
/// library: a half-written `template.json` is exactly what a concurrent rescan
/// would pick up.
pub fn stage_registrations(
    regs: &[TemplateRegistration],
    templates_root: &std::path::Path,
    root: &std::path::Path,
    mutation_id: &str,
) -> Result<StagedRegistrations, MutationError> {
    let staging = root.join(".staging-templates").join(mutation_id);
    let local = templates_root.join("local");
    let mut staged = StagedRegistrations {
        staging: staging.clone(),
        pending: Vec::with_capacity(regs.len()),
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
                return Err(MutationError::Schema(format!(
                    "add_templates[] '{}' carries a template.json the scanner refuses: {e:?}",
                    reg.name
                )));
            }
        };
        if parsed.name != reg.name {
            return Err(MutationError::Schema(format!(
                "add_templates[] '{}' ships a template.json that declares the name \
                 '{}'. The entry name is the directory and the declared name is what \
                 a reference resolves — two names is one of them being wrong",
                reg.name, parsed.name
            )));
        }

        if let Err(e) = std::fs::create_dir_all(&local) {
            return Err(MutationError::Schema(format!(
                "the local template root could not be created: {e}"
            )));
        }
        // GH #664: one rule, not two — a versioned entry always builds a
        // versioned directory, the first registration of a new name included.
        let target = local.join(match reg.version.as_deref() {
            Some(v) => format!("{}@{v}", reg.name),
            None => reg.name.clone(),
        });
        if target.exists() {
            // Not a registry hit (that is `template_name_taken` from
            // `refuse_if_taken`) but a directory no row names: the residue of an
            // aborted run, or something placed by hand. Refused by name rather
            // than overwritten — No-Delete.
            return Err(MutationError::TemplateNameTaken(format!(
                "'{}' already lies in the local template root and no registry row \
                 names it. It is refused rather than overwritten (No-Delete); \
                 clear it by hand if it is residue",
                target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| reg.name.clone())
            )));
        }
        // Fix round 1 to GH #664: `refuse_if_taken` runs against the registry
        // as it stood BEFORE the diff, and the target check below compares
        // PATHS — which differ since a versioned entry builds a versioned
        // directory. So the refusal of an unpinnable entry beside versioned
        // ones had to be repeated here, against what this diff has staged
        // itself, or it held only for diffs with one entry.
        if staged
            .pending
            .iter()
            .any(|(_, _, p)| p.name == reg.name && p.version.is_none() != reg.version.is_none())
        {
            return Err(MutationError::TemplateNameTaken(format!(
                "add_templates[] declares '{}' twice in one diff, once with a \
                 version and once without. An unversioned entry beside a \
                 versioned one is a reference nobody can pin, whether the other \
                 entry is already registered or arrives in the same diff",
                reg.name
            )));
        }
        // GH #443: an earlier entry of the SAME diff already claims this target.
        // `refuse_if_taken` cannot see it (nothing is registered yet) and the
        // library cannot either (nothing has moved yet), so the claim is checked
        // against what this mutation itself has staged. Without it the collision
        // would only surface as a failed `rename(2)` at commit time.
        if staged.pending.iter().any(|(_, t, _)| *t == target) {
            return Err(MutationError::TemplateNameTaken(format!(
                "add_templates[] declares '{}' twice in one diff. Both entries \
                 would become the same directory under the local template root, \
                 so one of them is a name nobody could resolve",
                target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| reg.name.clone())
            )));
        }
        staged.pending.push((stage_dir, target, parsed));
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn entry(name: &str) -> meclaw_core::serde_json::Value {
        // A root `config.json` beside it, because a class without one is not
        // instantiable and is refused (GH #509) — these cases are about the
        // NAME, so they must get past that guard.
        json!({"name": name, "files": {"template.json": "{}", "config.json": "{}"}})
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

    #[test]
    fn a_registration_without_a_root_config_json_is_refused() {
        // GH #509. Every instantiable template in the tree carries a
        // `config.json` at its root — the cell for a single-cell class, the
        // hive with its `params.graph` for a composite. Without one the scan
        // registers a directory nothing can be an instance OF, and the refusal
        // arrives one phase later as `template_missing` against the NODE that
        // named the class the same manifest had just registered. Measured: a
        // two-cell class written as `fetch/config.json` + `parse/config.json`,
        // no root, staged without complaint, and "the template you named does
        // not exist" back about a name in the caller's own diff.
        let err = refusal_of(&json!({"name": "note-unit", "files": {
            "template.json": "{}",
            "fetch/config.json": "{}",
            "parse/config.json": "{}"
        }}));
        assert_eq!(err.error_code(), "schema", "{}", err.message());
        assert!(err.message().contains("config.json"), "{}", err.message());
        assert!(err.message().contains("note-unit"), "{}", err.message());
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
