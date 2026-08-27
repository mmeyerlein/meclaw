//! GH #292 — a missing required key is refused BEFORE staging.
//!
//! A template says what it needs (`requires.ctx` / `requires.env`, see
//! `templates/requires.rs`). Until now nobody read that declaration at
//! instantiation time: a mutation that forgot `ctx.model` was copied onto disk
//! first and only broke when the staging substitution walked the copied
//! `config.json` and found the token unresolvable — `ctx_key_missing`, raised
//! after the copy. A missing `env` key was not caught at all; the instance was
//! born and failed later, at run time.
//!
//! The declaration is a contract, so it is checked where a contract belongs:
//! before anything is written. `validate_requires` runs in `handle_mutation`
//! right after the lazy template check and before `validate_scope_containment`
//! — the first point at which both the resolved template and the mutation's
//! `ctx`/`env` are known, and still before the first byte of staging.
//!
//! Four claims:
//!   1. a missing `ctx` key is refused, and the refusal names the key AND the
//!      `because` the template gave — the declaration exists to be quoted back;
//!   2. the union includes what the template's `ref`s need: a requirement of a
//!      referenced template is a requirement of the composite;
//!   3. the refusal is pre-destructive — `<root>/.staging` stays empty and the
//!      registry gains no row (the "nothing left behind" half of GH #276);
//!   4. a template with no `requires` block instantiates exactly as before.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

// ── topology helpers (same shape as the gh276 suite) ─────────────────────────

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

fn hive(root: &std::path::Path, rel: &str, params: &str) {
    write(
        root,
        &format!("{rel}/config.json"),
        &format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    );
}

fn echo_config(emitted_target: &str, extra: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo"}},
            "params":{{"emitted_target":"{emitted_target}"{extra}}},
            "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
    )
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut paths: Vec<String> = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    paths.sort();
    paths
}

/// The names under `{root}/.staging` — absent directory reads as empty.
fn staging_entries(root: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join(".staging"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// Boot a one-hive colony with the echo factory, then register `templates/`.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    rescan_templates(&h, td.path().join("templates")).await;
    h
}

// ── the declarations under test ─────────────────────────────────────────────

/// Quoted back verbatim by the refusal — a `because` exists to be read by the
/// person who forgot the key.
const CTX_BECAUSE: &str = "the brain this cell infers with";
const ENV_BECAUSE: &str = "the key the referenced unit authenticates with";

/// A single-cell template that declares `ctx.model` and uses it, so that the
/// pre-#292 behaviour is observable: without the check the key surfaces as
/// `ctx_key_missing` while the staged copy is substituted — after the copy.
fn write_needs_model(root: &std::path::Path) {
    let dir = root.join("templates").join("needs_model");
    write(
        &dir,
        "template.json",
        &format!(
            r#"{{"name":"needs_model","version":"1.0.0",
                 "requires":{{"ctx":{{"model":{{"type":"string","required":true,
                   "because":"{CTX_BECAUSE}"}}}}}}}}"#
        ),
    );
    write(
        &dir,
        "config.json",
        &echo_config("/needs", r#","model":"${ctx.model}""#),
    );
}

/// A composite whose OWN `template.json` declares nothing and whose `ref`
/// target declares `env.SOME_KEY`. The requirement travels with the ref.
fn write_composite(root: &std::path::Path) {
    let inner = root.join("templates").join("inner_unit");
    write(
        &inner,
        "template.json",
        &format!(
            r#"{{"name":"inner_unit","version":"1.0.0",
                 "requires":{{"env":{{"SOME_KEY":{{"because":"{ENV_BECAUSE}"}}}}}}}}"#
        ),
    );
    write(&inner, "config.json", &echo_config("/inner", ""));

    let outer = root.join("templates").join("outer_hive");
    write(
        &outer,
        "template.json",
        r#"{"name":"outer_hive","version":"1.0.0"}"#,
    );
    write(
        &outer,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        &outer,
        "unit/config.json",
        r#"{"cell":{"type":"ref","template":"inner_unit@1.0.0"}}"#,
    );
}

/// The control: no `requires` block at all.
fn write_plain(root: &std::path::Path) {
    let dir = root.join("templates").join("plain");
    write(
        &dir,
        "template.json",
        r#"{"name":"plain","version":"1.0.0"}"#,
    );
    write(&dir, "config.json", &echo_config("/plain", ""));
}

fn rejected(outcome: &MutationOutcome) -> (&str, &str) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => (error_code.as_str(), details.as_str()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ── 1. the named template's own requirement ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_missing_a_required_ctx_key_is_refused_and_names_the_key_and_the_because() {
    let td = tempfile::TempDir::new().unwrap();
    write_needs_model(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "needs", "template": "needs_model@1.0.0"}]},
               "ctx": {}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "requirement_missing",
        "a declared key that is not supplied is its own refusal, got {outcome:?}"
    );
    assert!(
        details.contains("model"),
        "the refusal names the missing key, got {details}"
    );
    assert!(
        details.contains(CTX_BECAUSE),
        "the refusal quotes the template's own `because`, got {details}"
    );
    h.shutdown().await;
}

// ── 2. what the refs need is required too ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_requirement_of_a_referenced_template_is_required_too() {
    let td = tempfile::TempDir::new().unwrap();
    write_composite(td.path());
    let h = boot(&td).await;

    // No `.env` was written, so `SOME_KEY` is nowhere — and the outer template
    // itself declares nothing. Only the ref's declaration can refuse this.
    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "composite", "template": "outer_hive@1.0.0"}]}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "requirement_missing",
        "the union spans the refs, got {outcome:?}"
    );
    assert!(
        details.contains("SOME_KEY"),
        "the refusal names the missing env key, got {details}"
    );
    assert!(
        details.contains(ENV_BECAUSE),
        "the refusal quotes the referenced template's `because`, got {details}"
    );
    h.shutdown().await;
}

// ── 3. refused before anything is written ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_staged_when_a_requirement_is_missing() {
    let td = tempfile::TempDir::new().unwrap();
    write_needs_model(td.path());
    let h = boot(&td).await;

    let before = registry_paths(&h).await;
    let staging_before = staging_entries(td.path());

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "needs", "template": "needs_model@1.0.0"}]},
               "ctx": {}}),
    )
    .await;
    assert_eq!(rejected(&outcome).0, "requirement_missing", "{outcome:?}");

    assert!(
        staging_entries(td.path()).is_empty(),
        "a requirement is checked before the copy: nothing may reach {}/.staging",
        td.path().display()
    );
    assert_eq!(
        staging_before,
        staging_entries(td.path()),
        "the staging tree is untouched by a refused mutation"
    );
    let after = registry_paths(&h).await;
    assert_eq!(
        before, after,
        "a refused mutation registers nothing: {before:?} vs {after:?}"
    );
    assert!(
        !td.path().join("main/needs").exists(),
        "no directory is renamed in for a refused mutation"
    );
    h.shutdown().await;
}

// ── 4. the control: no declaration, no change ───────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_with_no_requires_block_instantiates_exactly_as_before() {
    let td = tempfile::TempDir::new().unwrap();
    write_plain(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "plain", "template": "plain@1.0.0"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a template that declares nothing requires nothing, got {outcome:?}"
    );
    assert!(
        registry_paths(&h).await.iter().any(|p| p == "/plain"),
        "the instance is there"
    );
    h.shutdown().await;
}
