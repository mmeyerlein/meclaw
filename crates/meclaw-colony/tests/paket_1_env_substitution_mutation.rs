//! Paket-1 (d) — `.env` substitution semantics through the real
//! `handle_mutation` path: `${V:-fb}` POSIX default in three `.env` states,
//! `${V:=x}` rejected as `unsupported_substitution`, and `$${V}` escaped to a
//! literal `${V}`.
//!
//! The mutation path reads `<root>/.env` and runs `substitute_full` over the
//! diff before staging (see `colony.rs::handle_mutation`). A recorder factory
//! captures the spawned cell's resolved `params`, so we assert the substituted
//! value actually lands in `params.greeting`. The reject case asserts the
//! mutation outcome's `error_code`; the escape case asserts the literal.

use meclaw_colony::{
    CellFactory, ColonyMsg, ContractView, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
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
                    p,
                    rx,
                    o,
                    64,
                    cell,
                    db,
                    Some(peace_tx),
                    Some(cit),
                    None,
                    None,
                    None,
                 None,
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

/// Build a fresh colony root with a `recorder` template and the given `.env`
/// body, returning the handle + the captured-params slot.
fn setup(
    env_body: &str,
) -> (
    ColonyHandle,
    tempfile::TempDir,
    Arc<Mutex<Option<JsonValue>>>,
) {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join(".env"), env_body).unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let tpl_dir = td.path().join("templates").join("recorder");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"recorder"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"recorder"},"params":{"greeting":""},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let last_params = Arc::new(Mutex::new(None));
    let factory: Arc<dyn CellFactory> = Arc::new(ParamsRecorderFactory {
        last_params: last_params.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("recorder".to_string(), factory)]);
    (h, td, last_params)
}

/// `${V:-fb}` resolves to the fallback when V is unset, when V is empty, and to
/// the real value when V is set non-empty — proven through the mutation path
/// for all three `.env` states.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_default_operator_three_states_via_mutation() {
    for (env_body, expected, label) in [
        ("OTHER=x\n", "fb", "V unset"),
        ("V=\n", "fb", "V empty"),
        ("V=real\n", "real", "V set non-empty"),
    ] {
        let (h, td, last_params) = setup(env_body);
        rescan_templates(&h, td.path().join("templates")).await;

        let outcome = send_mutation(
            &h,
            meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "rec",
                    "template": "recorder",
                    "override_params": {"greeting": "${V:-fb}"}
                }]}
            }),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "[{label}] add_nodes must commit; got {outcome:?}"
        );

        let params = last_params.lock().unwrap().clone().expect("cell spawned");
        assert_eq!(
            params["greeting"], expected,
            "[{label}] ${{V:-fb}} must resolve to '{expected}'; params: {params}"
        );

        h.shutdown().await;
    }
}

/// `${V:=x}` is an unsupported POSIX operator form → the mutation is rejected
/// with `error_code == "unsupported_substitution"` (pre-destructive).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_unsupported_operator_rejects_via_mutation() {
    let (h, td, last_params) = setup("V=ignored\n");
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "rec",
                "template": "recorder",
                "override_params": {"greeting": "${V:=x}"}
            }]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "unsupported_substitution",
            "${{V:=x}} must reject as unsupported_substitution"
        ),
        other => panic!("expected Rejected for ${{V:=x}}, got {other:?}"),
    }
    assert!(
        last_params.lock().unwrap().is_none(),
        "no cell may spawn for a rejected substitution"
    );

    h.shutdown().await;
}

/// `$${V}` is the escape form → it must survive as the literal `${V}` in the
/// spawned cell's params (no env lookup).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_escape_yields_literal_via_mutation() {
    // V is set, to prove the escape suppresses the lookup entirely.
    let (h, td, last_params) = setup("V=should-not-appear\n");
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "rec",
                "template": "recorder",
                "override_params": {"greeting": "$${V}"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "$${{V}} escape must commit; got {outcome:?}"
    );

    let params = last_params.lock().unwrap().clone().expect("cell spawned");
    assert_eq!(
        params["greeting"], "${V}",
        "$${{V}} must yield the literal ${{V}}, not the env value; params: {params}"
    );

    h.shutdown().await;
}

/// GH #20 -- a committed `add_nodes` leaves NO secret VALUE on disk, on either
/// materialization path (`params.*` and `contract.settings.*.default`), while
/// the spawned cell still receives the resolved value.
///
/// This is the end-to-end shape of the two production finds on the issue: one
/// mutation used to leak one secret, a rebirth all of them. Here the real
/// `handle_mutation` pipeline runs and the instantiated file is read back raw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instantiation_writes_no_secret_value_into_config_json() {
    const SENTINEL: &str = "sk-e2e-do-not-materialize";
    let (h, td, last_params) = setup(&format!("SECRET_API_KEY={SENTINEL}\n"));
    // The template references the secret on BOTH paths.
    std::fs::write(
        td.path().join("templates/recorder/config.json"),
        r#"{"cell":{"type":"recorder"},"params":{"api_key":"${SECRET_API_KEY}"},
            "contract":{"version":"0.1.0","consumes":{},
              "settings":{"api_key":{"type":"string","default":"${SECRET_API_KEY}"}}}}"#,
    )
    .unwrap();
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "rec", "template": "recorder"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes must commit; got {outcome:?}"
    );

    let raw = std::fs::read_to_string(td.path().join("main/rec/config.json")).unwrap();
    assert!(
        !raw.contains(SENTINEL),
        "the secret VALUE must appear nowhere in the instance config: {raw}"
    );
    assert_eq!(
        raw.matches("${SECRET_API_KEY}").count(),
        2,
        "both references stay tokens (params + settings default): {raw}"
    );

    let params = last_params.lock().unwrap().clone().expect("cell spawned");
    assert_eq!(
        params["api_key"], SENTINEL,
        "the cell itself is born with the resolved value; params: {params}"
    );

    h.shutdown().await;
}
