//! GH #294 — an `override_params` key names a param the addressed cell has.
//!
//! GH #140 gave `override_params` its cell half: on a subtree template the keys
//! are the cells' paths, and a key that names no cell is refused
//! pre-destructively instead of committing as a silent no-op. The param half
//! was never built. A typo one level down — `{"a": {"externl_timeout_ms": 5}}`
//! — commits, the cell spawns with its default, and nothing anywhere says a
//! word. That is R10's original complaint, one nesting level deeper.
//!
//! Ruling Q6 (2026-08-21) settles what "exists" means: an **existence check**.
//! The addressed cell's template `config.json` either carries the key under
//! `params` or it does not; types and `because` may arrive later as
//! declarations. The key set does not depend on the values, so instance
//! substitution is irrelevant and the check reads the template's RAW `params`
//! object. A cell with no `params` block at all has the empty set — an override
//! addressed at it is refused naming that empty list, rather than being
//! swallowed by the `if let Some(params)` the staging merge is written around.
//!
//! Both forms are checked through the same call: the ADDRESSED form of a
//! subtree template (`{"a": {…}}`) and the FLAT form of a single-cell template
//! (`{…}`, merged into that one cell's params by
//! `stage::patch_and_substitute_config`). The check lives in validation for
//! both, so the two cannot drift apart.

use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "persist_mock".to_string(),
        Arc::new(PersistCellFactory {
            spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }) as Arc<dyn CellFactory>,
    )]
}

/// Subtree template `unit`: a root hive that declares only its graph, a cell
/// `a` with exactly one param, and a cell `bare` with no `params` block at all.
fn write_templates(root: &std::path::Path) {
    let unit = root.join("templates").join("unit");
    write(&unit, "template.json", r#"{"name":"unit"}"#);
    write(
        &unit,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./bare"}]}}}"#,
    );
    write(
        &unit,
        "a/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &unit,
        "bare/config.json",
        r#"{"cell":{"type":"persist_mock"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // Single-cell template: the FLAT form's subject.
    let solo = root.join("templates").join("solo");
    write(&solo, "template.json", r#"{"name":"solo"}"#);
    write(
        &solo,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// The params of an instantiated cell, read off the disk the mutation wrote.
fn instance_params(td: &std::path::Path, rel: &str) -> JsonValue {
    let raw = std::fs::read_to_string(td.join(rel).join("config.json"))
        .unwrap_or_else(|e| panic!("read {rel}/config.json: {e}"));
    let v: JsonValue = meclaw_core::serde_json::from_str(&raw).expect("config json");
    v["params"].clone()
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_templates(td.path());
    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan_templates(&h, td.path().join("templates")).await;
    h
}

/// The rejection details of a mutation that must be refused with `schema`.
fn schema_details(outcome: &MutationOutcome) -> String {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "schema", "{outcome:?}");
            details.clone()
        }
        other => panic!("expected a schema rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_override_naming_no_param_of_the_cell_is_refused_and_names_both() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"a":{"q":2}}}
        ]}}),
    )
    .await;
    let details = schema_details(&outcome);
    assert!(
        details.contains("'q'"),
        "the refusal names the param that does not exist: {details}"
    );
    assert!(
        details.contains("'a'"),
        "and the cell it was addressed at: {details}"
    );
    assert!(
        details.contains("Its params are:") && details.contains("'p'"),
        "and lists the params that DO exist: {details}"
    );
    assert!(
        !td.path().join("main/u1").exists(),
        "and nothing is materialised"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_override_naming_an_existing_param_still_commits() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"a":{"p":42}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a param the cell has is still settable; got {outcome:?}"
    );
    assert_eq!(
        instance_params(td.path(), "main/u1/a")["p"],
        42,
        "and it arrives in the instance"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_cell_addressed_with_a_key_it_does_not_read_is_refused() {
    // The case `gh212_documented_override_params` compensated for in prose: the
    // seals turned cells into hives, a hive path is a perfectly valid cell path,
    // and `{"collector": {"memory_tier": "1"}}` therefore committed and
    // configured nothing. It is a substrate refusal now.
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"":{"memory_tier":"1"}}}
        ]}}),
    )
    .await;
    let details = schema_details(&outcome);
    assert!(
        details.contains("'memory_tier'"),
        "the refusal names the key the hive does not read: {details}"
    );
    assert!(
        details.contains("hive"),
        "and says what kind of cell it addressed: {details}"
    );
    assert!(
        details.contains("Its params are:") && details.contains("'graph'"),
        "and lists what the hive DOES read: {details}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_without_a_params_block_refuses_the_override_and_says_so() {
    // The trap the staging merge is built around: `patch_and_substitute_config`
    // merges `override_params` only `if let Some(params) = cfg.get_mut("params")`
    // — a cell with no params block swallowed the whole override without a
    // sound. Absent params is the EMPTY set, and the empty set is said out loud.
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"bare":{"p":1}}}
        ]}}),
    )
    .await;
    let details = schema_details(&outcome);
    assert!(
        details.contains("'p'") && details.contains("'bare'"),
        "the refusal names the param and the cell: {details}"
    );
    assert!(
        details.contains("Its params are: none"),
        "and says the cell has no params at all: {details}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_flat_form_of_a_single_cell_template_is_checked_the_same_way() {
    // `stage::patch_and_substitute_config` merges the flat form into the one
    // cell's params. The check for it lives in validation next to the addressed
    // form's, so a param typo is refused identically on both roads.
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let bad = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"c1","template":"solo","override_params":{"q":2}}
        ]}}),
    )
    .await;
    let details = schema_details(&bad);
    assert!(
        details.contains("'q'") && details.contains("Its params are:") && details.contains("'p'"),
        "the flat form names the param and lists the real ones: {details}"
    );

    let good = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"c2","template":"solo","override_params":{"p":7}}
        ]}}),
    )
    .await;
    assert!(
        matches!(good, MutationOutcome::Committed { .. }),
        "an existing param stays settable in the flat form; got {good:?}"
    );
    assert_eq!(instance_params(td.path(), "main/c2")["p"], 7);

    h.shutdown().await;
}

// ─────────────────────────────────── GH #294 acceptance 2: no migration pass

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The shipped `templates/` directory as a registry snapshot — what a real
/// mutation resolves a `cell.type: "ref"` sub-unit against (GH #277). Without
/// it `talky` and `cogny` would not parse at all.
fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
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

/// GH #294's second acceptance criterion: the check costs no migration pass.
///
/// Every shipped template is instantiated with an EMPTY override set (nothing
/// may be refused where nothing is addressed) and then, cell by cell, with an
/// override that names exactly the params that cell declares — the strongest
/// legitimate override there is. A shipped template that could no longer be
/// parameterised in its own params would be the migration this ruling ruled
/// out.
#[test]
fn every_shipped_template_instantiates_unchanged() {
    let registry = shipped_registry();
    let templates = core_root().join("templates");
    let mut seen = 0usize;
    for entry in std::fs::read_dir(&templates).expect("templates dir") {
        let dir = entry.expect("dir entry").path();
        if !dir.join("template.json").is_file() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let parsed = parse_subtree(&dir, &registry)
            .unwrap_or_else(|e| panic!("shipped template '{name}' does not parse: {e:?}"));
        seen += 1;
        for cell in &parsed.cells {
            // Nothing addressed → nothing refused.
            check_override_params(cell, Some(&cell.rel_path), &name, &json!({}))
                .unwrap_or_else(|e| panic!("'{name}' rejects an empty override: {e:?}"));

            // Every param the cell declares, set to its own value.
            let own = cell
                .config
                .get("params")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            check_override_params(cell, Some(&cell.rel_path), &name, &JsonValue::Object(own))
                .unwrap_or_else(|e| {
                    panic!(
                        "'{name}' cell '{}' cannot be overridden in its own params: {e:?}",
                        cell.rel_path
                    )
                });
        }
    }
    assert!(
        seen >= 20,
        "the sweep found almost no shipped template: {seen}"
    );
}
