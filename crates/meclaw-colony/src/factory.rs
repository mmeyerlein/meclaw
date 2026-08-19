//! Cell-Type-spezifische Spawn-Logik.
//!
//! **Parser invariant**: every factory MUST route `validate_params` and
//! `spawn_cell` through the same parse path (typically via a private helper).
//! That is the contract making the `.expect("validated in plan-phase")` calls in
//! `apply_bootstrap_plan` safe — if the two paths drift apart, the expect becomes
//! a boot-time bomb. Applies to all future cell types
//! ab Phase 7.

use crate::RespawnFn;
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Closure that wakes a parked stateful cell. Receives the parked mailbox
/// Receiver, spawns the cell-task + watcher internally, and RETURNS the woken
/// task's colony-initiated peace-stop wiring `(stop_tx, death_ack_rx)` so the
/// colony can store it in the `RegistryEntry` for a later disconnect.
///
/// **Phase-13.5 Lifecycle-3b Task 7.5**: the return shape closes a spec gap — a
/// woken stateful cell previously had `entry.stop_tx == None` (the WakeFn
/// discarded the live stop pair from `build_stateful_task_with_peace`), so
/// disconnecting it fell through to "just flip active=false" while the task kept
/// running and its mailbox remainder was never drained to the DLQ (overview
/// Z.1434/Z.1448). The colony now captures `(stop_tx, death_ack_rx)` at the wake
/// call-site and stores them, enabling a proper peace-stop on disconnect.
///
/// No-op WakeFns (stateless/long-running cells that never wake) return a
/// throwaway disconnected pair — they must never be invoked while `Awake`.
///
/// **Phase-13-G-1**: introduced at the type level; productive consumers arrive in
/// 13-G-3 (`RegistryEntry.wake`) and 13-K-2 (stateful factories return
/// `Dormant`).
pub type WakeFn = Box<
    dyn Fn(mpsc::Receiver<Message>) -> (oneshot::Sender<()>, oneshot::Receiver<()>) + Send + Sync,
>;

/// A view extracted from `config.json::contract` for cell spawn time.
///
/// Extended in Paket 7 (P13/D-010a) with `emits` and `validate_emits`.
/// Earlier fields (`tools`, `tags`, `is_collector`, `consumes`, `capabilities`)
/// are added only when a runtime consumer exists (CONTRIBUTING.md Regel 1).
#[derive(Debug, Clone, Default)]
pub struct ContractView {
    /// Whether this cell is allowed to emit multiple output messages per input.
    pub multi_send_capable: bool,
    /// Pre-compiled emits validators (P13/D-010a). `None` when the cell
    /// declares no `emits` block. Behind `Arc` because `ContractView` is cloned
    /// into RespawnFn closures (Phase-5 corridor) and `jsonschema::Validator`
    /// is not cheaply cloneable.
    pub emits: Option<std::sync::Arc<meclaw_core::CompiledEmits>>,
    /// Effective enforcement flag, resolved at spawn:
    /// `cfg!(debug_assertions) || colony.json strict_validation` (F1 Knopf-Kette).
    /// Set to `false` by default; the real value is resolved in B5 at spawn time.
    pub validate_emits: bool,
    /// Pre-compiled required-`consumes` views (Slice 2). `None` when the cell
    /// declares no required consume keys (check is vacuous). Behind `Arc` for
    /// the same RespawnFn-clone reason as `emits`.
    pub consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    /// GH #260 — `contract.write_surface`: whether the writes the substrate
    /// answers before `handle()` (the `transfer` slot's `import`) are bounded to
    /// this cell's own parent scope. `Open` by default, so a cell that says
    /// nothing keeps the behaviour it had.
    pub write_surface: meclaw_core::WriteSurface,
}

/// What a `CellFactory` returns for `spawn_cell`.
///
/// **Phase-13-G-2 (variant a)**: TODAY every factory returns `Active`. The
/// `Dormant` variant already exists at the type level — stateful factories switch
/// over to it in 13-K-2 together with the bootstrap-apply match.
///
/// `#[allow(clippy::large_enum_variant)]` (Paket-3 P3-B-restart): `Active` wuchs
/// by `backstop_rx` and sits just above the size-difference threshold. Boxing
/// would only move the allocation around (a short-lived spawn result, destructured
/// immediately) — the same trade-off as for `ColonyMsg`.
#[allow(clippy::large_enum_variant)]
pub enum SpawnedCellKind {
    /// The cell task is spawned immediately (phase-12 eager-spawn behaviour,
    /// peace_pair from 13-E-1). The caller sends `ColonyMsg::Register`.
    Active {
        /// Sender end of the cell's mailbox.
        sender: mpsc::Sender<Message>,
        /// Join handle for the spawned cell task.
        join: JoinHandle<()>,
        /// Phase-13-E: oneshot receiver paired with the cell-task's `peace_tx`.
        /// Explicit `peace_tx.send(())` (idle/sleep) → watcher exits silent.
        /// `peace_tx` dropped at task end → watcher emits `CellDied`.
        peace_rx: tokio::sync::oneshot::Receiver<()>,
        /// Phase-13.5 Lifecycle-3b Task 3 (F2): colony-initiated peace-stop
        /// trigger. Firing `stop_tx.send(())` makes the cell-task finish its
        /// in-flight `handle()`, fire `peace_tx`, return its mailbox via
        /// `ColonyMsg::Stopped`, close `cell.db`, then fire `death_ack`.
        stop_tx: tokio::sync::oneshot::Sender<()>,
        /// Phase-13.5 Lifecycle-3b Task 3 (F2): death-ack receiver, fired by the
        /// task's `TermAckGuard` **after** `cell.db` close (sqlite3_close).
        death_ack_rx: tokio::sync::oneshot::Receiver<()>,
        /// Paket-3 P3-B-restart: oneshot paired with the cell-task's `backstop`
        /// sender. Fired (then a clean `return`) when the `message_timeout`
        /// B-backstop elapses → the watcher classifies the death as
        /// `DeathKind::Backstop` (→ restart). Stateless/long-running cells never
        /// backstop-restart, but still return their (never-fired) `backstop_rx`
        /// so the watcher classifies their death as `Normal`/`Panic`.
        backstop_rx: tokio::sync::oneshot::Receiver<()>,
        /// Closure that produces a fresh (sender, join, peace_rx, backstop_rx) on
        /// supervisor restart.
        respawn: RespawnFn,
    },
    /// The cell is NotYetSpawned: the mailbox pair exists but no task runs.
    /// The caller sends `ColonyMsg::RegisterDormant` (arrives in 13-G-3).
    /// Implemented since 13-K-2; the bootstrap-apply path takes this branch in
    /// `bootstrap_apply.rs`.
    Dormant {
        /// Sender end of the cell's mailbox.
        sender: mpsc::Sender<Message>,
        /// Receiver end of the cell's mailbox — moves into the wake-spawned task.
        receiver: mpsc::Receiver<Message>,
        /// Closure that wakes the parked cell on first message.
        wake: WakeFn,
        /// Phase-13.5 Lifecycle-3b Task 3 (F2): colony-initiated peace-stop
        /// trigger for the (eventually) woken cell-task. See `Active::stop_tx`.
        stop_tx: tokio::sync::oneshot::Sender<()>,
        /// Phase-13.5 Lifecycle-3b Task 3 (F2): death-ack receiver, fired by the
        /// task's `TermAckGuard` after `cell.db` close. See `Active::death_ack_rx`.
        death_ack_rx: tokio::sync::oneshot::Receiver<()>,
        /// Closure that produces a fresh (sender, join, peace_rx) on supervisor restart.
        respawn: RespawnFn,
    },
}

/// Cell-Type factory trait. One implementation per `cell.type` string.
///
/// **Parser-Invariante**: implementors MUST route `validate_params` and
/// `spawn_cell` through the same parse path (typically a private helper like
/// `parse_params_internal`). The `.expect("validated in plan-phase")` calls in
/// `apply_bootstrap_plan` rely on this invariant — if the two diverge,
/// validation can pass and spawn can still fail, turning expect into a
/// boot-time panic.
pub trait CellFactory: Send + Sync {
    /// Pre-spawn validation. Used by the bootstrap plan-phase to surface
    /// per-cell-type param errors before any cell is spawned.
    fn validate_params(&self, params: &JsonValue) -> Result<(), String>;

    /// Pre-spawn validation of the cell's ON-DISK assets (issue #56).
    ///
    /// `validate_params` only sees the `params` block; a cell type whose
    /// directory carries further **statically parseable** configuration (the
    /// `store` cell's `seed/<table>.jsonl` files) overrides this hook so the
    /// bootstrap plan-phase parses that content too. Without it a purely
    /// syntactic mistake passes `meclaw --validate --validate-strict` with exit 0 and
    /// only surfaces on the first message — as a crash rather than a named
    /// error, breaking the validate-equals-spawn invariant.
    ///
    /// Called by `plan_bootstrap` right after `validate_params`, with the
    /// cell's absolute filesystem directory. Implementors MUST route this
    /// through the same parse path their spawn/wake code uses (parser
    /// invariant), and MUST NOT touch anything outside `cell_dir` or mutate
    /// state — the plan phase is side-effect free.
    ///
    /// Default: `Ok(())` — most cell types have no on-disk assets.
    fn validate_cell_dir(
        &self,
        params: &JsonValue,
        cell_dir: &std::path::Path,
    ) -> Result<(), String> {
        let _ = (params, cell_dir);
        Ok(())
    }

    /// Spawn-kind discriminator for registration paths that must know the kind
    /// WITHOUT building a task (F1-KH2: boot-inactive rehydration, subtree
    /// merge). `true` = lazy stateful kind — `spawn_cell` returns `Dormant`
    /// (mailbox pair + WakeFn, NO task) and the cell wakes on its first
    /// message. `false` (default) = eager kind — `spawn_cell` builds a running
    /// task (`Active`), so the side-effect-free registration paths must NOT
    /// call it and use `build_boot_inactive_respawn` instead.
    ///
    /// A lazy factory that forgets to override this is treated as eager and
    /// registers WITHOUT a wake mechanic: deliveries then dead-letter loudly
    /// (`cell_inactive`, defense layer) instead of being lost silently.
    fn is_lazy(&self) -> bool {
        false
    }

    /// Spawn the cell task. Re-parses params defensively (shares parse path
    /// with `validate_params`). Returns `SpawnedCellKind` with a `RespawnFn`
    /// that has already captured the *parsed* params — restart is unfallible.
    ///
    /// `cell_dir` is the absolute filesystem directory of this cell's directory
    /// under the colony tree root. Stateful cells join `cell.db` to derive their
    /// database path; stateless cells ignore.
    ///
    /// `contract` carries the cell's `config.json::contract` fields extracted
    /// at spawn time. Phase-11 consumers (e.g. `CodeCellFactory`) use
    /// `contract.multi_send_capable`; all other factories ignore it (`_contract`).
    ///
    /// `colony_inbox_tx`, `idle_timeout`, `cell_timeout` (Phase-13-G-1): the
    /// substrate params needed by stateful idle/wake (Phase-13-H/K). In
    /// Phase-13-G-2 they are passed through verbatim to `cell_task_stateful`
    /// for stateful factories (behavior-neutral when `idle_timeout=None`,
    /// `cell_timeout=0`); stateless/long-running factories ignore them.
    ///
    /// `blob_store` (Phase-13.5 A8): the colony's per-colony blob store, passed
    /// through to the spawned `cell_task*` so the cell-delivery boundary can
    /// resolve `Body::Blob` to an inline `Value` (spec Z.1363). `None` when no
    /// store is wired (some tests). Factories forward it verbatim to their
    /// `cell_task` / `build_stateful_task_with_peace` call.
    ///
    /// `message_timeout` (P3-B-plumb-1): the resolved B-backstop value
    /// (`cell.message_timeout`) for this cell. Stateful/stateless factories
    /// forward it to their spawn helper (`build_stateful_task_with_peace` /
    /// `build_stateless_task`) so the substrate can wrap the whole `handle()`
    /// call in a `tokio::time::timeout`; long-running factories drop it (LR has
    /// no backstop per spec). In THIS task the call-sites pass `None`
    /// (behavior-neutral — no backstop); a later task resolves the real value.
    ///
    /// `mailbox_capacity` (paket-1 T7): resolved bounded-mpsc capacity for this
    /// cell's mailbox (per-cell `cell.mailbox_size` override already resolved
    /// against `colony.json mailbox_default_capacity` at the call site).
    /// Factories use it for their `mpsc::channel(N)`.
    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<crate::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String>;

    /// Phase-13.5 Slice 4 T7: build ONLY the `RespawnFn` for a boot-inactive
    /// **eager** (Long-Running / Stateless) cell — WITHOUT spawning the initial
    /// task and WITHOUT building one (so boot-gating is preserved: no task runs
    /// at boot for an inactive cell).
    ///
    /// At boot a rehydrated cell whose persisted `status != 'active'` is NOT
    /// eager-spawned (see `apply_bootstrap_plan`). For an eager cell this hook
    /// lets the boot path register a REAL respawn closure (the SAME construction
    /// a normal eager cell's `RespawnFn` gets, captured from `cell_dir` / `params`
    /// / `colony_inbox_tx`) so that a later `add_edges` reconnect can call
    /// `(entry.respawn)()` and start the task IMMEDIATELY — no reboot needed.
    ///
    /// Returns `None` (the **default**) when the kind has no real respawn to hand
    /// out at this stage (lazy stateful cells wake on the first message; factories
    /// that have not opted in). In that case the boot path keeps the inert
    /// no-op closure and the reconnect arm falls back to wake-on-message.
    ///
    /// The arguments mirror `spawn_cell` so an implementation can share its
    /// `RespawnFn` construction between the two entry points.
    ///
    /// `mailbox_capacity` (paket-1 T20): resolved bounded-mpsc capacity for the
    /// respawned task's mailbox (per-cell `cell.mailbox_size` override already
    /// resolved against `colony.json mailbox_default_capacity` at the call site).
    /// Eager Long-Running factories use it for the respawned cell's
    /// `mpsc::channel(N)`; lazy / opted-out factories ignore it
    /// (`_mailbox_capacity`).
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<crate::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        // Default: no real respawn at boot-inactive stage → inert fallback.
        let _ = (
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
        );
        None
    }
}

/// Registry mapping `cell.type` strings to factory implementations.
pub type CellFactoryRegistry = HashMap<String, Arc<dyn CellFactory>>;

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    struct StubFactory;
    impl CellFactory for StubFactory {
        fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
            Ok(())
        }
        fn spawn_cell(
            self: Arc<Self>,
            _: Path,
            _: JsonValue,
            _: mpsc::Sender<CellEmission>,
            _cell_dir: std::path::PathBuf,
            _contract: ContractView,
            _colony_inbox_tx: mpsc::Sender<crate::ColonyMsg>,
            _idle_timeout: Option<std::time::Duration>,
            _cell_timeout: i64,
            _message_timeout: Option<std::time::Duration>,
            _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
            _mailbox_capacity: usize,
        ) -> Result<SpawnedCellKind, String> {
            unimplemented!("smoke test only checks trait object compiles")
        }
    }

    #[test]
    fn trait_object_arc_compiles_and_dispatches() {
        let factory: Arc<dyn CellFactory> = Arc::new(StubFactory);
        factory.validate_params(&json!({})).unwrap();
    }

    #[test]
    fn registry_alias_compiles() {
        let mut reg: CellFactoryRegistry = HashMap::new();
        reg.insert("stub".into(), Arc::new(StubFactory));
        assert!(reg.contains_key("stub"));
    }

    #[test]
    fn contract_view_default_is_false() {
        let cv = ContractView::default();
        assert!(!cv.multi_send_capable);
    }

    #[test]
    fn contract_view_default_has_no_emits_and_no_validation() {
        let cv = ContractView::default();
        assert!(cv.emits.is_none());
        assert!(!cv.validate_emits);
    }

    // Phase-13-G-1 type-shape tests.

    #[test]
    fn wake_fn_type_alias_compiles() {
        let _wake: crate::WakeFn = Box::new(|_rx| {
            let (s, _r) = tokio::sync::oneshot::channel::<()>();
            let (_s2, r2) = tokio::sync::oneshot::channel::<()>();
            (s, r2)
        });
    }

    #[tokio::test]
    async fn spawned_cell_kind_active_compiles() {
        let (sender, _rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        let join = tokio::spawn(async {});
        let (_p_tx, p_rx) = tokio::sync::oneshot::channel::<()>();
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_da_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        let (_bs_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let respawn: crate::RespawnFn = Box::new(|| unreachable!());
        let _kind = crate::SpawnedCellKind::Active {
            sender,
            join,
            peace_rx: p_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        };
    }

    #[test]
    fn spawned_cell_kind_dormant_compiles() {
        let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_da_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        let respawn: crate::RespawnFn = Box::new(|| unreachable!());
        let wake: crate::WakeFn = Box::new(|_| {
            let (s, _r) = tokio::sync::oneshot::channel::<()>();
            let (_s2, r2) = tokio::sync::oneshot::channel::<()>();
            (s, r2)
        });
        let _kind = crate::SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        };
    }
}
