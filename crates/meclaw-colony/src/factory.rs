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
    /// GH #314 — `contract.transfer`: whether this cell's database answers the
    /// `transfer` slot at all. `All` by default, for the same reason.
    pub transfer: meclaw_core::TransferPolicy,
}

impl ContractView {
    /// The two declarations the substrate consults on the `transfer` slot, as
    /// the one value the spawn helpers carry (GH #260 + GH #314).
    ///
    /// Deliberately not derived from one another: `write_surface` says WHO may
    /// write, `transfer` says WHETHER the database answers the seam at all. A
    /// cell that wants both declares both.
    pub fn transfer_bounds(&self) -> meclaw_core::TransferBounds {
        meclaw_core::TransferBounds {
            write_surface: self.write_surface,
            policy: self.transfer,
        }
    }
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
    /// Default: `Ok(())` for a type that is seeded for — and a **refusal** for a
    /// type that owns its schema and therefore has no reader for a seed
    /// (GH #399).
    ///
    /// # Why the refusal lives in the default rather than in each type
    ///
    /// [`Self::owns_schema`] already obliges a type declaring `true` to load its
    /// own seed files, because the staging seeder stands down for exactly those
    /// types. A type that declares it and does **not** override this hook is
    /// therefore saying two things at once: "staging must not touch my
    /// database", and "I have no code that reads a seed". Put a
    /// `seed/anything.jsonl` beside such a cell and nothing writes it and
    /// nothing reads it — the file is silently ignored, and an operator who
    /// authored it waits for rows that will never appear. That is the same shape
    /// of quiet GH #398 was about, one step further out.
    ///
    /// Expressing it as the default is what makes it impossible to forget: a
    /// type that owns its schema AND loads its own seed overrides this hook to
    /// check that seed (the `web` cell is the model), and in overriding it takes
    /// the refusal off itself by the same act. A type that owns its schema and
    /// writes no loader gets the refusal for free, without anyone remembering to
    /// add it.
    ///
    /// Only `*.jsonl` counts. The seed vocabulary is JSONL, and refusing a
    /// `NOTES.md` or an editor backup would be a boot failure over litter.
    fn validate_cell_dir(
        &self,
        params: &JsonValue,
        cell_dir: &std::path::Path,
    ) -> Result<(), String> {
        let _ = params;
        if !self.owns_schema() {
            return Ok(());
        }
        let seed_dir = cell_dir.join("seed");
        let Ok(entries) = std::fs::read_dir(&seed_dir) else {
            return Ok(());
        };
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if found.is_empty() {
            return Ok(());
        }
        found.sort();
        Err(format!(
            "seed/{} cannot be loaded: a `{}` cell has its tables fixed in code, \
             so the mutation staging seeder deliberately keeps out of its \
             database (a seed header describes rows, not a schema) — and this \
             cell type has no seed loader of its own, so nothing else would read \
             the file either. Remove it, or seed this cell through the \
             operations its type offers.",
            found.join(", seed/"),
            self.type_name(),
        ))
    }

    /// The `cell.type` string this factory answers to.
    ///
    /// Used where a refusal has to name the type rather than describe it — the
    /// default [`Self::validate_cell_dir`] is the first such place. Defaults to
    /// the Rust type name, which is close enough to be useful and wrong enough
    /// to be worth overriding.
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
            .rsplit("::")
            .next()
            .unwrap_or("cell")
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

    /// Does this cell type own the SCHEMA of its `cell.db`? (GH #398)
    ///
    /// `false` (default) is the `store` shape: the tables are declared per
    /// instance, so whoever creates one from a `seed/<table>.jsonl` header
    /// creates the right one, and the mutation staging seeder
    /// (`mutation::stage::seed_cell_db_if_present`) may materialise a template's
    /// seed into the new cell's database at INSTANTIATION time, before the cell
    /// has ever been awake.
    ///
    /// `true` says the opposite: the tables are fixed in this type's own code,
    /// and a header line cannot describe them. A seed header carries column
    /// names and a coarse type and nothing else a schema means — no primary key,
    /// no `NOT NULL`, no default, no index, no column order. A type that
    /// declares `true` therefore keeps the staging seeder out of its database
    /// entirely: it creates its own tables and loads its own
    /// `seed/<table>.jsonl` at first spawn (`OpenStatus::Created`), which is the
    /// same path a cell instantiated from the filesystem at boot has always
    /// taken.
    ///
    /// The cost of getting this wrong was measured on the shipped `web` cell:
    /// staging built its `pages` table as `("root" TEXT, "route" TEXT, "title"
    /// TEXT)`, the cell's own `CREATE TABLE IF NOT EXISTS` found it standing and
    /// left it, and `page.set` — an upsert on `route` — was refused by SQLite
    /// for every display that had ever been grown by mutation.
    ///
    /// **Declaring `true` obliges the type to load its own seed files.** Nobody
    /// else will: the staging seeder is the only other reader, and it stands
    /// down for exactly the types that declare this.
    fn owns_schema(&self) -> bool {
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
