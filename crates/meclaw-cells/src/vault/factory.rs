//! `VaultCellFactory` — open `cell.db`, apply the vault DDL, hand over a cell
//! that is LOCKED.
//!
//! The one thing worth saying about the lifecycle: a woken vault is locked,
//! always. A vault that could resume its unlocked state across a sleep would
//! have to keep the key somewhere that survives the sleep, and there is no such
//! place that is not a worse version of the problem the vault exists to solve.
//! So the key lives in the task, dies with the task, and comes back only
//! through an unlock that attests first.

use super::degraded::DegradedVaultCell;
use super::{VaultCell, VaultParams, store};
use meclaw_colony::persist::open_or_create_cell_db_with_status;
use meclaw_colony::{
    CellFactory, DbConn, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
    renotify_stop_wiring,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The `vault` cell factory. Unit struct — all per-instance config is params.
pub struct VaultCellFactory;

impl CellFactory for VaultCellFactory {
    /// This type's tables are fixed in its own Rust code, so a seed header --
    /// which describes rows, not a schema -- can never describe them (GH #399,
    /// same class as GH #398). Declaring this keeps the mutation staging seeder
    /// out of the database entirely.
    ///
    /// It carries an obligation: a type that declares this must load its own
    /// seed files, because nobody else will. `vault` has no such loader and
    /// wants none, so the default `validate_cell_dir` refuses a `seed/*.jsonl`
    /// beside it by name instead of ignoring it in silence.
    fn owns_schema(&self) -> bool {
        true
    }

    /// The `cell.type` string, so the refusal above names what an operator
    /// wrote in `config.json` rather than a Rust identifier.
    fn type_name(&self) -> &'static str {
        "vault"
    }

    /// Lazy stateful, like the store: mailbox pair now, task on first message.
    fn is_lazy(&self) -> bool {
        true
    }

    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        VaultParams::parse(raw).map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
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
        // Parser invariant: the same parse both entry points use.
        let _params = VaultParams::parse(&raw_params)?;

        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // GH #160: the attestation capability. Derived from what this factory
        // already holds — the contract, the cell's own path, the colony inbox —
        // and handed to every incarnation of the cell (wake and respawn alike).
        // Undeclared ⇒ `None` ⇒ the vault cannot attest and therefore never
        // unlocks, which is the fail-closed direction.
        let neighbourhood = meclaw_colony::NeighbourhoodView::for_contract(
            &contract,
            path.clone(),
            colony_inbox_tx.clone(),
        );

        let respawn_dir = cell_dir.clone();
        let respawn_path = path.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_birth = raw_params.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_blob = blob_store.clone();
        let respawn_consumes = contract.consumes.clone();
        // GH #260: the substrate half of the write boundary travels with them.
        let respawn_bounds = contract.transfer_bounds();
        let respawn_neighbourhood = neighbourhood.clone();
        let respawn_capacity = mailbox_capacity;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                // Runs inside the colony task's await-free restart barrier —
                // nothing here may panic (A1′ class).
                let (sender, receiver) = mpsc::channel::<Message>(respawn_capacity);
                let built = build_vault(
                    &respawn_dir,
                    &respawn_birth,
                    &respawn_path,
                    "respawn",
                    respawn_neighbourhood.clone(),
                );
                let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = match built {
                    Ok((cell, db)) => build_stateful_task_with_peace(
                        respawn_path.clone(),
                        receiver,
                        respawn_outputs.clone(),
                        respawn_inbox_tx.clone(),
                        idle_timeout,
                        message_timeout,
                        cell_timeout,
                        cell,
                        db,
                        respawn_blob.clone(),
                        respawn_consumes.clone(),
                        respawn_bounds.clone(),
                    ),
                    Err(reason) => meclaw_colony::build_stateless_task(
                        respawn_path.clone(),
                        receiver,
                        respawn_outputs.clone(),
                        Arc::new(DegradedVaultCell::new(reason)),
                        1,
                        message_timeout,
                        Some(respawn_inbox_tx.clone()),
                        respawn_blob.clone(),
                        None,
                    ),
                };
                renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (sender, join, peace_rx, backstop_rx)
            },
        );

        let wake_dir = cell_dir.clone();
        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_birth = raw_params.clone();
        let wake_inbox_tx = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob = blob_store.clone();
        let wake_consumes = contract.consumes.clone();
        // GH #260: the substrate half of the write boundary travels with them.
        let wake_bounds = contract.transfer_bounds();
        let wake_neighbourhood = neighbourhood.clone();
        let wake: WakeFn = Box::new(move |receiver: mpsc::Receiver<Message>| {
            // Same class as the respawn: synchronous, inside the colony task.
            let built = build_vault(
                &wake_dir,
                &wake_birth,
                &wake_path,
                "wake",
                wake_neighbourhood.clone(),
            );
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = match built {
                Ok((cell, db)) => build_stateful_task_with_peace(
                    wake_path.clone(),
                    receiver,
                    wake_outputs.clone(),
                    wake_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    wake_blob.clone(),
                    wake_consumes.clone(),
                    wake_bounds.clone(),
                ),
                Err(reason) => meclaw_colony::build_stateless_task(
                    wake_path.clone(),
                    receiver,
                    wake_outputs.clone(),
                    Arc::new(DegradedVaultCell::new(reason)),
                    1,
                    message_timeout,
                    Some(wake_inbox_tx.clone()),
                    wake_blob.clone(),
                    None,
                ),
            };
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            (stop_tx, death_ack_rx)
        });

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

/// Open the database, apply the DDL, build a locked cell. Shared by wake and
/// respawn so both degrade identically.
///
/// The DDL failing is treated as hard here, unlike in the store where a missing
/// index costs one operation. A vault whose tables are not there cannot store,
/// cannot audit, and cannot tell the difference between "no such secret" and
/// "no such table" — that is not a cell, that is a trap.
fn build_vault(
    cell_dir: &std::path::Path,
    birth: &JsonValue,
    path: &Path,
    phase: &str,
    neighbourhood: Option<meclaw_colony::NeighbourhoodView>,
) -> Result<(VaultCell, DbConn), String> {
    let (conn, _status) = match open_or_create_cell_db_with_status(&cell_dir.join("cell.db")) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(
                path = path.as_str(),
                error = %e,
                "vault: cell.db could not be opened at {phase} — the cell starts LOCKED and \
                 answers every message with an error"
            );
            return Err(format!("cell.db could not be opened at {phase}: {e}"));
        }
    };
    if let Err(e) = store::apply_ddl(&conn) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "vault: the DDL failed at {phase} — the cell starts LOCKED"
        );
        return Err(format!("vault DDL failed at {phase}: {e}"));
    }
    let params = match VaultParams::parse(birth) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(
                path = path.as_str(),
                error = %e,
                "vault: the birth params no longer parse at {phase} — the cell starts LOCKED"
            );
            return Err(format!("params unusable at {phase}: {e}"));
        }
    };
    Ok((
        VaultCell::new(params, neighbourhood),
        DbConn::wrap(conn, None),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn validate_params_shares_the_parse_path_with_spawn() {
        let f = VaultCellFactory;
        assert!(
            f.validate_params(&json!({})).is_err(),
            "broker is mandatory"
        );
        assert!(
            f.validate_params(&json!({"broker": "/main/access/broker"}))
                .is_ok()
        );
    }

    #[test]
    fn a_vault_is_a_lazy_stateful_kind() {
        assert!(VaultCellFactory.is_lazy());
    }

    #[test]
    fn build_vault_creates_its_tables_and_starts_locked() {
        let dir = tempfile::tempdir().unwrap();
        let (_cell, _db) = match build_vault(
            dir.path(),
            &json!({"broker": "/main/access/broker"}),
            &Path::new("/main/access/vault"),
            "test",
            None,
        ) {
            Ok(pair) => pair,
            Err(e) => panic!("a fresh vault must build: {e}"),
        };
        // The tables exist in the freshly created cell.db.
        let conn = rusqlite::Connection::open(dir.path().join("cell.db")).unwrap();
        for table in ["vault_meta", "vault_secrets", "vault_audit"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }
    }

    #[test]
    fn unparseable_birth_params_degrade_rather_than_panic() {
        let dir = tempfile::tempdir().unwrap();
        let err = match build_vault(
            dir.path(),
            &json!({"broker": 42}),
            &Path::new("/main/access/vault"),
            "wake",
            None,
        ) {
            Err(e) => e,
            Ok(_) => panic!("unparseable params must not build a vault"),
        };
        assert!(err.contains("params unusable at wake"), "{err}");
    }
}
