//! GH #516 — a composite hive must be able to give its conversation surface and
//! its reasoning core two different models.
//!
//! `talky` and `cogny` both carry `"model": "${ctx.model}"` in their brain cell,
//! and both are right to: standalone, each is THE agent of its instantiation and
//! `model` is the one model it infers with. A LEVEL that references both — one
//! as the conversation surface, one as the reasoning core — is instantiated with
//! a single flat `ctx`, so the two brains resolve the same key and get the same
//! model. The surface then runs the reasoning model and every turn pays core
//! latency for the half of the topology whose whole job is to answer fast; the
//! split the two-brain design exists for is gone, silently, with nothing in the
//! tree saying so.
//!
//! The addressing that fixes it already existed: a `cell.type: "ref"` marker
//! accepts `override_params` keyed by the referenced template's cell paths
//! (GH #140, re-addressed to the outer template's rel-paths by GH #277). What
//! did NOT exist is the substitution. `SubtreeTemplate::ref_overrides` is read
//! off a template TREE, and nothing gave it the instance-class pass every other
//! value read off a template tree gets in `patch_and_substitute_config`. So a
//! ref marker could hand a LITERAL down into the template it names, but not the
//! outer instantiation's own ctx: `${ctx.X}` survived into the env pass and was
//! refused there as an environment variable named `ctx.X`.
//!
//! Three assertions, and the file states all three because any one alone is a
//! false green:
//!
//! - `a_ref_marker_passes_the_outer_ctx_into_the_template_it_names` is the
//!   SUBSTRATE half, on templates this file writes itself. It is the mechanism,
//!   and it holds for every composite, not only for the one that found the bug.
//! - `the_surface_and_the_core_take_different_models` is the SHIPPED half, on
//!   the real `assistant` tree through the real registry. A mechanism nothing
//!   uses is a mechanism nobody notices breaking.
//! - `the_level_declares_the_surface_model_as_a_requirement` pins the key
//!   STRICT. A defaulted surface model is the same failure mode one layer down:
//!   an instantiation that forgets the key would be accepted and would run one
//!   model where it meant two, which is exactly how this got shipped.
//!
//! Nothing here touches `talky` or `cogny`: both stand standalone elsewhere and
//! keep `${ctx.model}`. The level that composes them owns the distinction, which
//! is where a level's own parameter belongs.

use meclaw_colony::mutation::subtree::{StagedSubtree, SubtreeOverrides, stage_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// A registry snapshot of the templates under `dir` — built by the scanner a
/// booted colony uses, because a `cell.type: "ref"` resolves against exactly
/// that and a hand-rolled snapshot would be free to disagree with it.
fn registry_of(dir: &Path) -> TemplatesRegistry {
    let scanned = scan_templates_dir(dir).unwrap_or_else(|e| panic!("scan {}: {e}", dir.display()));
    assert!(!scanned.is_empty(), "{} scanned to nothing", dir.display());
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Stage `template_root` under the logical name `name` into a throwaway root,
/// through the real staging path.
fn stage(
    template_root: &Path,
    name: &str,
    ctx: &[(&str, &str)],
    registry: &TemplatesRegistry,
) -> (tempfile::TempDir, StagedSubtree) {
    let root = tempfile::tempdir().expect("tempdir");
    // Both shipped brains carry a defaultless `${OPENROUTER_API_KEY}` and the
    // RUNTIME view of a staged config has to resolve, so the env map cannot be
    // empty. The value never reaches disk (GH #20).
    let env: HashMap<String, String> = [(
        "OPENROUTER_API_KEY".to_string(),
        "test-placeholder".to_string(),
    )]
    .into_iter()
    .collect();
    let ctx: HashMap<String, String> = ctx
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let staged = stage_subtree(
        root.path(),
        "m-gh516",
        "/main",
        name,
        template_root,
        &env,
        &ctx,
        None,
        &SubtreeOverrides::default(),
        registry,
        &Default::default(),
        &meclaw_colony::WorkPulse::silent(),
        meclaw_colony::mutation::Birth::Active,
    )
    .unwrap_or_else(|e| panic!("stage {name}: {e:?}"));
    (root, staged)
}

/// One staged cell's `params` — the DISK view, which is what a boot re-reads.
fn staged_params(staged: &StagedSubtree, absolute_path: &str) -> Value {
    let cell = staged
        .cells
        .iter()
        .find(|c| c.absolute_path.as_str() == absolute_path)
        .unwrap_or_else(|| {
            let known: Vec<&str> = staged
                .cells
                .iter()
                .map(|c| c.absolute_path.as_str())
                .collect();
            panic!("no staged cell at {absolute_path:?}; staged: {known:?}")
        });
    cell.params.clone()
}

/// The SUBSTRATE half: a ref marker's `override_params` is a value read off a
/// template tree, so it takes the instance-class pass like every other one.
#[test]
fn a_ref_marker_passes_the_outer_ctx_into_the_template_it_names() {
    let td = tempfile::tempdir().expect("tempdir");
    let templates = td.path().join("templates");

    // `unit` — the referenced template. It knows one key, `${ctx.model}`, and
    // that is the point: it is right standalone and must not be rewritten for
    // the sake of a composite that happens to use it twice.
    write(
        &templates,
        "unit/template.json",
        r#"{"name":"unit","version":"1.0.0"}"#,
    );
    write(
        &templates,
        "unit/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]},"ports":null}}"#,
    );
    write(
        &templates,
        "unit/brain/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"model":"${ctx.model}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // `level` — the composite. Two refs to the SAME template; one of them says
    // which key its brain reads instead.
    write(
        &templates,
        "level/template.json",
        r#"{"name":"level","version":"1.0.0"}"#,
    );
    write(
        &templates,
        "level/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]},"ports":null}}"#,
    );
    write(
        &templates,
        "level/surface/config.json",
        r#"{"cell":{"type":"ref","template":"unit@1.0.0"},"override_params":{"brain":{"model":"${ctx.model_surface}"}}}"#,
    );
    write(
        &templates,
        "level/core/config.json",
        r#"{"cell":{"type":"ref","template":"unit@1.0.0"}}"#,
    );

    let registry = registry_of(&templates);
    let (_root, staged) = stage(
        &templates.join("level"),
        "l1",
        &[
            ("model", "the-core-model"),
            ("model_surface", "the-surface-model"),
        ],
        &registry,
    );

    assert_eq!(
        staged_params(&staged, "/main/l1/surface/brain")["model"],
        "the-surface-model",
        "the ref marker named ctx.model_surface for the cell it references — a \
         ref that can pass only literals down cannot parameterise a sub-template \
         per instance, which is the whole purpose of override_params on a ref"
    );
    assert_eq!(
        staged_params(&staged, "/main/l1/core/brain")["model"],
        "the-core-model",
        "the ref that overrides nothing keeps the referenced template's own key"
    );
}

/// An environment token handed down by a ref marker still binds LATE (GH #20):
/// the instance pass added by GH #516 resolves the instance class and nothing
/// else, so a secret a composite passes into a sub-template is still never
/// materialised on the filesystem.
#[test]
fn a_ref_marker_still_hands_an_env_token_down_as_a_token() {
    let td = tempfile::tempdir().expect("tempdir");
    let templates = td.path().join("templates");

    write(
        &templates,
        "unit/template.json",
        r#"{"name":"unit","version":"1.0.0"}"#,
    );
    write(
        &templates,
        "unit/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]},"ports":null}}"#,
    );
    write(
        &templates,
        "unit/brain/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"model":"${ctx.model}","api_key":"x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &templates,
        "level/template.json",
        r#"{"name":"level","version":"1.0.0"}"#,
    );
    write(
        &templates,
        "level/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]},"ports":null}}"#,
    );
    write(
        &templates,
        "level/surface/config.json",
        r#"{"cell":{"type":"ref","template":"unit@1.0.0"},"override_params":{"brain":{"api_key":"${SOME_SECRET}"}}}"#,
    );

    let registry = registry_of(&templates);
    let root = tempfile::tempdir().expect("tempdir");
    let env: HashMap<String, String> =
        [("SOME_SECRET".to_string(), "sk-never-on-disk".to_string())]
            .into_iter()
            .collect();
    let ctx: HashMap<String, String> = [("model".to_string(), "m".to_string())]
        .into_iter()
        .collect();
    let staged = stage_subtree(
        root.path(),
        "m-gh516-env",
        "/main",
        "l1",
        &templates.join("level"),
        &env,
        &ctx,
        None,
        &SubtreeOverrides::default(),
        &registry,
        &Default::default(),
        &meclaw_colony::WorkPulse::silent(),
        meclaw_colony::mutation::Birth::Active,
    )
    .expect("stage level");

    let on_disk: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(
            staged
                .root_staging_path
                .join("surface")
                .join("brain")
                .join("config.json"),
        )
        .expect("read the staged config"),
    )
    .expect("parse the staged config");
    assert_eq!(
        on_disk["params"]["api_key"], "${SOME_SECRET}",
        "the env token a ref marker passed down stays a token on disk — the \
         instance pass resolves ctx and uuid7, never the environment class"
    );
}

/// The SHIPPED half: the level the defect was measured on.
#[test]
fn the_surface_and_the_core_take_different_models() {
    let registry = registry_of(&repo("templates"));
    let (_root, staged) = stage(
        &repo("templates/assistant"),
        "gen",
        &[
            ("model", "the-core-model"),
            ("model_surface", "the-surface-model"),
        ],
        &registry,
    );

    assert_eq!(
        staged_params(&staged, "/main/gen/surface/brain")["model"],
        "the-surface-model",
        "the conversation surface infers with the SURFACE model — this is the \
         whole defect: it used to resolve ctx.model and run the reasoning model"
    );
    assert_eq!(
        staged_params(&staged, "/main/gen/cogny/brain")["model"],
        "the-core-model",
        "the reasoning core keeps ctx.model"
    );
    // There was a third assertion here until `cogny@4.4.0`
    // ([#528](https://github.com/mmeyerlein/meclaw/issues/528)): the core's
    // lookup lane resolved `ctx.model_fast`. The lane is gone, so the key feeds
    // nothing and the level's ctx is two model keys, not three. The defect this
    // file measures is unchanged and so is its shape -- one flat ctx meeting two
    // refs that both read `model` -- it is simply two brains wide now instead of
    // three.
}

/// The key is STRICT, not defaulted. A silent fallback is the failure mode that
/// produced this issue: nothing looked wrong, the level simply ran one model
/// where it meant two, and no rejection said so.
#[test]
fn the_level_declares_the_surface_model_as_a_requirement() {
    let req = meclaw_colony::templates::read_requires(&repo("templates/assistant"))
        .expect("assistant declares a readable requires block");
    let decl = req
        .ctx
        .get("model_surface")
        .expect("assistant declares ctx.model_surface");
    assert!(
        decl.required,
        "a defaulted surface model would re-open exactly this bug: an \
         instantiation that forgets the key would be accepted and run one model"
    );
    assert!(
        decl.because.is_some(),
        "a declared key carries the reason it exists — it is quoted verbatim \
         when a mutation is refused"
    );
    // The two the level inherits through its refs stay where they are.
    assert!(
        !req.ctx.contains_key("model"),
        "ctx.model belongs to the referenced templates and is inherited through \
         the ref chain, never restated here (GH #292: restating IS the drift)"
    );
}
