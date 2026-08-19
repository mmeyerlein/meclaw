//! U8 (roadmap 2026-06-11; RULED A8 2026-06-12, escalated + fixed):
//! env source consistency. The colony remembers its env source from startup;
//! ALL substitution paths (boot, mutation/instantiation, 2b adoption) read from
//! the SAME source. Before: `--env <file>` only applied on the boot path; the
//! `${VAR}` substitution at mutation time still read the default source
//! `<root>/.env` and silently ignored the operator override.
//!
//! Discriminator: the env file lies OUTSIDE `root` (reachable only via the
//! remembered source). `<root>/.env` is absent. Without the fix,
//! `handle_mutation` reads `<root>/.env` → `${U8_VAR}` is missing →
//! `env_var_missing` reject. With the fix it reads the remembered source →
//! resolved → committed, and the substituted value lands as a positive receipt
//! in `params`.

use meclaw_colony::{
    CellFactory, ColonyMsg, ContractView, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Recorder factory: captures the resolved `params` JSON of each spawned cell.
struct ParamsRecorderFactory {
    last_params: Arc<Mutex<Option<JsonValue>>>,
}

impl CellFactory for ParamsRecorderFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        *self.last_params.lock().unwrap() = Some(params);
        let build = move || -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
            let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            let join = tokio::spawn(async move {
                let _keep_inject = inject_tx;
                cell_task_long_running(
                    p, rx, o, 64, cell, db, Some(peace_tx), Some(cit), None, None, None, None,
                    Default::default(),
                )
                .await;
            });
            (tx, join, peace_rx, backstop_rx)
        };
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let _ = &death_ack_tx;
        let respawn: RespawnFn = Box::new(build);
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
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

/// A fresh recorder factory registered under `"recorder"` + its capture slot.
fn recorder_factory() -> (Arc<Mutex<Option<JsonValue>>>, Arc<dyn CellFactory>) {
    let last_params = Arc::new(Mutex::new(None));
    let factory: Arc<dyn CellFactory> = Arc::new(ParamsRecorderFactory {
        last_params: last_params.clone(),
    });
    (last_params, factory)
}

/// Write the root tree (hive + a `recorder` template). Does NOT write
/// `<root>/.env` — the caller decides whether a default `.env` exists.
fn write_root_tree(td: &std::path::Path) {
    write(
        td,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let tpl_dir = td.join("templates").join("recorder");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"recorder"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"recorder"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// U8-a: a per-mutation-instantiated cell pulls `${U8_VAR}` from the FLAG source
/// (a `.env` OUTSIDE root), not from the absent default `<root>/.env`. Commit +
/// the resolved value in `params` is the positive receipt. Without the fix the
/// mutation rejects with `env_var_missing` (default source lacks the var).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_substitutes_from_pinned_env_source() {
    let td = tempfile::TempDir::new().unwrap();
    write_root_tree(td.path());
    // The env file lives OUTSIDE root — only reachable via the pinned source.
    // No `<root>/.env` is written: the default source has NO U8_VAR.
    let env_dir = tempfile::TempDir::new().unwrap();
    let env_file = env_dir.path().join("staging.env");
    std::fs::write(&env_file, "U8_VAR=from_staging\n").unwrap();

    let (last_params, factory) = recorder_factory();
    let h = ColonyHandle::new_with_factories_and_env_at(
        &td,
        vec![("recorder".to_string(), factory)],
        Some(env_file.clone()),
    );
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "rec",
                "template": "recorder",
                "override_params": {"greeting": "${U8_VAR}"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "mutation must commit by reading the pinned --env source; got {outcome:?}"
    );

    let params = last_params.lock().unwrap().clone().expect("cell spawned");
    assert_eq!(
        params["greeting"], "from_staging",
        "the per-mutation cell must resolve ${{U8_VAR}} from the pinned source; params: {params}"
    );

    h.shutdown().await;
}

/// U8-b: the 2b-adoption path substitutes from the SAME pinned source. An
/// `adopt` of an unregistered on-disk node carries `${U8_VAR}` in its
/// override_params → committed + resolved from the flag source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adoption_substitutes_from_pinned_env_source() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    // An unregistered, offline-built recorder node (no cell.id) to adopt.
    write(
        td.path(),
        "main/foo/config.json",
        r#"{"cell":{"type":"recorder"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let env_dir = tempfile::TempDir::new().unwrap();
    let env_file = env_dir.path().join("staging.env");
    std::fs::write(&env_file, "U8_VAR=adopted_value\n").unwrap();

    let (last_params, factory) = recorder_factory();
    let h = ColonyHandle::new_with_factories_and_env_at(
        &td,
        vec![("recorder".to_string(), factory)],
        Some(env_file.clone()),
    );

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "foo",
                "adopt": {"type": "recorder", "version": "0.1.0"},
                "override_params": {"greeting": "${U8_VAR}"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "2b-adoption must commit by reading the pinned --env source; got {outcome:?}"
    );

    let params = last_params.lock().unwrap().clone().expect("cell spawned");
    assert_eq!(
        params["greeting"], "adopted_value",
        "the adopted cell must resolve ${{U8_VAR}} from the pinned source; params: {params}"
    );

    h.shutdown().await;
}

/// U8-c (Bestands-Pin): without a pinned source (`None`), substitution falls
/// back to the default `<root>/.env` everywhere — the regression guard that the
/// fix did not break the default path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_pin_falls_back_to_root_env() {
    let td = tempfile::TempDir::new().unwrap();
    write_root_tree(td.path());
    // The default source carries the var.
    std::fs::write(td.path().join(".env"), "U8_VAR=from_root_env\n").unwrap();

    let (last_params, factory) = recorder_factory();
    // No pinned source → default `<root>/.env`.
    let h = ColonyHandle::new_with_factories_and_env_at(
        &td,
        vec![("recorder".to_string(), factory)],
        None,
    );
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "rec",
                "template": "recorder",
                "override_params": {"greeting": "${U8_VAR}"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "without --env the default <root>/.env must still resolve; got {outcome:?}"
    );

    let params = last_params.lock().unwrap().clone().expect("cell spawned");
    assert_eq!(
        params["greeting"], "from_root_env",
        "default source must still work; params: {params}"
    );

    h.shutdown().await;
}
