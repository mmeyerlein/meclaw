//! Phase-8 LlmCellFactory — production entry point for the `llm`-Cell.
//!
//! T23: unit-struct + `validate_params` (delegates to `LlmParams::parse`).
//! T24 fleshes out `spawn_cell` (Phase-7.5 `cell_dir`-substrate,
//! Client build/clone, schema_version-check init-only, RespawnFn sync).

use crate::llm::cell::LlmCell;
use crate::llm::params::LlmParams;
use crate::llm::seed;
use crate::llm::state::check_schema_version;
use meclaw_colony::persist::{
    OpenStatus, open_or_create_cell_db, open_or_create_cell_db_with_status,
};
use meclaw_colony::{
    CellFactory, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
    renotify_stop_wiring,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Rebuild the effective `LlmParams` for a wake/respawn (W4b restore).
///
/// Reads the runtime-param overlay from `cell.db` and replays it over the
/// birth-params (`config.json` `params`, already `${VAR}`-substituted by the
/// bootstrap), then re-parses. `config.json` is never written; a `cell.db`-wipe
/// (empty overlay) restores the bootstrap state (config.md § Access l.20).
/// Used by both the WakeFn and RespawnFn closures.
fn restore_params(conn: &rusqlite::Connection, birth: &JsonValue) -> Result<LlmParams, String> {
    // β: delegate to the generic params-overlay core (read overlay → merge over
    // birth → re-parse). `config.json` stays the instantiation snapshot.
    crate::params_overlay::restore::<LlmParams>(conn, birth)
}

/// Phase-8 `llm`-Cell factory. Unit-struct (no fields) — all per-instance
/// config lives in `params` (per cell-type-Konvention, see
/// `WebFetchCellFactory`, `BashCellFactory` etc.).
pub struct LlmCellFactory;

impl CellFactory for LlmCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    /// Pre-spawn validation. Routes through the SAME parse path as
    /// `spawn_cell` (parser-invariant per `meclaw_colony::CellFactory` doc):
    /// `validate_params(raw) ≡ LlmParams::parse(raw).map(|_| ())`.
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        LlmParams::parse(raw).map(|_| ())
    }

    /// Pre-spawn validation of the cell's on-disk assets (GH #99): the optional
    /// `seed/system.jsonl`. Routes through the SAME parse path `spawn_cell`
    /// uses (`seed::check_system_seed` → `parse_system_seed` ←
    /// `load_system_seed_if_present`), so `meclaw --validate` and the spawn
    /// agree on every syntactic verdict — the store's validate-equals-spawn
    /// invariant (issue #56), applied to the llm seed.
    fn validate_cell_dir(&self, raw: &JsonValue, cell_dir: &std::path::Path) -> Result<(), String> {
        let _ = raw;
        seed::check_system_seed(cell_dir).map_err(|e| format!("llm: {e}"))
    }

    /// Spawn an `llm`-Cell instance. Phase-7.5 substrate hands us
    /// `cell_dir`; we join `cell.db` underneath. Reqwest client is built
    /// ONCE (Phase-7-Pattern) and cloned into the RespawnFn — no rebuild
    /// path means no panic-only failure path. schema_version is checked
    /// init-only (sync RespawnFn cannot return Result, and cell.db is
    /// `db:own` so the version cannot drift across the cell's lifetime).
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw_params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Parser-Invariante: same path as validate_params (spawn-time validate
        // → Mutation-Reject on bad config). W4b: the closures capture the BIRTH
        // params Value and rebuild the effective params from the cell.db overlay
        // at each wake/respawn (`restore_params`) — config.json stays the
        // instantiation snapshot.
        LlmParams::parse(&raw_params)?;
        let birth_params = raw_params.clone();

        // Build reqwest client ONCE (Phase-7-Pattern, see web_fetch.rs:229-265).
        // Build-Failure → spawn_cell-Err → Mutation-Reject. Clone into the
        // Wake/respawn closures — no rebuild means no failure path that could
        // only panic.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;

        // GH #99: the seed file is parsed HERE, BEFORE `cell.db` is opened. Two
        // reasons, both load-bearing:
        //   * a syntactically broken seed (or a `{text_id}` leaf) fails THIS CELL
        //     with a named factory error — boot-plan reject, mutation reject —
        //     instead of a half-configured agent;
        //   * failing before the open means no `cell.db` file exists yet. Were it
        //     created first, the next spawn against the FIXED seed would see
        //     `Resumed` and skip the seed forever.
        seed::check_system_seed(&cell_dir).map_err(|e| format!("llm: {e}"))?;

        // Phase-13-K-2: NO initial cell-task spawn. The initial open of cell.db is
        // still needed — `check_schema_version` is the ONLY check site (WakeFn and
        // RespawnFn are sync and skip the schema check intentionally: db:own →
        // schema_version does not drift over the cell's lifetime). The connection
        // is closed again immediately; WakeFn opens fresh on the first wake.
        //
        // GH #99: this open is also the cell's ONE `OpenStatus` observation. It
        // is the first open of the cell's lifetime — the WakeFn always finds the
        // file already there — so `Created` here IS "fresh cell.db", and the
        // overview's no-re-seed-on-resume rule (§ Seed-Konzept) falls out of it:
        // every later spawn/wake/respawn sees `Resumed` and leaves the
        // accumulated `system.*` identity untouched.
        let (mut init_conn, status) = open_or_create_cell_db_with_status(&cell_dir.join("cell.db"))
            .map_err(|e| format!("open cell.db: {e}"))?;
        check_schema_version(&init_conn)?;
        if status == OpenStatus::Created {
            seed::load_system_seed_if_present(&mut init_conn, &cell_dir)
                .map_err(|e| format!("llm: {e}"))?;
        }
        drop(init_conn);

        // GH #87: the attachments[] read handle. A function of two things this
        // factory already holds — the contract (does the cell declare
        // `consumes.body.attachments`?) and the blob store that already rides
        // in for the delivery boundary. No eleventh `CellFactory` parameter.
        // `None` for every cell that does not declare consumption.
        let attachment_reader =
            meclaw_colony::AttachmentReader::for_contract(&contract, blob_store.clone());

        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — crash-restart path, fresh channel + fresh connection.
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn_path = path.clone();
        let respawn_birth = birth_params.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_client = client.clone();
        let respawn_cell_dir = cell_dir.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn_attachments = attachment_reader.clone();
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let conn = open_or_create_cell_db(&respawn_cell_dir.join("cell.db"))
                    .expect("respawn: open_or_create_cell_db failed");
                // W4b: replay the persisted params overlay over birth-params.
                let effective = restore_params(&conn, &respawn_birth)
                    .expect("respawn: restore params from cell.db overlay");
                let db = meclaw_colony::DbConn::wrap(conn, None);
                let cell = LlmCell::new(effective, respawn_client.clone())
                    .with_attachment_reader(respawn_attachments.clone());
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let (j, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateful_task_with_peace(
                    respawn_path.clone(),
                    r,
                    respawn_outputs.clone(),
                    respawn_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    respawn_blob.clone(),
                    respawn_consumes.clone(),
                );
                // Phase-13.5 Slice 4 T6: re-notify the colony of the fresh stop
                // pair (the frozen RespawnFn 3-tuple cannot return it). try_send,
                // never await — this closure runs in the await-free respawn
                // corridor (see `renotify_stop_wiring`).
                renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, j, peace_rx, backstop_rx)
            },
        );

        // WakeFn — NotYetSpawned/Asleep → Awake (Phase-13-K-2 NEU). Opens cell.db
        // fresh (db:own → no schema drift, no re-check needed), builds LlmCell +
        // task via the shared helper, registers the watcher like the bootstrap register.
        let wake_path = path.clone();
        let wake_birth = birth_params.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_client = client.clone();
        let wake_cell_dir = cell_dir.clone();
        let wake_inbox_tx = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let wake_consumes = contract.consumes.clone();
        let wake_attachments = attachment_reader.clone();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let conn = open_or_create_cell_db(&wake_cell_dir.join("cell.db"))
                .expect("wake: open_or_create_cell_db failed");
            // W4b: replay the persisted params overlay over birth-params.
            let effective = restore_params(&conn, &wake_birth)
                .expect("wake: restore params from cell.db overlay");
            let db = meclaw_colony::DbConn::wrap(conn, None);
            let cell = LlmCell::new(effective, wake_client.clone())
                .with_attachment_reader(wake_attachments.clone());
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                build_stateful_task_with_peace(
                    wake_path.clone(),
                    recv,
                    wake_outputs.clone(),
                    wake_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    wake_blob.clone(),
                    wake_consumes.clone(),
                );
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            // Phase-13.5 Lifecycle-3b Task 7.5: return the woken task's live
            // peace-stop wiring so the colony stores it in the RegistryEntry and
            // a later disconnect can peace-stop the cell + drain its mailbox.
            (stop_tx, death_ack_rx)
        });

        // Phase-13.5 Lifecycle-3b Task 3: placeholder peace-stop ends for the
        // Dormant (lazy-wake) variant. The task is spawned later in WakeFn, so
        // these ends are inert until Task 4 wires the lazy-wake peace-stop into
        // the registry; the colony drops them in Task 3.
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn restore_params_replays_overlay_over_birth() {
        // W4b (c-core): a persisted overlay survives a rebuild from birth-params.
        use meclaw_colony::persist::open_or_create_cell_db;
        let td = tempfile::TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        crate::params_overlay::persist_params_overlay(
            &mut conn,
            &[("model".to_string(), json!("gpt-4o-mini"))],
            100,
        )
        .unwrap();
        let birth = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let effective = restore_params(&conn, &birth).unwrap();
        assert_eq!(effective.model, "gpt-4o-mini");
        assert_eq!(effective.api_key.as_deref(), Some("x"));
    }

    #[test]
    fn restore_params_empty_overlay_is_birth() {
        // W4b (g-core): reset = cell.db wipe = empty overlay ⇒ bootstrap state.
        use meclaw_colony::persist::open_or_create_cell_db;
        let td = tempfile::TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let birth = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let effective = restore_params(&conn, &birth).unwrap();
        assert_eq!(effective.model, "gpt-4o");
    }

    #[test]
    fn validate_params_rejects_empty_object() {
        let r = LlmCellFactory.validate_params(&json!({}));
        assert!(
            r.is_err(),
            "empty params must reject (missing provider/model/api_key)"
        );
    }

    #[test]
    fn validate_params_accepts_minimal_valid() {
        let r = LlmCellFactory.validate_params(&json!({
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "sk-test"
        }));
        assert!(r.is_ok(), "minimal valid params must accept: {r:?}");
    }

    #[test]
    fn validate_params_rejects_non_openai_provider() {
        let r = LlmCellFactory.validate_params(&json!({
            "provider": "anthropic",
            "model": "claude-3",
            "api_key": "sk-test"
        }));
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(
            err.contains("openai"),
            "error mentions openai constraint: {err}"
        );
    }
}
