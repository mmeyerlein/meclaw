//! GH #1326 — the offline half of the mutation door: stage a manifest and
//! report what the door would refuse at birth, without a colony.
//!
//! `meclaw --validate --apply <manifest>` used to check the filesystem tree
//! only; a cell a manifest grows from a template met
//! `CellFactory::validate_params` for the first time at the live door. The
//! case that found it: an `llm` cell with provider `anthropic` and
//! `override_params.length_continuations: 1` passed `--validate` and was then
//! refused as `anthropic_unsupported_param`.
//!
//! **One source of truth.** Nothing here knows a rule. Each entry goes through
//! the door's own substitution ([`substitute_mutation_diff`]) and the door's
//! own staging body ([`build_staging_tree_into`], which calls
//! `validate_params` and `validate_declared_params` for every staged cell,
//! subtree cells included). What the door refuses there, this refuses, with
//! the door's `error_code` and wording.
//!
//! **Nothing in the root.** Staging goes into a scratch directory under the
//! system temp directory, which is removed again afterwards; the colony root
//! is only read (final paths, existing nodes). No registry, no `colony.db`
//! write, no spawn.
//!
//! **Not replayed:** the door's pre-staging validation of the post-state
//! (edges, header contracts, ports), the `replace_nodes` lift staging, and an
//! entry's effect on the next one (a node or template an earlier entry adds).
//! A template that an `add_templates` of an earlier entry -- or of the same
//! entry, which the door registers before it stages that entry's growth --
//! provides by exactly that name is reported as not checked; any other missing
//! template stays a refusal.
//!
//! [`substitute_mutation_diff`]: crate::mutation::substitute::substitute_mutation_diff
//! [`build_staging_tree_into`]: crate::mutation::stage::build_staging_tree_into

use super::{ManifestBody, MutationError};
use meclaw_core::JsonValue;
use std::collections::HashMap;

/// What the dry run found for one manifest entry.
#[derive(Debug)]
pub enum DryRunFinding {
    /// The door would refuse this entry; `error` is its own refusal.
    Refused {
        /// 1-based position of the entry in the manifest (an operator counts
        /// entries, not indices -- the same convention as the manifest parser).
        position: usize,
        /// The refusal, exactly as the door's staging raised it.
        error: MutationError,
    },
    /// The entry could not be checked offline.
    NotChecked {
        /// 1-based position of the entry in the manifest.
        position: usize,
        /// Why not, for the operator.
        reason: String,
    },
}

/// Stage every entry of `manifest` the way the mutation door would and return
/// what it refuses.
///
/// `root` is the colony root (read only), `templates` the registry a growth
/// resolves against, `env` the same environment map the boot substitution of
/// `--validate` reads (`--env` or `<root>/.env`), `factories` the cell-type
/// registry whose param checks the door asks. An empty result means no entry
/// was refused at staging.
///
/// The `scope` is taken literally, as the door reads it (`handle_mutation`
/// takes `payload.scope` without substitution and `meclaw --apply` passes the
/// file through unchanged). A `${VAR}` in it therefore places the final path
/// the way the door would place it; the staged copy lands in the scratch
/// directory either way, so the param check runs regardless.
pub fn dry_run_manifest(
    root: &std::path::Path,
    manifest: &ManifestBody,
    templates: &crate::templates::TemplatesRegistry,
    env: &HashMap<String, String>,
    factories: &crate::CellFactoryRegistry,
) -> Vec<DryRunFinding> {
    let scratch = std::env::temp_dir().join(format!("meclaw-validate-{}", uuid::Uuid::now_v7()));
    let mut findings = Vec::new();
    // Template names an `add_templates` has declared so far (this entry's own
    // included, see below): only a missing template named here is "not
    // checked"; a typo in a name stays a refusal.
    let mut templates_declared: std::collections::HashSet<String> = Default::default();
    for (i, entry) in manifest.entries().iter().enumerate() {
        let position = i + 1;
        let diff_raw = entry.get("diff").cloned().unwrap_or(JsonValue::Null);
        // The door registers an entry's own `add_templates` before it stages
        // that entry's `add_nodes` (GH #443), so the entry's own names count.
        templates_declared.extend(declared_template_names(&diff_raw));
        let ctx: HashMap<String, String> = entry
            .get("ctx")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.into())))
                    .collect()
            })
            .unwrap_or_default();
        let diff = match crate::mutation::substitute::substitute_mutation_diff(&diff_raw, env, &ctx)
        {
            Ok(d) => d,
            Err(error) => {
                findings.push(DryRunFinding::Refused { position, error });
                continue;
            }
        };
        let scope = entry.get("scope").and_then(|v| v.as_str()).unwrap_or("/");
        // `.staging/<entry>` inside the scratch directory: the door's refusal
        // text cuts everything up to the id after `.staging`, so the operator
        // reads the same `<node>/config.json` the door would name.
        let staging = scratch.join(".staging").join(format!("entry-{position}"));
        if let Err(error) = crate::mutation::stage::build_staging_tree_into(
            &staging,
            root,
            scope,
            &diff,
            templates,
            env,
            &ctx,
            factories,
            &crate::watchdog::WorkPulse::silent(),
            &crate::mutation::twin::TwinFacts::default(),
        ) {
            match error {
                MutationError::TemplateMissing(name)
                    if templates_declared.contains(template_base_name(&name)) =>
                {
                    findings.push(DryRunFinding::NotChecked {
                        position,
                        reason: format!(
                            "template '{name}' is not in the library yet; an `add_templates` \
                             entry of this manifest provides it only when the colony runs it"
                        ),
                    });
                }
                error => findings.push(DryRunFinding::Refused { position, error }),
            }
        }
    }
    // Outside the colony root: No-Delete does not reach the scratch directory,
    // and leaving it would litter the temp directory with one copy per run.
    if let Err(e) = std::fs::remove_dir_all(&scratch)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %scratch.display(), error = %e, "dry-run scratch not removed");
    }
    findings
}

/// The `name`s an entry's `add_templates[]` declares (raw, before
/// substitution: a name must match `^[a-z][a-z0-9-]+$`, so it carries no
/// `${VAR}`).
fn declared_template_names(diff: &JsonValue) -> Vec<String> {
    diff.get("add_templates")
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A template reference without its `@version` pin.
fn template_base_name(tpl_ref: &str) -> &str {
    tpl_ref.split('@').next().unwrap_or(tpl_ref)
}
