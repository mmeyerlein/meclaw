//! GH #1268 -- a parked node never blocks the boot of its colony.
//!
//! After a `replace_nodes` lift the old life of a node stays parked as
//! `<name>~<version>`. Since GH #1084 the boot leaves the placeholders of a
//! parked node verbatim (it never runs, so no secret reaches it). The boot
//! still ran the factory's `validate_params` over those verbatim params, and a
//! param that must be an absolute path after substitution -- here
//! `params.sandbox.filesystem.read` of a `code` cell -- refused
//! `"${PROBE_DIR}"`: the whole colony did not start after a lift.
//!
//! What this file pins:
//! 1. grow a `code` cell whose sandbox reads `${PROBE_DIR}` (set in the
//!    colony env), lift it with `replace_nodes`, restart: the colony boots,
//!    the parked `~0.1.0` row is registered inactive and never spawned, its
//!    `config.json` still carries the placeholder verbatim, and the new life
//!    is active;
//! 2. the counter-pin: an ACTIVE node is still validated at boot -- a
//!    resolved value that is not absolute fails the boot with `InvalidParams`,
//!    and a missing variable fails it with `env_var_missing`.
//!
//! It lives in `meclaw-cells/tests/` because the claim needs the shipped
//! `code` factory, which `meclaw-colony` cannot depend on.

use meclaw_cells::TimerCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
    plan_bootstrap, set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// A `code` script that reads its input and emits nothing.
const SCRIPT: &str =
    "import sys, json; json.load(sys.stdin); sys.stdout.write(json.dumps({\"messages\": []}))";

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The `code` worker at one contract version, its sandbox reading the
/// directory the colony env names.
fn worker_config(version: &str, read: &str) -> String {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": SCRIPT,
            "external_timeout_ms": 10000,
            "sandbox": {
                "trust": "restricted",
                "network": "deny",
                "filesystem": {"runtime": true, "read": [read]}
            }
        },
        "contract": {"version": version, "settings": {}, "consumes": {}}
    })
    .to_string()
}

fn timer_config() -> String {
    json!({
        "cell": {"type": "timer", "timeout": -1},
        "params": {"query_timeout_ms": 5000},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
    .to_string()
}

/// The hive `box`: one door into the worker and one way back.
fn box_config() -> String {
    json!({
        "cell": {"type": "hive"},
        "params": {
            "contract": {"accepts": [{"route": "in_probe", "because": "probe the worker"}]},
            "graph": {"edges": [
                {"from": ".", "to": "./worker",
                 "condition": "has(hop.route) && hop.route == 'in_probe'"},
                {"from": "./worker", "to": "."}
            ]}
        }
    })
    .to_string()
}

/// `box@0.1.0` and `box@0.2.0`, differing only in the worker's version.
fn write_box_templates(root: &std::path::Path) {
    for version in ["0.1.0", "0.2.0"] {
        let dir = root.join(format!("templates/local/box@{version}"));
        write(
            &dir,
            "template.json",
            &format!(r#"{{"name":"box","version":"{version}"}}"#),
        );
        write(&dir, "config.json", &box_config());
        write(
            &dir,
            "worker/config.json",
            &worker_config(version, "${PROBE_DIR}"),
        );
    }
}

/// `/world` (a timer wired into `/probe`, so the hive is connected, GH #265),
/// the empty hive `/probe`, the env with an absolute `PROBE_DIR`.
fn tree() -> TempDir {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./world","to":"./probe"}
        ]}}}"#,
    );
    write(td.path(), "main/world/config.json", &timer_config());
    write(
        td.path(),
        "main/probe/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let data = td.path().join("probe-data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        td.path().join(".env"),
        format!("PROBE_DIR={}\n", data.display()),
    )
    .unwrap();
    write_box_templates(td.path());
    td
}

fn factory_list() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
    ]
}

fn factory_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factory_list() {
        r.insert(name, f);
    }
    r
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
    ack_rx.await.unwrap().expect("the rescan must not abort");
}

/// Boot a colony over `td`: rescan the library, spawn the sink, bootstrap.
async fn start(td: &TempDir) -> Result<ColonyHandle, String> {
    let h = ColonyHandle::new_with_factories_at(td, factory_list());
    rescan_templates(&h, td.path().join("templates")).await;
    let (capture_tx, _capture_rx) = mpsc::channel(64);
    h.spawn(Path::new("/probe/capture"), move || {
        CaptureCell::new(capture_tx.clone())
    })
    .await;
    match bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime()).await {
        Ok(_) => Ok(h),
        Err(e) => {
            h.shutdown().await;
            Err(format!("{e:?}"))
        }
    }
}

async fn submit_diff(h: &ColonyHandle, scope: &str, diff: JsonValue) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": scope, "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

fn committed(outcome: &MutationOutcome, why: &str) {
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{why} must commit: {outcome:?}"
    );
}

async fn box_rows(h: &ColonyHandle) -> Vec<RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: Some(Path::new("/probe/box/")),
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().entries
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_with_a_parked_placeholder_in_an_absolute_param_boots_after_a_lift() {
    set_term_timeout_ms_for_test(30_000);
    let td = tree();

    let h = start(&td).await.expect("the first boot");
    committed(
        &submit_diff(
            &h,
            "/probe",
            json!({
                "add_nodes": [{"name": "box", "template": "box@0.1.0"}],
                "add_edges": [{"from": "./box", "to": "./capture"}]
            }),
        )
        .await,
        "growing box@0.1.0",
    );
    committed(
        &submit_diff(
            &h,
            "/probe",
            json!({"replace_nodes": [{"match": {"name": "box"},
                                      "with": {"template": "box@0.2.0"}}]}),
        )
        .await,
        "lifting box to 0.2.0",
    );
    h.shutdown().await;

    let parked_config =
        std::fs::read_to_string(td.path().join("main/probe/box/worker~0.1.0/config.json"))
            .expect("the lift parks the old worker on disk (no-delete)");
    assert!(
        parked_config.contains("${PROBE_DIR}"),
        "the parked node carries the placeholder the fixture is about: {parked_config}"
    );

    let h = start(&td)
        .await
        .expect("a parked node with a placeholder in an absolute param must not block the boot");
    let rows = box_rows(&h).await;
    let row = |path: &str| {
        rows.iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("{path} registered: {rows:?}"))
            .clone()
    };
    let parked = row("/probe/box/worker~0.1.0");
    assert!(!parked.active, "the parked node stays inactive: {parked:?}");
    assert_eq!(
        parked.lifecycle_status, "NotYetSpawned",
        "the parked node is registered but never spawned: {parked:?}"
    );
    let live = row("/probe/box/worker");
    assert!(live.active, "the new life is active: {live:?}");
    h.shutdown().await;

    let after =
        std::fs::read_to_string(td.path().join("main/probe/box/worker~0.1.0/config.json")).unwrap();
    assert_eq!(
        after, parked_config,
        "the boot never rewrites the parked node's config.json"
    );
}

/// The counter-pin: the skip is for parked paths only. An active node with
/// the same param is validated against its resolved value as before.
#[test]
fn an_active_node_with_a_bad_absolute_param_still_fails_the_boot() {
    let td = tree();
    write(
        td.path(),
        "main/probe/rel/config.json",
        &worker_config("0.1.0", "${PROBE_REL}"),
    );
    std::fs::write(td.path().join(".env"), "PROBE_REL=relative/dir\n").unwrap();
    let errs = plan_bootstrap(td.path(), &factory_registry(), &Default::default())
        .expect_err("an active node whose resolved path is relative fails the boot");
    let rendered = format!("{errs:?}");
    assert!(
        rendered.contains("InvalidParams")
            && rendered.contains("relative/dir")
            && rendered.contains("must be an absolute path"),
        "the boot error names the param and the value: {rendered}"
    );

    write(
        td.path(),
        "main/probe/rel/config.json",
        &worker_config("0.1.0", "${PROBE_GONE}"),
    );
    let errs = plan_bootstrap(td.path(), &factory_registry(), &Default::default())
        .expect_err("an active node with a missing variable fails the boot");
    let rendered = format!("{errs:?}");
    assert!(
        rendered.contains("env_var_missing") && rendered.contains("PROBE_GONE"),
        "the boot error names the missing variable: {rendered}"
    );
}

/// The same tree on the plan side: a parked node with a placeholder in an
/// absolute param plans, inactive, with its params verbatim.
#[test]
fn a_parked_node_with_a_placeholder_in_an_absolute_param_plans_inactive() {
    let td = tree();
    write(
        td.path(),
        "main/probe/worker~0.1.0/config.json",
        &worker_config("0.1.0", "${PROBE_DIR}"),
    );
    std::fs::write(td.path().join(".env"), "").unwrap();
    let plan = plan_bootstrap(td.path(), &factory_registry(), &Default::default())
        .expect("a parked node must not fail the boot plan");
    let cell = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/probe/worker~0.1.0")
        .expect("the parked node is planned");
    assert!(!cell.active, "the parked node is inactive");
    assert_eq!(
        cell.params["sandbox"]["filesystem"]["read"][0], "${PROBE_DIR}",
        "the parked node keeps its placeholder verbatim"
    );
}

/// Unparking validates what it unparks: a parked node whose resolved value is
/// not absolute is refused by `move_nodes` with `invalid_params` before
/// anything moves; with an absolute value the same move commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unparking_a_node_with_a_bad_resolved_value_is_refused_before_the_rename() {
    set_term_timeout_ms_for_test(30_000);
    let td = tree();
    let good_env = std::fs::read_to_string(td.path().join(".env")).unwrap();

    let h = start(&td).await.expect("the first boot");
    committed(
        &submit_diff(
            &h,
            "/probe",
            json!({
                "add_nodes": [{"name": "box", "template": "box@0.1.0"}],
                "add_edges": [{"from": "./box", "to": "./capture"}]
            }),
        )
        .await,
        "growing box@0.1.0",
    );
    committed(
        &submit_diff(
            &h,
            "/probe",
            json!({"replace_nodes": [{"match": {"name": "box"},
                                      "with": {"template": "box@0.2.0"}}]}),
        )
        .await,
        "lifting box to 0.2.0",
    );

    let parked_dir = td.path().join("main/probe/box/worker~0.1.0");
    let target_dir = td.path().join("main/probe/box/worker-v1");
    let unpark = json!({"move_nodes": [{"match": {"name": "worker~0.1.0"}, "to": "worker-v1"}]});

    std::fs::write(td.path().join(".env"), "PROBE_DIR=relative/dir\n").unwrap();
    let refused = submit_diff(&h, "/probe/box", unpark.clone()).await;
    match &refused {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "invalid_params", "refusal: {details}");
            assert!(
                details.contains("must be an absolute path") && details.contains("relative/dir"),
                "the refusal names the param and the value: {details}"
            );
        }
        other => panic!("unparking a node with a relative path must be refused: {other:?}"),
    }
    assert!(parked_dir.is_dir(), "the refused move renames nothing");
    assert!(!target_dir.exists(), "the refused move creates nothing");
    let rows = box_rows(&h).await;
    assert!(
        rows.iter()
            .any(|r| r.path == "/probe/box/worker~0.1.0" && !r.active),
        "the parked row is untouched: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.path == "/probe/box/worker-v1"),
        "no row at the target: {rows:?}"
    );

    std::fs::write(td.path().join(".env"), good_env).unwrap();
    committed(
        &submit_diff(&h, "/probe/box", unpark).await,
        "unparking with an absolute path",
    );
    assert!(target_dir.is_dir(), "the committed move renames the node");
    h.shutdown().await;
}

/// The other half of the boot condition: a parked path that `colony.db`
/// still carries as `active` would spawn, so the boot validates it -- a
/// relative value fails the boot with `InvalidParams` for exactly that path;
/// at rest (no overlay row) the same node plans.
#[test]
fn a_parked_node_the_overlay_carries_as_active_is_still_validated_at_boot() {
    let td = tree();
    write(
        td.path(),
        "main/probe/worker~0.1.0/config.json",
        &worker_config("0.1.0", "relative/dir"),
    );
    plan_bootstrap(td.path(), &factory_registry(), &Default::default())
        .expect("a parked node at rest plans whatever its params say");

    let mut overlay = meclaw_colony::RegistryOverlay::new();
    overlay.insert(
        Path::new("/probe/worker~0.1.0"),
        (Uuid::now_v7(), "active".to_string()),
    );
    let errs = plan_bootstrap(td.path(), &factory_registry(), &overlay)
        .expect_err("a parked node that would spawn is validated");
    let rendered = format!("{errs:?}");
    assert!(
        rendered.contains("InvalidParams")
            && rendered.contains("worker~0.1.0")
            && rendered.contains("must be an absolute path"),
        "the boot error names the parked path and the param: {rendered}"
    );
}
