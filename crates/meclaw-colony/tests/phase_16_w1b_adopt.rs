//! Phase-16 W1b A5b (Ruling 2026-06-12, tests c + d): the adoption path "2b".
//!
//! A mutation `add_nodes` entry carrying an `adopt` block instantiates from an
//! EXISTING, UNREGISTERED on-disk node — the template-instantiation pipeline
//! MINUS template lookup/copy: full validation of the existing config (schema,
//! contract, type/version match against the `adopt` expectation), fresh cell_id
//! (ID-Vergabe), substitution, registration. The on-disk content (incl. any
//! `cell.db`) is preserved; only `config.json` is overwritten with the patched
//! (fresh-id) version. `adopt` and `template` are mutually exclusive; an `adopt`
//! without a declared `type` is a `schema` reject (no blind adoption).

use meclaw_colony::{CellFactory, ColonyMsg, ContractView, MutationOutcome, SpawnedCellKind};
use meclaw_core::{CellEmission, JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The on-disk `cell.id` of a cell dir's config.json, if present.
fn read_cell_id(dir: &std::path::Path, rel: &str) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(rel).join("config.json")).ok()?;
    let v: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    v.get("cell")
        .and_then(|c| c.get("id"))
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
}

/// `(cell_id, status)` of a registry row in colony.db, if present.
fn registry_row(db_path: &std::path::Path, path: &str) -> Option<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).ok()?;
    conn.query_row(
        "SELECT cell_id, status FROM registry WHERE path = ?1",
        [path],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
}

async fn send_mutation(h: &ColonyHandle, payload: meclaw_core::JsonValue) -> MutationOutcome {
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

/// (c) 2b positive: a valid pre-placed unregistered node + an adopt mutation
/// (with a port-edge) ⇒ committed, registered with a FRESH cell_id, validated,
/// connected + active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_registers_existing_node_fresh_id_validated_active() {
    let td = tempfile::TempDir::new().unwrap();
    // `main` is the FS anchor (find_root_cell_dir) — never registered itself.
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    // The adopt target: a valid echo cell placed offline, UNREGISTERED, NO cell.id.
    write(
        td.path(),
        "main/foo/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{CONTRACT}}}"#),
    );

    let h = ColonyHandle::new_with_echo_at(td.path());
    // A pre-registered sink so the port-edge endpoint resolves.
    h.spawn(Path::new("/sink"), || EchoMockCell::new(Path::new("/sink")))
        .await;

    // Pre-condition: the offline-built config has NO cell.id yet.
    assert!(
        read_cell_id(td.path(), "main/foo").is_none(),
        "pre-condition: offline node must have no cell.id before adoption"
    );

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "foo", "adopt": {"type": "echo", "version": "0.1.0"}}],
                "add_edges": [{"from": "./foo", "to": "./sink"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "adopt of a valid unregistered node must commit, got {outcome:?}"
    );

    // ID-Vergabe: adoption minted a fresh cell.id imprint into the overwritten
    // config.json (the offline node had none) — proves the staging/patch +
    // config-overwrite ran, not a silent no-op. (The colony.db registry holds
    // the authoritative id, minted independently at registration time — the
    // config.json `cell.id` is the bootstrap imprint, never read back for
    // identity; that pre-existing split is out of W1b scope.)
    let minted = read_cell_id(td.path(), "main/foo").expect("adoption must mint a cell.id imprint");
    assert!(
        Uuid::parse_str(&minted).is_ok(),
        "minted cell.id must be a valid uuid, got {minted}"
    );

    // registriert + per Port-Edge aktiv: flush + inspect colony.db registry.
    h.shutdown().await;
    let (cid, status) =
        registry_row(&td.path().join("colony.db"), "/foo").expect("/foo must be registered");
    assert_eq!(
        status, "active",
        "an adopted node wired by a port-edge must be active"
    );
    assert!(
        Uuid::parse_str(&cid).is_ok(),
        "registry must hold a fresh valid cell_id for the adopted node, got {cid}"
    );
}

/// (d.1) 2b negative — type mismatch: the on-disk node is an `echo`, but the
/// adopt block declares `type: "bash"` ⇒ pre-destructive `resume_type_mismatch`
/// reject; the on-disk config.json is left untouched (no cell.id minted).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_type_mismatch_rejects_pre_destructively() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        td.path(),
        "main/foo/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{CONTRACT}}}"#),
    );
    let h = ColonyHandle::new_with_echo_at(td.path());

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "foo", "adopt": {"type": "bash"}}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "resume_type_mismatch",
            "adopt type mismatch must reject as resume_type_mismatch"
        ),
        other => panic!("expected resume_type_mismatch reject, got {other:?}"),
    }
    // Pre-destructive: the on-disk config was NOT rewritten (no minted id).
    assert!(
        read_cell_id(td.path(), "main/foo").is_none(),
        "a rejected adopt must not touch the on-disk config.json"
    );
}

/// (d.2) 2b negative — invalid existing config: the on-disk node is missing the
/// mandatory `contract` keys ⇒ pre-destructive `contract_incomplete` reject
/// (staging validation, before the config-overwrite rename).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_invalid_existing_config_rejects() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    // No `contract` block → contract-presence validation must reject.
    write(
        td.path(),
        "main/bad/config.json",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"}}"#,
    );
    let h = ColonyHandle::new_with_echo_at(td.path());

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "bad", "adopt": {"type": "echo"}}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "contract_incomplete",
            "an adopt of a contract-incomplete node must reject"
        ),
        other => panic!("expected contract_incomplete reject, got {other:?}"),
    }
}

/// (d.3) 2b grammar — `adopt` and `template` are mutually exclusive ⇒ `schema`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_with_template_is_schema_reject() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        td.path(),
        "main/foo/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{CONTRACT}}}"#),
    );
    let h = ColonyHandle::new_with_echo_at(td.path());

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "foo", "template": "x", "adopt": {"type": "echo"}}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "schema",
            "adopt + template must reject as schema (mutually exclusive)"
        ),
        other => panic!("expected schema reject, got {other:?}"),
    }
}

/// (d.4) 2b grammar — a bare `adopt` without a declared `type` ⇒ `schema`
/// (no blind adoption, Marcus 2026-06-12).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_without_type_is_schema_reject() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        td.path(),
        "main/foo/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{CONTRACT}}}"#),
    );
    let h = ColonyHandle::new_with_echo_at(td.path());

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "foo", "adopt": true}]}
        }),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "schema",
            "bare adopt without type must reject as schema"
        ),
        other => panic!("expected schema reject, got {other:?}"),
    }
}

/// A factory whose `spawn_cell` always fails — drives the post-rename spawn
/// reject on the adoption path (the config-overwrite already happened; spawn
/// then fails). `validate_params` passes so the reject is at spawn, not validate.
struct FailSpawnFactory;
impl CellFactory for FailSpawnFactory {
    fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _: Path,
        _: JsonValue,
        _: mpsc::Sender<CellEmission>,
        _: std::path::PathBuf,
        _: ContractView,
        _: mpsc::Sender<ColonyMsg>,
        _: Option<std::time::Duration>,
        _: i64,
        _: Option<std::time::Duration>,
        _: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _: usize,
    ) -> Result<SpawnedCellKind, String> {
        Err("spawn always fails (adopt crash-atomicity test)".into())
    }
}

/// (f) Adoption crash-atomicity (Staging-/No-Delete semantics, repro-style
/// `bootstrap_crash_recovery`): a spawn failure DURING the adopt apply (after
/// the config-overwrite) must NOT destroy the builder's pre-placed directory.
/// The existing `cell.db` is preserved (No-Delete), the staging residue is
/// swept, and the mutation rejects. Red-demo: removing the `preexisting_target`
/// guard in `sweep_reject_residue` deletes the adopted dir + its cell.db.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_spawn_failure_preserves_existing_dir_and_sweeps_staging() {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        td.path(),
        "main/foo/config.json",
        &format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{CONTRACT}}}"#),
    );
    // The builder's pre-placed cell.db — a sentinel that MUST survive a failed adopt.
    let cell_db = td.path().join("main/foo/cell.db");
    std::fs::write(&cell_db, b"SENTINEL_CELL_DB_MUST_SURVIVE").unwrap();

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".into(),
            Arc::new(FailSpawnFactory) as Arc<dyn CellFactory>,
        )],
    );

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "foo", "adopt": {"type": "echo"}}]}
        }),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "spawn", "spawn failure must reject as spawn")
        }
        other => panic!("expected spawn reject, got {other:?}"),
    }

    // No-Delete: the builder's directory + its cell.db survive the failed adopt.
    assert!(
        td.path().join("main/foo/config.json").exists(),
        "adopted dir must NOT be deleted on a spawn-reject (No-Delete)"
    );
    let db_after = std::fs::read(&cell_db).expect("cell.db must still exist after a failed adopt");
    assert_eq!(
        db_after, b"SENTINEL_CELL_DB_MUST_SURVIVE",
        "existing cell.db must be byte-preserved through a failed adopt"
    );

    // Staging residue is swept — no stranded `.staging/<id>` left behind.
    let staging = td.path().join(".staging");
    let staging_entries = std::fs::read_dir(&staging)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        staging_entries, 0,
        "staging residue must be swept on reject"
    );

    h.shutdown().await;
}
