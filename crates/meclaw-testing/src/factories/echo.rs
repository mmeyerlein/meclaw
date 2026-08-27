//! `EchoCellFactory` — phase-4 test factory. Wraps `EchoMockCell` so the
//! filesystem bootstrap can spawn echo cells from `config.json` params.
//!
//! # `params.emitted_target` is a field, not a delivery address (GH #224)
//!
//! `emitted_target` writes the `target` field of the cell's emission. It does
//! NOT decide where that emission goes. The colony's outputs arm routes a cell
//! emission by the EMITTING cell's out-edges: a matching edge overlays the
//! target, and an emission that matches no out-edge dead-letters as `no_route`
//! (Ruling A1) no matter what `emitted_target` says.
//!
//! So a test topology needs BOTH: this param (otherwise the cell emits nothing
//! at all — see `EchoMockCell::emitted_target`) AND an out-edge from the cell
//! (a `graph.edges` entry in the parent hive, an `add_edges` mutation, or a
//! catch-all out-edge). The param was called `echo_to` until GH #224, which
//! read as a promise of delivery the factory never made.
//!
//! Parser invariant (see the `meclaw_colony::CellFactory` docs): `validate_params`
//! and `spawn_cell` share `parse_params_internal`.

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind};
use meclaw_core::{JsonValue, Path};
use std::sync::Arc;

/// Factory for phase-4 test cells (`EchoMockCell` wrapper).
pub struct EchoCellFactory;

/// Parsed Echo cell parameters (typed form of `params` block).
#[derive(Debug, Clone)]
pub(crate) struct EchoParams {
    /// `target` field written on the emission — NOT a route. The out-edge
    /// decides delivery; see the module doc.
    pub emitted_target: Path,
    pub emitted_header: Option<(String, JsonValue)>,
}

impl EchoCellFactory {
    /// Shared parse path for `validate_params` and `spawn_cell` (see
    /// the `meclaw_colony::CellFactory` docs — parser invariant).
    pub(crate) fn parse_params_internal(raw: &JsonValue) -> Result<EchoParams, String> {
        let emitted_target_str = raw
            .get("emitted_target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "params.emitted_target missing or not a string".to_string())?;
        let emitted_header = match raw.get("emitted_header") {
            None => None,
            Some(obj) => {
                let key = obj.get("key").and_then(|k| k.as_str()).ok_or_else(|| {
                    "params.emitted_header.key missing or not a string".to_string()
                })?;
                let value = obj
                    .get("value")
                    .cloned()
                    .ok_or_else(|| "params.emitted_header.value missing".to_string())?;
                Some((key.to_string(), value))
            }
        };
        Ok(EchoParams {
            emitted_target: Path::new(emitted_target_str),
            emitted_header,
        })
    }

    /// Spawn a single `EchoMockCell` task and return its inbox sender, join handle,
    /// and a oneshot peace_rx (Phase-13-E watcher pairing). The peace_tx is held
    /// inside the task as `_peace_keep` and dropped on natural task end, which
    /// signals the watcher to emit `CellDied` as in Phase 12.
    ///
    /// `consumes`: the cell's own pre-compiled required-`consumes` views
    /// (`contract.consumes`), forwarded to `cell_task` for the
    /// delivery-boundary consumes check (Slice 2, consumed in Task 2.4).
    ///
    /// `colony_inbox_tx`: GH #47 — this factory runs behind a REAL colony in
    /// hundreds of tests, so its cell must return the per-delivery `WorkDone`
    /// ticket; a silent one would hold every one of those drains to its
    /// deadline. `None` only where there is no colony at all.
    pub(crate) fn spawn_once(
        &self,
        path: Path,
        params: EchoParams,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        mailbox_capacity: usize,
        consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
        colony_inbox_tx: Option<tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>>,
    ) -> (
        tokio::sync::mpsc::Sender<meclaw_core::Message>,
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        use crate::mocks::EchoMockCell;
        use meclaw_colony::cell_task;
        use tokio::sync::mpsc;

        let (tx, rx) = mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
        // Backstop pair (P3-B-restart): never fired for this stateless test cell.
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
        let mut cell = EchoMockCell::new(path.clone()).emitted_target(params.emitted_target);
        if let Some((k, v)) = params.emitted_header {
            cell = cell.with_emitted_header(&k, v);
        }
        let join = tokio::spawn(async move {
            let _peace_keep = peace_tx;
            // Phase-13.5 A8: stateless echo factory does not use a blob store.
            cell_task(path, rx, outputs_tx, cell, None, consumes, colony_inbox_tx).await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for EchoCellFactory {
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        Self::parse_params_internal(raw).map(|_| ())
    }

    /// Test-only stateless factory — `cell_dir` is unused. Phase-13-G-2:
    /// stateless-style factory, the three new substrate params are ignored
    /// (idle/wake is a stateful concept).
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        raw: JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let params = Self::parse_params_internal(&raw)?;
        let (sender, join, peace_rx, backstop_rx) = self.spawn_once(
            path.clone(),
            params.clone(),
            outputs_tx.clone(),
            mailbox_capacity,
            contract.consumes.clone(),
            Some(colony_inbox_tx.clone()),
        );

        // RespawnFn captures Arc-clone of factory + parsed params + path + outputs_tx.
        // Restart calls spawn_once again — no re-parse, infallible by construction.
        let factory = self.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            factory.spawn_once(
                path.clone(),
                params.clone(),
                outputs_tx.clone(),
                mailbox_capacity,
                respawn_consumes.clone(),
                Some(colony_inbox_tx.clone()),
            )
        });
        // Phase-13.5 Lifecycle-3b Task 3: placeholder peace-stop ends. The
        // task is (re)spawned via the make_pair/spawn_once closure which keeps
        // its pre-3b shape; these ends are inert until Task 4 wires the
        // registry-side peace-stop. The colony drops them in Task 3.
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_minimal_with_emitted_target_only() {
        let raw = json!({"emitted_target": "/target"});
        let p = EchoCellFactory::parse_params_internal(&raw).unwrap();
        assert_eq!(p.emitted_target.as_str(), "/target");
        assert!(p.emitted_header.is_none());
    }

    #[test]
    fn parse_with_emitted_header_object() {
        let raw = json!({
            "emitted_target": "/target",
            "emitted_header": {"key": "via", "value": "/here"}
        });
        let p = EchoCellFactory::parse_params_internal(&raw).unwrap();
        assert_eq!(p.emitted_target.as_str(), "/target");
        let (k, v) = p.emitted_header.unwrap();
        assert_eq!(k, "via");
        assert_eq!(v, json!("/here"));
    }

    #[test]
    fn parse_missing_emitted_target_returns_error() {
        let raw = json!({});
        let err = EchoCellFactory::parse_params_internal(&raw).unwrap_err();
        assert!(err.contains("emitted_target"));
    }

    #[test]
    fn parse_emitted_header_missing_key_returns_error() {
        let raw = json!({
            "emitted_target": "/x",
            "emitted_header": {"value": 42}
        });
        let err = EchoCellFactory::parse_params_internal(&raw).unwrap_err();
        assert!(err.contains("emitted_header.key"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_once_returns_alive_sender() {
        use meclaw_core::MessageBuilder;
        use tokio::sync::mpsc;
        let (out_tx, _out_rx) = mpsc::channel(16);
        let factory = EchoCellFactory;
        let params = EchoParams {
            emitted_target: Path::new("/dst"),
            emitted_header: None,
        };
        let (sender, _join, _peace_rx, _backstop_rx) =
            factory.spawn_once(Path::new("/src"), params, out_tx, 1000, None, None);
        let msg = MessageBuilder::new(Path::new("/src")).build();
        sender.send(msg).await.expect("cell receives message");
    }

    #[test]
    fn validate_params_passes_for_valid_input() {
        let f = EchoCellFactory;
        f.validate_params(&json!({"emitted_target": "/x"})).unwrap();
    }

    #[test]
    fn validate_params_returns_error_on_missing_emitted_target() {
        let f = EchoCellFactory;
        assert!(f.validate_params(&json!({})).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_returns_error_on_invalid_params_no_spawn() {
        use tokio::sync::mpsc;
        let (out_tx, _out_rx) = mpsc::channel(16);
        let (inbox_tx, _inbox_rx) = mpsc::channel(16);
        let f: Arc<EchoCellFactory> = Arc::new(EchoCellFactory);
        let result = f.spawn_cell(
            Path::new("/src"),
            json!({}),
            out_tx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        );
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_returns_working_sender_with_valid_params() {
        use meclaw_core::MessageBuilder;
        use tokio::sync::mpsc;
        let (out_tx, _out_rx) = mpsc::channel(16);
        let (inbox_tx, _inbox_rx) = mpsc::channel(16);
        let f: Arc<EchoCellFactory> = Arc::new(EchoCellFactory);
        let spawned = f
            .spawn_cell(
                Path::new("/src"),
                json!({"emitted_target": "/dst"}),
                out_tx,
                std::path::PathBuf::new(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let msg = MessageBuilder::new(Path::new("/src")).build();
        let sender = match spawned {
            SpawnedCellKind::Active { sender, .. } => sender,
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();
    }
}
