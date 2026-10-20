//! GH #1253: a source cell's read-only view of the colony's shutdown drain.
//!
//! During the drain the colony refuses every new source emission (GH #47,
//! `DeadLetterReason::ShutdownDraining`). A cell that persists "done" before it
//! emits — the timer's completion mark, the Telegram cursor — lost exactly that
//! work for good: the mark said fired, the colony threw the strike away. The
//! view lets such a cell hold the work back instead, so the next start finds it
//! where a standstill would have left it.
//!
//! A `tokio::sync::watch` channel, not a lock (AGENTS.md concurrency model): the
//! colony task owns the only sender and flips it once, at drain entry; a cell
//! only reads the latest value. It reaches the cells on their
//! [`crate::ContractView`], put there by [`with_drain_view`], so no factory
//! gained a parameter.

use tokio::sync::watch;

/// Read-only view of whether the colony has entered its shutdown drain.
///
/// The default view never drains: a cell spawned outside a colony (unit tests,
/// hand-built topologies) keeps the behaviour it had.
#[derive(Debug, Clone)]
pub struct DrainView(watch::Receiver<bool>);

impl DrainView {
    /// A fresh pair: the sender the colony task flips at drain entry, and the
    /// view handed to cells.
    pub fn pair() -> (watch::Sender<bool>, Self) {
        let (tx, rx) = watch::channel(false);
        (tx, Self(rx))
    }

    /// Wrap a receiver subscribed from the colony's drain sender.
    pub fn from_receiver(rx: watch::Receiver<bool>) -> Self {
        Self(rx)
    }

    /// `true` once the colony has entered its shutdown drain. A view whose
    /// sender is gone keeps the last value it saw.
    pub fn is_draining(&self) -> bool {
        *self.0.borrow()
    }
}

impl Default for DrainView {
    fn default() -> Self {
        Self::pair().1
    }
}

/// GH #1253: a factory that hands every cell it spawns the colony's
/// [`DrainView`] on its `ContractView`, and forwards everything else to the
/// factory it wraps.
///
/// Why a wrapper and not a parameter: a cell is spawned from four sites
/// (bootstrap apply, boot-inactive respawn, the mutation path, the shutdown
/// teardown), and the mutation path alone threads its context through four
/// signatures of ~30 parameters each. The registry is the one thing all of
/// them share, so the colony task wraps its own registry once at start
/// ([`with_drain_view`]) and the boot apply wraps the registry it applies with,
/// using the view the colony hands back on `ColonyMsg::BeginInitialApply`.
/// Every trait method is forwarded explicitly, so the wrapped factory's own
/// overrides stay in force.
struct DrainAwareFactory {
    inner: std::sync::Arc<dyn crate::CellFactory>,
    drain: DrainView,
}

impl crate::CellFactory for DrainAwareFactory {
    fn validate_params(&self, params: &meclaw_core::JsonValue) -> Result<(), String> {
        self.inner.validate_params(params)
    }

    fn validate_declared_params(&self, declared: &meclaw_core::JsonValue) -> Result<(), String> {
        self.inner.validate_declared_params(declared)
    }

    fn connection_setup(&self) -> Option<crate::db_conn::ReopenSetup> {
        self.inner.connection_setup()
    }

    fn handover_wait(
        &self,
        backstop_ms: Option<u64>,
        params: &meclaw_core::JsonValue,
    ) -> Option<(&'static str, u64)> {
        self.inner.handover_wait(backstop_ms, params)
    }

    fn validate_cell_dir(
        &self,
        params: &meclaw_core::JsonValue,
        cell_dir: &std::path::Path,
    ) -> Result<(), String> {
        self.inner.validate_cell_dir(params, cell_dir)
    }

    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }

    fn is_lazy(&self) -> bool {
        self.inner.is_lazy()
    }

    fn owns_schema(&self) -> bool {
        self.inner.owns_schema()
    }

    fn default_idle_timeout_ms(&self) -> Option<u64> {
        self.inner.default_idle_timeout_ms()
    }

    fn spawn_cell(
        self: std::sync::Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        mut contract: crate::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<crate::SpawnedCellKind, String> {
        contract.drain = self.drain.clone();
        self.inner.clone().spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }

    fn build_boot_inactive_respawn(
        self: std::sync::Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        mut contract: crate::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<crate::RespawnFn> {
        contract.drain = self.drain.clone();
        self.inner.clone().build_boot_inactive_respawn(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

/// GH #1253: the same registry, every factory wrapped so the cells it spawns
/// read `drain` (see [`DrainAwareFactory`]).
pub fn with_drain_view(
    factories: crate::CellFactoryRegistry,
    drain: &DrainView,
) -> crate::CellFactoryRegistry {
    factories
        .into_iter()
        .map(|(cell_type, inner)| {
            let wrapped: std::sync::Arc<dyn crate::CellFactory> =
                std::sync::Arc::new(DrainAwareFactory {
                    inner,
                    drain: drain.clone(),
                });
            (cell_type, wrapped)
        })
        .collect()
}
