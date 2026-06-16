//! Phase-9 CodeCellFactory: stateless_dispatcher mit CodeCell.

use crate::code::{CodeCell, CodeParams};
use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// `code`-Cell factory. Stateless via `stateless_dispatcher`.
///
/// `multi_send_capable` is read from `contract.multi_send_capable` (the
/// `CellFactory` trait param, Phase 11). The Phase-9 `params`-bridge
/// (`raw_params["multi_send_capable"]`) has been removed.
pub struct CodeCellFactory;

impl CellFactory for CodeCellFactory {
    /// Pre-spawn validation. Same parse path as `spawn_cell`.
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        CodeParams::parse(raw).map(|_| ())
    }

    /// Spawn a `code`-Cell instance via the stateless dispatcher.
    ///
    /// `cell_dir` is unused (`_cell_dir`) — `code` is stateless and has
    /// no `cell.db` (Brainstorm E9). The underscore prefix signals this
    /// is intentionally unused.
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw_params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let params = CodeParams::parse(&raw_params)?;
        let max_concurrency = params.max_concurrency.unwrap_or(4);
        // Reads contract.multi_send_capable from the CellFactory trait param (Phase 11).
        let multi_send_capable = contract.multi_send_capable;
        // Paket 7 (P13/D-017): carry compiled emits + effective validate flag.
        let emits = contract.emits.clone();
        // Befund 2 (Phase-1): `code` is an always-on trust boundary — its output
        // is validated unconditionally, independent of the debug build or
        // colony.json `strict_validation`. `resolve_validate_emits` remains the
        // knob for the (not-yet-wired) non-code emitting cells; the code path
        // forces always-on.
        let validate_emits = true;

        let cell = Arc::new(CodeCell::new(
            params.clone(),
            multi_send_capable,
            emits,
            validate_emits,
        ));
        let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
        // Phase-13.5 Lifecycle-3b Task 3 + P3-A4 funnel: initial dispatcher via
        // `build_stateless_task` (owns the peace-keep-alive; stateless → no
        // cell.db → death_ack on dispatcher task-end). RespawnFn passes
        // `colony_inbox = None`.
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
            path.clone(),
            rx,
            outputs_tx.clone(),
            cell.clone(),
            max_concurrency,
            message_timeout,
            Some(colony_inbox_tx.clone()),
            blob_store.clone(),
            contract.consumes.clone(),
        );

        // RespawnFn: clones outside the closure, no .await inside
        // (Phase-5-Tripwire). `cell: Arc<CodeCell>` is Send + Sync —
        // safe to share across respawn cycles (stateless).
        // `contract` is cloned here so the respawn closure can rebuild the
        // cell with the same contract settings on restart (sync, no await).
        let respawn_mailbox_capacity = mailbox_capacity;
        let r_path = path.clone();
        let r_out = outputs_tx.clone();
        let r_cell = cell.clone();
        let r_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let r_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let p = r_path.clone();
                let o = r_out.clone();
                let c = r_cell.clone();
                let b = r_blob.clone();
                // Stateless respawn is intentionally bare (no renotify,
                // colony_inbox = None). Dropping stop_tx/death_ack_rx is
                // behaviorally identical to the old bare `None,None,None` spawn
                // (stop-fut parks, death_ack unobserved). Peace-keep-alive lives
                // in the helper.
                let (j, peace_rx, _stop_tx, _death_ack_rx, backstop_rx) =
                    build_stateless_task(
                        p,
                        r,
                        o,
                        c,
                        max_concurrency,
                        message_timeout,
                        None,
                        b,
                        r_consumes.clone(),
                    );
                (s, j, peace_rx, backstop_rx)
            },
        );

        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    /// Paket-8: boot-inactive eager respawn (No-Delete reconnect-after-reboot).
    /// Builds the SAME `Arc<CodeCell>` as `spawn_cell` and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` only if
    /// params no longer parse (defensive; validated at boot).
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        raw_params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let params = CodeParams::parse(&raw_params).ok()?;
        let max_concurrency = params.max_concurrency.unwrap_or(4);
        let cell = Arc::new(CodeCell::new(
            params,
            contract.multi_send_capable,
            contract.emits.clone(),
            // Befund 2 (Phase-1): `code` is an always-on trust boundary (see
            // `spawn_cell`) — force always-on on the restart path too.
            true,
        ));
        Some(meclaw_colony::build_stateless_boot_inactive_respawn(
            path,
            outputs_tx,
            cell,
            max_concurrency,
            message_timeout,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::CellFactory;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, Path};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_reads_multi_send_capable_from_contract() {
        // Spawn with contract.multi_send_capable=true — smoke test (no crash).
        let factory = Arc::new(CodeCellFactory);
        let (out_tx, _) = tokio::sync::mpsc::channel(8);
        let params =
            json!({"runner":"python3","script_inline":"print('x')","external_timeout_ms":5000});
        let cv = meclaw_colony::ContractView {
            multi_send_capable: true,
            ..Default::default()
        };
        let td = tempfile::TempDir::new().unwrap();
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/c"),
                params,
                out_tx,
                td.path().to_path_buf(),
                cv,
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        drop(spawned);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_ignores_legacy_params_multi_send_capable() {
        // Phase-9-Bridge is removed: params.multi_send_capable is IGNORED.
        // contract.default() = false  →  cell built with multi_send_capable=false.
        let factory = Arc::new(CodeCellFactory);
        let (out_tx, _) = tokio::sync::mpsc::channel(8);
        let params = json!({
            "runner":"python3","script_inline":"print('x')",
            "external_timeout_ms":5000,
            "multi_send_capable": true   // legacy — must NOT be read
        });
        let cv = meclaw_colony::ContractView::default(); // multi_send_capable=false
        let td = tempfile::TempDir::new().unwrap();
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let _spawned = factory
            .spawn_cell(
                Path::new("/c"),
                params,
                out_tx,
                td.path().to_path_buf(),
                cv,
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        // Concrete proof (array-output → contract_violation) is in cell.rs tests.
        // Here: spawn succeeds; cell was built with multi_send_capable=false.
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_spawn_emits_via_dispatcher() {
        let raw = json!({
            "runner":"python3",
            "script_inline": r#"import sys,json; sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"ok"}]}))"#,
            "external_timeout_ms": 10000
        });
        let (otx, mut orx) = tokio::sync::mpsc::channel(8);
        let td = tempfile::TempDir::new().unwrap();
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(CodeCellFactory)
            .spawn_cell(
                Path::new("/code"),
                raw,
                otx,
                td.path().to_path_buf(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let msg = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[]})))
            .reply_to(Path::new("/sink"))
            .build();
        let sender = match spawned {
            SpawnedCellKind::Active { sender, .. } => sender,
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();
        let em = orx.recv().await.unwrap();
        assert_eq!(em.content["header"]["exit_code"], 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn code_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let factory = Arc::new(CodeCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let td = tempfile::TempDir::new().unwrap();
        let params =
            json!({"runner":"python3","script_inline":"print('x')","external_timeout_ms":5000});
        let hook = factory.build_boot_inactive_respawn(
            Path::new("/c"),
            params,
            out_tx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None, // idle_timeout
            0,    // cell_timeout
            None, // message_timeout
            None, // blob_store
            1000, // mailbox_capacity
        );
        assert!(
            hook.is_some(),
            "stateless code factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
