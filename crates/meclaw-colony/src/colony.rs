//! Colony task: sole write-authority over registry and routing.
//!
//! Phase 1 implements:
//! - Registry as task-local `HashMap<Path, RegistryEntry>` (no Mutex/RwLock).
//! - `tokio::select!` over (a) inbox, (b) outputs-from-cells channel,
//!   (c) supervisor death-events channel (added in Task 15).
//! - `ColonyMsg::Register`, `ColonyMsg::Route`, `ColonyMsg::Shutdown`.
//!
//! Edges, dead-letter cascade, and path resolution arrive in Phase 2.
//! Phase 1 routes purely on `message.target` with exact-match lookup.

use crate::dead_letter::{DeadLetter, DeadLetterReason};
use crate::edge_table::{Edge, EdgeDecision, EdgeTable, apply_edges};
use crate::hive_scope::{HiveScope, HiveScopeTable};
use meclaw_core::serde_json::{Map, Value};
#[cfg(debug_assertions)]
use meclaw_core::validate_ubf_body;
use meclaw_core::{ActorHandle, Body, CellEmission, Headers, Message, MessageBuilder, Path, Uuid};
use std::collections::HashMap;
use std::collections::VecDeque;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Default maximum number of *respawn attempts* after panic when no per-cell
/// override is configured. "5 Versuche" per spec = 5 respawns, i.e. up to
/// 6 cell instances total (1 original + 5 respawns).
/// Used as the `unwrap_or` fallback in `handle_register`.
const DEFAULT_RESTART_LIMIT: u32 = 5;

/// Phase-13.5 Lifecycle-3b Task 4 (F5-Variante-A): default wall-clock budget for
/// the inline death-ack-wait after a colony-initiated peace-stop during a
/// mutation, in milliseconds. A cell finishing its in-flight `handle()`, closing
/// `cell.db`, and firing `death_ack` is normally sub-millisecond; 5 s is the
/// generous backstop that lets a backpressured cell (stuck mid-`handle()` on a
/// full `outputs_tx`) time out cleanly into a `term_timeout` reject instead of
/// hanging the colony inbox loop. 5 s mirrors the project's other
/// graceful-termination budgets (Phase-10 watcher/abort join-timeouts).
const DEFAULT_TERM_TIMEOUT_MS: u64 = 5_000;

/// Phase-13.5 slice-4 T9c: runtime-overridable term-timeout budget (ms).
///
/// Default is [`DEFAULT_TERM_TIMEOUT_MS`] (5 s) — production reads this and never
/// mutates it, so production semantics are byte-identical to the former
/// `const TERM_TIMEOUT`. Integration tests that fire a real topology under heavy
/// `cargo`-parallel load can `set_term_timeout_ms` to a *generous* value (30 s)
/// so a legitimately-slow death-ack is not spuriously timed out by scheduler
/// oversubscription, or to a *short* value so the "timeout fires" test is fast
/// and deterministic. Both the override ([`set_term_timeout_ms_for_test`]) and
/// the read-side ([`term_timeout`]) are always compiled and behaviour-neutral at
/// the default.
static TERM_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(DEFAULT_TERM_TIMEOUT_MS);

/// Current term-timeout budget as a `Duration` (read-side, always compiled).
///
/// Reads [`TERM_TIMEOUT_MS`] with `Relaxed` ordering — the value is a single
/// scalar with no other shared state to synchronise against, and tests set it
/// once before driving the topology.
fn term_timeout() -> std::time::Duration {
    std::time::Duration::from_millis(TERM_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed))
}

/// Override the term-timeout budget — **for tests only** (hence the
/// `_for_test` suffix; production never calls this and keeps the 5 s default).
///
/// Tests call this before sending the disconnect/swap mutation that triggers a
/// death-ack-wait. Use a generous value (e.g. 30 000 ms) for "must not fire
/// under load" tests and a short value (e.g. 500 ms) for the "timeout fires"
/// test. Always compiled (not feature-gated) so integration tests run via a
/// plain `cargo test -p meclaw-colony` — which does not enable `test-hooks` —
/// can still make the death-ack-wait deterministic.
pub fn set_term_timeout_ms_for_test(ms: u64) {
    TERM_TIMEOUT_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// Canonical `error_code` for a mutation rejected because a disconnect's
/// death-ack-wait hit the [`term_timeout`] budget (F5-Variante-A). Pinned by a
/// unit test.
///
/// Spec: listed in the mutation-reject `error_code` enum
/// (`docs/meclaw-overview.md` § Validation). It is a mutation-reject code, not
/// a dead-letter reason, so it is deliberately absent from the dead-letter
/// `error_code` string list.
pub(crate) const TERM_TIMEOUT_ERROR_CODE: &str = "term_timeout";

/// Canonical `error_code` for a mutation rejected by the **permanent stop-wiring
/// guard**: a disconnect would deactivate a running (`Awake`) cell that has NO
/// live `stop_tx`. Such a disconnect cannot be peace-stopped, so honoring it
/// would leave the task running while the cell is marked inactive — a silent
/// "task ⇔ active" violation (zombie). Instead the whole mutation is rejected
/// atomically (edges rolled back, no durable write).
///
/// **Guard-stays (ruling 2026-06-07, P4-B2 closure): this guard STAYS as the
/// PERMANENT backstop — it is NOT interim and NOT unreachable.** Slice-4
/// stop-wiring restoration (`renotify_stop_wiring` on the reconnect-eager and
/// crash/backstop-restart paths) closes the COMMON case, but three classes of
/// genuinely-unwired `Awake` survivor remain and are the guard's standing duty:
///   (a) **long-running survivors** — an LR cell has NO `cell.message_timeout`
///       backstop (per spec it is 0/-1), so a `term_timeout`-consumed LR cell
///       cannot self-heal; the guard is its permanent backstop. Proved by
///       `second_disconnect_without_stop_wiring_is_rejected_not_zombie`.
///   (b) **`message_timeout = 0/-1` stateful survivors** — a stateful cell with
///       the backstop disabled likewise has no self-heal path.
///   (c) the tiny **eager-respawn ↔ `StopWiringRestored` race window**, and any
///       future spawn path that forgets to call `renotify_stop_wiring`.
/// (A stateful survivor WITH a finite backstop DOES self-heal — the backstop
/// fires → restart → `renotify_stop_wiring` restores `stop_tx` → a retry
/// disconnect commits, never reaching this guard. Proved by P4-B1
/// `stateful_survivor_heals_via_backstop_then_retry_disconnect_commits`.)
///
/// Spec: like `term_timeout`, listed in the mutation-reject `error_code` enum
/// (`docs/meclaw-overview.md` § Validation), not a dead-letter string.
pub(crate) const STOP_WIRING_UNAVAILABLE_ERROR_CODE: &str = "stop_wiring_unavailable";

/// Factory function that spawns a fresh cell task, returning its sender, join handle,
/// a oneshot `peace_rx` for the watcher (Phase-13-E: peace-aware supervision), and
/// a oneshot `backstop_rx` (Paket-3 P3-B-restart) so the watcher can classify a
/// `message_timeout` B-backstop death as `DeathKind::Backstop` (→ restart).
pub type RespawnFn = Box<
    dyn Fn() -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            tokio::sync::oneshot::Receiver<()>,
            tokio::sync::oneshot::Receiver<()>,
        ) + Send
        + Sync,
>;

/// Lifecycle status per stateful cell (Phase 13).
///
/// Stateless and long-running cells stay permanently `Awake`.
/// The parked `mailbox::Receiver` lives exclusively in the enum payload.
pub enum CellStatus {
    /// Cell-task is running; mailbox-Sender is in `entry.handle`.
    Awake,
    /// Cell self-despawned after idle-timeout or one-shot.
    Asleep {
        receiver: tokio::sync::mpsc::Receiver<meclaw_core::Message>,
    },
    /// Cell exists in FS-tree, not spawned since colony boot.
    NotYetSpawned {
        receiver: tokio::sync::mpsc::Receiver<meclaw_core::Message>,
    },
}

/// Outcome of a single `route()` call. Replaces the Phase-4-vintage
/// `Option<(Path, Message)>`-cascade-protocol with explicit enum variants.
///
/// Phase-13.5-A6: introduced as the sanctioned route()-break for
/// Cell→/colony-routing. The outputs-arm callsite holds the full
/// `colony_task`-state and dispatches `ColonyDispatch` directly, avoiding
/// the inbox_self_tx-self-send deadlock (`colony.rs:1604` note).
///
/// Illegal states are unrepresentable — no more `Option<>` ambiguity between
/// "nothing more to do" and "cascade further" (the CellStatus discipline from
/// phase 13).
pub(crate) enum RouteAction {
    /// Routing terminal — successful cell send, DLQ, TTL expired,
    /// or `/colony` bare endpoint-invalid.
    Done,
    /// Iterative cascade — corresponds to today's `Some((sender, msg))`.
    /// The outputs arm + inbox arm keep looping through `route_with_log`.
    Cascade { sender: Path, msg: Message },
    /// `/colony/<endpoint>` — the outputs arm holds the full state and calls
    /// `dispatch_colony_endpoint` directly. NO self-send, NO outputs_tx
    /// round-trip. `sender` is the `sender_path` from the `route()` call
    /// (= `em.sender_path` in the outputs arm, = ColonyMsg::Route's
    /// `sender_path` in the inbox arm), needed for (a) the T2 stub, which
    /// mirrors the pre-T2 `handle_colony_target` behaviour exactly, and (b) the
    /// DLQ reason sender for unknown endpoints in T3.
    ColonyDispatch {
        endpoint: Path,
        msg: Message,
        sender: Path,
    },
    /// Registry-miss + HiveScopeTable-hit: the resolved target is a hive
    /// scope-marker, not a cell. The hive has no actor/mailbox — Colony
    /// evaluates it as a logical transit node. `route()` only classifies and
    /// defers; the state-rich edge evaluation (`apply_edges` against the hive's
    /// out-edges, fan-out, `hive_no_route`-DLQ) lives in the caller, which holds
    /// `&edges`/`&mut dead_letters` (analog to `ColonyDispatch`).
    HiveTransit { hive_path: Path, msg: Message },
}

/// Entry stored per registered cell in Colony's task-local registry.
pub struct RegistryEntry {
    /// Handle to the cell's running task.
    pub handle: ActorHandle,
    /// Factory for restarting this cell.
    pub respawn: RespawnFn,
    /// Phase-13-G-3: closure that wakes a parked stateful cell (consumes the
    /// `NotYetSpawned`/`Asleep` mailbox-Receiver, spawns cell-task + watcher).
    ///
    /// F1-KH2 Schicht 2 (defense-in-depth): `None` = NO wake mechanic installed
    /// (eager kinds — re-spawned on reconnect, never woken — and exceptional
    /// fallbacks). A delivery that would need to wake a `None`-wake entry is
    /// dead-lettered LOUDLY (`cell_inactive`) and the parked status stays
    /// untouched. Structurally replaces the old inert closures, which dropped
    /// the parked receiver on invocation (silent loss + false `Awake`).
    pub wake: Option<crate::WakeFn>,
    /// Number of times this cell has been restarted by the supervisor.
    pub restart_count: u32,
    /// Per-cell restart ceiling, resolved at Register time from
    /// `CellHeader::restart_limit` with `DEFAULT_RESTART_LIMIT` as fallback.
    pub restart_limit: u32,
    /// Cell-ID (UUID v7) assigned at Register-time. Mirrors the `registry.cell_id`
    /// column; kept in-memory so `ColonyMsg::ReadRegistry` can answer without
    /// hitting the `read_conn` (Phase 12-B step-7.1).
    pub cell_id: Uuid,
    /// Cell-type string (from `config.json`), e.g. `"echo"`, `"llm"`.
    /// Mirrors the `registry.cell_type` column (Phase 12-B step-7.1).
    pub cell_type: String,
    /// Phase-13 lifecycle status. See `CellStatus` doc.
    pub status: CellStatus,
    /// Phase-13.5 Lifecycle-3b Task 7 (F6): eager-vs-lazy discriminator for the
    /// reconnect arm (recompute `false→true`). `true` for eager kinds
    /// (stateless / long-running, spawned via `SpawnedCellKind::Active`) — these
    /// re-spawn their task IMMEDIATELY on reconnect. `false` for lazy kinds
    /// (stateful, `SpawnedCellKind::Dormant`) — these only flip `active=true` and
    /// stay `NotYetSpawned` until the first message wakes them (wake-on-message).
    /// Set once at spawn time; MUST survive a disconnect (`handle_stopped` does
    /// not touch it).
    pub eager_on_reconnect: bool,
    /// Phase-13.5 Lifecycle-3b: edge-derived activity, persisted in
    /// `colony.db.registry.status`; orthogonal to `CellStatus`. `true` at every
    /// spawn (spawn = active); rehydration maps a persisted `status == "active"`
    /// to `true`, anything else to `false`.
    pub active: bool,
    /// `true` once the cell exhausted its restart limit (Paket 6 / P7). Invariante:
    /// `failed ⟹ !active`. A failed entry is RETAINED (No-Delete) but never spawned;
    /// only a direct reconnect clears it. Orthogonal to `CellStatus` (lifecycle) and
    /// `active` (edge-derived connectivity).
    pub failed: bool,
    /// Phase-13.5 Lifecycle-3b Task 4 (F2): colony-initiated peace-stop trigger
    /// for an Awake/Active cell's running task. Firing `stop_tx.send(())` makes
    /// the task finish its in-flight `handle()`, return its mailbox via
    /// `ColonyMsg::Stopped`, close `cell.db`, then fire `death_ack`. `take()`n at
    /// disconnect (single-use). `None` for cells without a live stop wiring
    /// (NotYetSpawned/Asleep before wake, or inert placeholder factories).
    pub stop_tx: Option<oneshot::Sender<()>>,
    /// Phase-13.5 Lifecycle-3b Task 4 (F2): death-ack receiver, fired by the
    /// task's `TermAckGuard` **after** `cell.db` close. `take()`n at disconnect
    /// and moved into the inline death-ack-wait (side-map). `None` mirrors
    /// `stop_tx == None`.
    pub death_ack_rx: Option<oneshot::Receiver<()>>,
}

/// Per-cell contract data the colony task keeps for substrate-side
/// validation: the 14-B header projection (Slice 1, mutation locality check)
/// plus the compiled emits + resolved flag (Slice 3, central emits check).
/// Populated at boot (`ColonyMsg::SetNodeContract`) and at mutation-spawn
/// time (`handle_mutation` inserts directly). Absent entry ⇒ vacuous checks.
#[derive(Debug, Clone)]
pub struct NodeContract {
    /// 14-B header projection (emits.hop keys + required consumes keys).
    pub header_view: crate::mutation::validate::HeaderNodeView,
    /// Compiled emits validators; `None` when the cell declares no `emits`.
    pub emits: Option<std::sync::Arc<meclaw_core::CompiledEmits>>,
    /// Effective emits-enforcement flag (resolve_validate_emits at spawn).
    pub validate_emits: bool,
}

/// How a cell-task ended, as classified by `spawn_watcher` from the join result
/// plus the `backstop` oneshot. Replaces the binary `was_panic: bool` so the
/// supervisor can express the three distinct death causes (Paket-3 P3-B-restart,
/// sanctioned `handle_cell_died` corridor break, 2026-06-07).
///
/// `handle_cell_died` restarts on `Panic` OR `Backstop` (one_for_one), removes
/// on `Normal`. The `message_timeout` B-backstop is the first legitimate
/// NON-panic death that must restart (spec § Timeouts B: "Task terminiert,
/// Supervisor restartet"); the binary `was_panic` world could not express it.
///
/// **AUDIT-PRE14-001 (panic priority)**: panic classification WINS over
/// backstop — a `handle()` that panics yields `Panic` regardless of whether the
/// backstop oneshot fired. The two are mutually exclusive per join result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathKind {
    /// The cell-task panicked (`JoinError::is_panic()`).
    Panic,
    /// The cell-task ended because its `cell.message_timeout` B-backstop
    /// elapsed (`cell_task_stateful` fired the `backstop` oneshot before its
    /// clean `return`). Restart-worthy (one_for_one).
    Backstop,
    /// The cell-task ended normally (clean `return` / mailbox-close), without a
    /// panic and without a backstop signal. Removed from the registry.
    Normal,
}

/// Messages accepted by the Colony task.
///
/// `Register`/`RegisterDormant` are the heavy spawn-control variants (they carry
/// the full spawn payload: boxed `RespawnFn`/`WakeFn`, the mailbox sender, the
/// join handle, and — since Phase-13.5 Lifecycle-3b Task 4 — the `stop_tx` /
/// `death_ack_rx` oneshots). They dominate `ColonyMsg`'s size, but boxing them
/// would only move the allocation while rippling through ~15 construction sites
/// of a substrate control-message enum that is sent over a bounded channel where
/// spawn traffic is rare relative to `Route`. The size asymmetry is accepted.
#[allow(clippy::large_enum_variant)]
pub enum ColonyMsg {
    /// Register a newly spawned cell under a path.
    Register {
        path: Path,
        sender: mpsc::Sender<Message>,
        join: JoinHandle<()>,
        /// Phase-13-E: peace-channel receiver for the watcher. An explicit `peace_tx.send(())`
        /// signals graceful sleep; dropping the sender (task exit) signals death.
        peace_rx: tokio::sync::oneshot::Receiver<()>,
        /// Phase-13.5 Lifecycle-3b Task 4 (F2): colony-initiated peace-stop
        /// trigger for the running cell-task. `None` for inert placeholder
        /// factories (no live stop wiring yet).
        stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
        /// Phase-13.5 Lifecycle-3b Task 4 (F2): death-ack receiver, fired by the
        /// task's `TermAckGuard` after `cell.db` close. Pairs with `stop_tx`.
        death_ack_rx: Option<tokio::sync::oneshot::Receiver<()>>,
        /// Paket-3 P3-B-restart: oneshot paired with the cell-task's `backstop`
        /// sender. Fired before the task's clean `return` when the
        /// `message_timeout` B-backstop elapses → watcher classifies
        /// `DeathKind::Backstop` (→ restart). Forwarded to `spawn_watcher`.
        backstop_rx: tokio::sync::oneshot::Receiver<()>,
        respawn: RespawnFn,
        /// Phase-13-G-3: wake-closure for stateful cells. Stateless/long-running
        /// callers pass `None` (status stays `Awake`, the cell is never woken;
        /// F1-KH2 Schicht 2 — a `None`-wake delivery dead-letters loudly).
        wake: Option<crate::WakeFn>,
        /// Per-cell restart ceiling. `None` → `DEFAULT_RESTART_LIMIT` (5).
        restart_limit: Option<u32>,
        /// UUID v7, assigned in plan_bootstrap (PlannedCell.cell_id) OR synthetically
        /// in test helpers (ColonyHandle::spawn). Stable across reboots.
        cell_id: Uuid,
        /// Cell-type string from config.json (PlannedCell.cell_type).
        cell_type: String,
        /// Phase-13.5 Lifecycle-3b: edge-derived activity for `RegistryEntry`.
        /// `true` for fresh spawns; rehydration passes the overlay-derived value.
        active: bool,
        ack: oneshot::Sender<()>,
    },
    /// Phase-13-G-3: register a stateful cell as `NotYetSpawned` (no task,
    /// no watcher — the mailbox pair moves into the `RegistryEntry.status` payload).
    /// Persist UpsertRegistry **identical** to `Register` (no lifecycle column
    /// in `colony.db.registry`). The consumer arrives in 13-K-2 (bootstrap apply
    /// branches per CellKind). No production caller today.
    RegisterDormant {
        path: Path,
        sender: mpsc::Sender<Message>,
        receiver: mpsc::Receiver<Message>,
        respawn: RespawnFn,
        /// `Some(real wake)` for lazy (Dormant-kind) cells — wake-on-message.
        /// `None` for boot-inactive EAGER cells (re-spawned on reconnect, never
        /// woken; F1-KH2 Schicht 2 — a `None`-wake delivery dead-letters loudly).
        wake: Option<crate::WakeFn>,
        restart_limit: Option<u32>,
        cell_id: Uuid,
        cell_type: String,
        /// Phase-13.5 Lifecycle-3b: edge-derived activity for `RegistryEntry`.
        /// `true` for fresh spawns; rehydration passes the overlay-derived value.
        active: bool,
        /// Paket-6 C: persisted failure flag for `RegistryEntry`. `true` iff the
        /// overlay's persisted `status == "failed"`; `false` for fresh spawns.
        /// Wins over edge-derived activity: a failed cell stays inactive and is
        /// never re-spawned across reboot, even when fully wired.
        failed: bool,
        /// Phase-13.5 Slice 4 T7: `eager_on_reconnect` for the `RegistryEntry`.
        /// `false` for lazy stateful cells (wake-on-message). `true` ONLY for a
        /// **boot-inactive eager** cell parked `NotYetSpawned` with a REAL
        /// `respawn` (`build_boot_inactive_respawn`): an `add_edges` reconnect
        /// then eager-re-spawns it immediately instead of waiting for a message.
        eager_on_reconnect: bool,
        ack: oneshot::Sender<()>,
    },
    /// Insert a new edge into the colony's edge table (used by bootstrap).
    AddEdge {
        id: Uuid,
        from: Path,
        to: Path,
        ack: oneshot::Sender<()>,
    },
    /// Register the per-cell contract data (boot path; mutation path inserts
    /// directly inside `handle_mutation`).
    SetNodeContract {
        path: Path,
        contract: NodeContract,
        ack: oneshot::Sender<()>,
    },
    /// GH #62: fill the `registry` provenance columns of an already-registered
    /// node from the `cell.provenance` block of its `config.json` (boot path).
    ///
    /// The mutation path writes the same columns directly through
    /// `ColonyWriteOp::SetRegistryProvenance` inside `handle_mutation`, where the
    /// staged provenance is already in hand. The boot path needs this hop
    /// because `apply_bootstrap_plan` speaks `ColonyMsg` only — it holds no
    /// writer handle. Sent AFTER the `Register`/`RegisterDormant` ack, so the
    /// row it updates exists.
    SetRegistryProvenance {
        path: Path,
        provenance: crate::config::NodeProvenance,
        ack: oneshot::Sender<()>,
    },
    /// Route a message to the registered cell at `msg.target`.
    ///
    /// `sender_path` is the originator of the message: an external/test sender
    /// typically passes `Path::new("/")` (root); the colony itself, when routing
    /// a `CellEmission`, passes `em.sender_path`.
    /// Used for relative-target resolution (`./x`, `../x`) per spec § Path addressing.
    Route { sender_path: Path, msg: Message },
    /// Graceful shutdown: sends ack then exits the loop.
    Shutdown { ack: oneshot::Sender<()> },
    /// Emitted by a watcher task when a cell's JoinHandle resolves.
    CellDied {
        /// Path of the cell whose task ended.
        path: Path,
        /// How the task ended (`Panic` / `Backstop` / `Normal`). `handle_cell_died`
        /// restarts on `Panic`/`Backstop`, removes on `Normal`.
        death_kind: DeathKind,
    },
    /// Register a hive-scope marker (path-prefix only, no actor).
    AddHiveScope {
        path: Path,
        ack: oneshot::Sender<()>,
    },
    /// Issue #7: a long-running cell's I/O sub-task reporting on itself.
    ///
    /// Sent with `try_send` from the I/O task (see
    /// [`crate::io_liveness::IoLivenessMark`]), so it can never backpressure the
    /// very task whose stall it is meant to expose. The colony owns the
    /// resulting map — no lock, no shared state.
    IoLiveness {
        /// Cell whose I/O sub-task this is.
        path: Path,
        /// `Some(t)`: a successful external round trip completed at `t`.
        /// `None`: this I/O task has started and has no round trip yet (sent
        /// once at `run_io` entry; it also clears a predecessor's mark after a
        /// restart).
        at: Option<std::time::SystemTime>,
    },
    /// Issue #7: read the per-I/O-task liveness marks (in-memory, no DB).
    /// Answers `GET /health`.
    ReadLiveness {
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadLivenessReply>,
    },
    /// **Phase-2 test hook** for inspecting the dead-letter queue.
    ///
    /// This is NOT the final design. The spec-symmetric read path is a `Message`
    /// to `/colony/dead_letters` with `reply_to` set; that requires the UBF header
    /// (Phase 3) and is fully realised once the HTTP-API layer arrives (Phase 12).
    /// Tests in Phase 2 use this variant to assert that unroutable messages landed
    /// in the queue with the expected reason.
    DrainDeadLetters {
        ack: oneshot::Sender<Vec<DeadLetter>>,
    },
    /// Phase-13.5 A8: a cell-delivery-boundary blob resolution failed. The
    /// `cell_task` cannot push to the colony-owned dead-letter `VecDeque`
    /// directly, so it forwards the undeliverable message here; the colony arm
    /// records it with the given reason (`blob_unavailable`). Own inbox arm —
    /// `route()`/`handle_cell_died` untouched.
    DeadLetterMessage {
        /// The message whose `Body::Blob` could not be resolved.
        message: Message,
        /// Why it is undeliverable (`DeadLetterReason::BlobUnavailable`).
        reason: DeadLetterReason,
    },
    /// First-boot atomic apply (FIX 3, E9): persists edges + hive_scopes in ONE
    /// transaction AND enters them into the in-memory tables.
    /// `apply_bootstrap_plan` sends this after all registers.
    /// op-before-ack: `colony_db.send_op(InitialApply)` runs BEFORE `ack.send()`.
    InitialApply {
        /// Edges from the bootstrap plan.
        edges: Vec<crate::bootstrap::PlannedEdge>,
        /// Hive-scope paths from the bootstrap plan.
        hive_scopes: Vec<Path>,
        /// Ack channel — fires AFTER send_op.
        ack: oneshot::Sender<()>,
    },
    /// Bootstrap recovery (run-5/5b finding): `apply_bootstrap_plan` sends this
    /// BEFORE the first cell spawn. On the FirstBoot path the arm writes the
    /// durable `bootstrap_in_flight` marker (colony.db `meta`) and only acks
    /// AFTER the writer commit — a crash anywhere in the apply leaves the marker
    /// behind and the following boot classifies as a resumable `FirstBoot`
    /// instead of `Inconsistent`. The clear runs atomically inside the
    /// `InitialApply` transaction. On the reboot path the arm is a no-op (all
    /// tables full — a reboot apply crash is classification-neutral).
    BeginInitialApply {
        /// Ack channel — fires after the durable marker commit (FirstBoot)
        /// resp. immediately (reboot).
        ack: oneshot::Sender<()>,
    },
    /// Phase 11 slice 11-E: internally triggers the same path as the boot scan.
    /// The CLI flag `--rescan-templates` and (phase 12) an HTTP POST send this message.
    RescanTemplates {
        templates_root: std::path::PathBuf,
        ack: oneshot::Sender<()>,
    },
    /// Phase 12-B step-7.6: trace read with `spawn_blocking` + a fresh
    /// `SQLITE_OPEN_READ_ONLY` connection on `colony.db`. WAL allows
    /// concurrent readers; the writer thread stays unaffected.
    ///
    /// **Honest warning**: stalls the colony inbox loop for the query duration
    /// (bounded by `limit ≤ 1000` rows). Off-loop reads (a dedicated read task)
    /// are phase 14.
    ReadTrace {
        /// Optional: filter by trace_id (UUID).
        trace_id: Option<Uuid>,
        /// Optional: filter by to_path prefix.
        path_prefix: Option<Path>,
        /// Optional: filter by correlation_id (UUID).
        correlation_id: Option<Uuid>,
        /// If `true`, only return rows whose `headers` JSON contains `"error_code"`.
        only_error: bool,
        /// Optional: only rows with `created_at >= since` (Unix seconds).
        since: Option<i64>,
        /// Hard cap on returned entries; clamped to `1..=1000` in the arm.
        limit: usize,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadTraceReply>,
    },
    /// P1 (message browser): paginated, filtered read over
    /// `colony.db::message_log`. Mirrors `ReadTrace` — `spawn_blocking` + a
    /// fresh `SQLITE_OPEN_READ_ONLY` connection; the entire logic lives in
    /// `colony_dispatch::handle_read_messages`.
    ///
    /// **Honest warning**: stalls the colony inbox loop for the query duration.
    /// Bounded by `filter.scan_budget` (≤ 50_000 rows read), not by `limit`
    /// alone. Off-loop reads are post-v0.1.0 (`docs/roadmap.md`).
    ReadMessages {
        /// Filter + paging cursor; all caps are clamped in the dispatch helper.
        filter: crate::api_dto::MessageLogFilter,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadMessagesReply>,
    },
    /// Phase 12-B step-7.5: scope-filtered graph snapshot (nodes + edges).
    /// Nodes = registry entries whose path starts with `scope`.
    /// Edges = `EdgeTable` entries whose `from` AND `to` lie inside the scope.
    ReadGraph {
        /// Scope path prefix (e.g. `/main` → matches `/main`, `/main/x`, ...).
        scope: Path,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadGraphReply>,
    },
    /// Phase 12-B step-7.4: read-only audit view on `colony.db::mutation_log`.
    /// Sync read via `colony_db.read_mutation_log()`.
    ReadMutationsAudit {
        /// Optional: only rows with `created_at >= since` (unix seconds).
        since: Option<i64>,
        /// Hard cap on returned entries; clamped to `1..=1000` in the arm.
        limit: usize,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadMutationsAuditReply>,
    },
    /// Phase 12-B step-7.3: read-only snapshot of `colony.db::templates`.
    /// Uses the existing sync `colony_db.read_templates()` (no `.await`).
    ReadTemplates {
        /// Optional exact-match on the cell-type from the template's `config.json`.
        /// **Currently no-op** — see `TemplateEntryDto` for the Phase-14 backlog.
        cell_type: Option<String>,
        /// Optional exact-match on `template.json::name`.
        name: Option<String>,
        /// Hard cap on returned entries; clamped to `1..=1000` in the arm.
        limit: usize,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadTemplatesReply>,
    },
    /// Phase 12-B step-7.2: pure-Read of the in-memory dead-letter queue
    /// (does NOT drain — use `DrainDeadLetters` for the DELETE-path).
    /// HTTP `GET /colony/dead_letters` in Task 8 uses this.
    ReadDeadLetters {
        /// Optional since-timestamp filter. **Currently no-op** — see
        /// `DeadLetterDto` doc-comment for the Phase-14 backlog item.
        since: Option<i64>,
        /// Optional exact-match on canonical error_code string.
        error_code: Option<String>,
        /// Hard cap on returned entries; clamped to `1..=1000` in the arm.
        limit: usize,
        /// Reply channel; dropped on Shutdown-drain.
        ack: oneshot::Sender<crate::api_dto::ReadDeadLettersReply>,
    },
    /// Phase 12-B step-7.1: read-only registry snapshot with optional filters.
    /// HTTP-handlers in Task 8 marshal `ReadRegistryReply` to JSON. Inbox-arm
    /// builds the reply from the in-memory `registry` HashMap (no DB hit).
    ReadRegistry {
        /// Optional exact-match path filter.
        path: Option<Path>,
        /// Optional prefix-match filter on the path string repr.
        path_prefix: Option<Path>,
        /// Optional exact-match cell_type filter.
        cell_type: Option<String>,
        /// Phase-13.5 Lifecycle-3b T8 (F7): optional `active` filter.
        /// `Some(true)` keeps only active entries, `Some(false)` only inactive,
        /// `None` keeps all. Symmetric with the cell→/colony read path.
        active: Option<bool>,
        /// Hard cap on returned entries; clamped to `1..=1000` in the arm.
        limit: usize,
        /// Reply channel; dropped on Shutdown-drain (Read is best-effort).
        ack: oneshot::Sender<crate::api_dto::ReadRegistryReply>,
    },
    /// Cell-task self-despawned (idle-timeout or one-shot). Sleep-arm
    /// in colony_task parks the receiver or re-spawns if a race occurred.
    /// Full impl in 13-H-3.
    Sleep {
        path: Path,
        receiver: mpsc::Receiver<Message>,
    },
    /// GH #18: a cell task is dying (panic, I/O-end abort, B-backstop) with
    /// messages still buffered in its mailbox. Its `MailboxGuard` drained them
    /// on the way out and hands them here.
    ///
    /// Distinct from `Stopped`/`Sleep`, which return the whole `Receiver` on a
    /// deliberate, peaceful exit: a death cannot hand over a receiver (it is
    /// unwound or aborted with the task), so the messages travel on their own.
    /// The colony holds them until the matching `CellDied` has been handled and
    /// then delivers them to the successor — or dead-letters them if the death
    /// left no successor. Ordering is what makes that work: the guard's
    /// `try_send` runs while the task is being dropped, i.e. strictly before
    /// the join handle resolves and the watcher can send `CellDied`.
    MailboxRescued {
        /// Path of the dying cell.
        path: Path,
        /// The unread remainder, oldest first.
        messages: Vec<Message>,
    },
    /// Phase-13.5 Lifecycle-3b Task 3 (F2): a cell-task has finished its
    /// colony-initiated **peace-stop** (Disconnect, NOT idle) and is returning
    /// its mailbox `Receiver` so the remainder can be drained to the DLQ.
    ///
    /// Distinct from `Sleep`: `Sleep` means "idle, may wake on next message";
    /// `Stopped` means "Disconnected, no further processing — remainder →
    /// DLQ `cell_inactive`". The watcher saw the explicit `peace_tx.send(())`
    /// and exited silently → no `CellDied`, `handle_cell_died` never fires.
    ///
    /// Handled by `handle_stopped` (A3): drains the returned mailbox remainder
    /// to the DLQ as `cell_inactive`, then swaps in a fresh channel pair and
    /// parks the entry as `NotYetSpawned` (`active = false`) so a later
    /// reconnect-wake hits a live sender.
    Stopped {
        path: Path,
        receiver: mpsc::Receiver<Message>,
    },
    /// Phase-13.5 Slice 4 T5: restore a cell's colony-initiated stop-wiring.
    ///
    /// A disconnect `take()`s a cell's `RegistryEntry.stop_tx` /
    /// `death_ack_rx` (the persistent home of its stop-wiring), leaving them
    /// `None`. When a reconnect/restart re-spawns a live cell-task, the spawner
    /// (T6/T7) sends this so the colony-task can put the fresh `(stop_tx,
    /// death_ack_rx)` pair back onto those two `RegistryEntry` fields — restoring
    /// the colony's ability to peace-stop the cell on a later disconnect.
    ///
    /// Handled by `handle_stop_wiring_restored`. Like the heavy spawn-control
    /// variants this is deliberately NOT boxed (see the enum doc-comment): a
    /// two-oneshot payload is small.
    StopWiringRestored {
        /// Path of the cell whose stop-wiring is being restored.
        path: Path,
        /// Fresh colony-initiated peace-stop trigger for the running cell-task.
        stop_tx: tokio::sync::oneshot::Sender<()>,
        /// Fresh death-ack receiver, fired by the task's `TermAckGuard` after
        /// `cell.db` close. Pairs with `stop_tx`.
        death_ack_rx: tokio::sync::oneshot::Receiver<()>,
    },
    /// Phase 6: a mutation request. The arm handles it via inline skeleton in T12;
    /// T13+ extracts the real logic into `handle_mutation`.
    Mutation {
        /// Raw payload of the mutation message body (diff + ctx + scope).
        payload: meclaw_core::JsonValue,
        /// Reply target for EDA error replies on validation fail.
        reply_to: Option<meclaw_core::Path>,
        /// Trace header fields, needed for the error-reply envelope.
        trace_id: meclaw_core::Uuid,
        parent_message_id: meclaw_core::Uuid,
        /// Ack — fires AFTER committed-UPDATE (or AFTER validate-fail reply).
        ack: tokio::sync::oneshot::Sender<crate::mutation::MutationOutcome>,
    },
}

/// Fire-and-forget watcher. Awaits `peace_rx` — explicit peace → exit silent,
/// no `CellDied`. `peace_tx` dropped without send → cell-task exited normally
/// or panicked; emit `CellDied` via `cell_join` inspection.
///
/// One task per Awake cell, no leak.
///
/// **Phase-13-K-2**: `pub` so Stateful-Factory `WakeFn`-Closures (in
/// `meclaw-cells` and `meclaw-testing`) can register the cell-task with the
/// supervisor identical to `handle_register`/`handle_cell_died`/Mutation-Spawn.
pub fn spawn_watcher(
    inbox_self_tx: &mpsc::Sender<ColonyMsg>,
    path: Path,
    mut cell_join: JoinHandle<()>,
    peace_rx: tokio::sync::oneshot::Receiver<()>,
    mut backstop_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let death_tx = inbox_self_tx.clone();
    tokio::spawn(async move {
        match peace_rx.await {
            Ok(()) => {
                drop(cell_join);
            }
            Err(_) => {
                let r = (&mut cell_join).await;
                // AUDIT-PRE14-001: panic classification has PRIORITY over the
                // backstop signal — a panicking handle yields `Panic` regardless
                // of whether the backstop oneshot fired. They are mutually
                // exclusive per join result.
                let death_kind = match r {
                    Err(e) if e.is_panic() => DeathKind::Panic,
                    _ => {
                        if backstop_rx.try_recv().is_ok() {
                            DeathKind::Backstop
                        } else {
                            DeathKind::Normal
                        }
                    }
                };
                let _ = death_tx
                    .send(ColonyMsg::CellDied { path, death_kind })
                    .await;
            }
        }
    });
}

/// GH #62: enqueue the UPDATE-only provenance fill for an already-registered
/// path.
///
/// Fire-and-forget like every other registry-column write: the channel is FIFO,
/// so the `UpsertRegistry` that created the row is always ahead of this op, and
/// a lost provenance index is a lost index, not lost truth — the authoritative
/// record is `cell.provenance` in the node's own `config.json`.
///
/// Takes the raw sender + queue-depth counter rather than `&ColonyDb`, for the
/// usual reason: `&ColonyDb` is !Send (rusqlite::Connection is !Sync) and must
/// not live across an `.await` inside `colony_task`.
async fn send_registry_provenance(
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    queue_depth: &std::sync::Arc<std::sync::atomic::AtomicI64>,
    path: Path,
    provenance: crate::config::NodeProvenance,
) {
    queue_depth.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = writer_tx
        .send(crate::persist::writer::ColonyWriteOp::SetRegistryProvenance { path, provenance })
        .await;
}

/// Handle a `ColonyMsg::Register` message: insert the cell into the registry,
/// enqueue the UpsertRegistry write-op (op-before-ack invariante, T22),
/// and spawn a watcher that reports its death back into the colony inbox.
#[allow(clippy::too_many_arguments)]
async fn handle_register(
    registry: &mut HashMap<Path, RegistryEntry>,
    inbox_self_tx: &mpsc::Sender<ColonyMsg>,
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    queue_depth: &std::sync::Arc<std::sync::atomic::AtomicI64>,
    path: Path,
    sender: mpsc::Sender<Message>,
    join: JoinHandle<()>,
    peace_rx: tokio::sync::oneshot::Receiver<()>,
    backstop_rx: tokio::sync::oneshot::Receiver<()>,
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    death_ack_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    respawn: RespawnFn,
    wake: Option<crate::WakeFn>,
    restart_limit: Option<u32>,
    cell_id: Uuid,
    cell_type: String,
    active: bool,
    ack: oneshot::Sender<()>,
) {
    // 1. In-memory insert.
    let handle = ActorHandle::new(path.clone(), sender);
    let entry = RegistryEntry {
        handle,
        respawn,
        wake,
        restart_count: 0,
        restart_limit: restart_limit.unwrap_or(DEFAULT_RESTART_LIMIT),
        cell_id,
        cell_type: cell_type.clone(),
        status: CellStatus::Awake,
        // Active = eager kind (stateless/long-running) → eager re-spawn on reconnect.
        eager_on_reconnect: true,
        active,
        failed: false,
        stop_tx,
        death_ack_rx,
    };
    registry.insert(path.clone(), entry);

    // 2. Enqueue the writer op — op-before-ack invariant (T22).
    // Direct send via `writer_tx`+`queue_depth` (NOT via `&ColonyDb.send_op`,
    // because `&ColonyDb` is !Send — rusqlite::Connection is !Sync, so the
    // borrow must not live across `.await`).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_secs() as i64;
    queue_depth.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let depth = queue_depth.load(std::sync::atomic::Ordering::Relaxed);
    if depth > 1000 {
        tracing::warn!(depth, "colony.db writer backlog > 1000");
    }
    writer_tx
        .send(crate::persist::writer::ColonyWriteOp::UpsertRegistry {
            path: path.clone(),
            cell_id: cell_id.to_string(),
            cell_type,
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("writer thread dead");

    // 3. Ack — the caller knows: the op is in the channel.
    let _ = ack.send(());

    // 4. Spawn watcher.
    spawn_watcher(inbox_self_tx, path, join, peace_rx, backstop_rx);
}

/// Phase-13-G-3: Handle a `ColonyMsg::RegisterDormant` message: insert the
/// cell as `NotYetSpawned` into the registry (mailbox-Receiver parked in the
/// status payload, no cell-task spawned, no watcher), enqueue the
/// UpsertRegistry write-op (**identical** to `handle_register` — no
/// lifecycle column in `colony.db.registry`), then ack.
#[allow(clippy::too_many_arguments)]
async fn handle_register_dormant(
    registry: &mut HashMap<Path, RegistryEntry>,
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    queue_depth: &std::sync::Arc<std::sync::atomic::AtomicI64>,
    path: Path,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
    respawn: RespawnFn,
    wake: Option<crate::WakeFn>,
    restart_limit: Option<u32>,
    cell_id: Uuid,
    cell_type: String,
    active: bool,
    failed: bool,
    eager_on_reconnect: bool,
    ack: oneshot::Sender<()>,
) {
    // 1. In-memory insert with parked mailbox-Receiver.
    let handle = ActorHandle::new(path.clone(), sender);
    let entry = RegistryEntry {
        handle,
        respawn,
        wake,
        restart_count: 0,
        restart_limit: restart_limit.unwrap_or(DEFAULT_RESTART_LIMIT),
        cell_id,
        cell_type: cell_type.clone(),
        status: CellStatus::NotYetSpawned { receiver },
        // Lazy stateful → `false` (wake-on-message). Boot-inactive eager passes
        // `true` (Phase-13.5 Slice 4 T7): a real `respawn` is registered so the
        // reconnect arm can eager-re-spawn it immediately.
        eager_on_reconnect,
        active,
        failed,
        // Dormant cells have no running task → no live stop wiring. A wake
        // (Phase-13-I) spawns a fresh task with its own stop pair; before wake a
        // disconnect just flips `active=false` (no death_ack-wait, Task-4.2).
        stop_tx: None,
        death_ack_rx: None,
    };
    registry.insert(path.clone(), entry);

    // 2. Enqueue the writer op — IDENTICAL to `handle_register`. No
    // lifecycle field; lifecycle_status exists only in-memory + in the DTO.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_secs() as i64;
    queue_depth.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let depth = queue_depth.load(std::sync::atomic::Ordering::Relaxed);
    if depth > 1000 {
        tracing::warn!(depth, "colony.db writer backlog > 1000");
    }
    writer_tx
        .send(crate::persist::writer::ColonyWriteOp::UpsertRegistry {
            path: path.clone(),
            cell_id: cell_id.to_string(),
            cell_type,
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("writer thread dead");

    // 3. Ack — the caller knows: the op is in the channel.
    let _ = ack.send(());

    // NO spawn_watcher — no cell-task running yet (wake-on-first-message
    // path lives in Phase-13-I).
}

/// Sleep-arm handler. Extracted for unit-testing (race-coverage in 13-J).
///
/// Race logic: `receiver.len() > 0` = a message arrived in the despawn
/// window → re-spawn via `WakeFn` instead of parking. `receiver.len() == 0`
/// → `status = Asleep { receiver }`. Ghost-path (entry already removed via
/// mutation race) silently returns; the receiver is dropped on return.
async fn handle_sleep(
    registry: &mut HashMap<Path, RegistryEntry>,
    dead_letters: &mut VecDeque<DeadLetter>,
    inbox_self_tx: &mpsc::Sender<ColonyMsg>,
    path: Path,
    mut receiver: mpsc::Receiver<Message>,
) {
    let Some(entry) = registry.get_mut(&path) else {
        // Ghost-path: the registry entry is gone. L-1 model invariant: under the
        // No-Delete model this is UNREACHABLE for a Sleep message — emitting Sleep
        // always fires `peace_tx` FIRST (cell_task), so the watcher exits silently
        // (no `CellDied` → no `registry.remove`). The sole `registry.remove` site
        // (handle_cell_died `DeathKind::Normal`) requires peace NOT fired, so it is
        // mutually exclusive with having emitted Sleep; `remove_nodes`/`swap_nodes`
        // are disconnect-instead-of-delete and never remove the entry. The parked
        // receiver is therefore expected EMPTY.
        debug_assert!(
            receiver.is_empty(),
            "L-1: Sleep ghost-arm hit a non-empty parked receiver for removed path \
             {} — a removal path now races Sleep emission; re-audit the peace/CellDied \
             mutual exclusion",
            path.as_str()
        );
        // Defensive drain against future model drift: order-preserved → DLQ as
        // `cell_inactive` (mirrors `handle_stopped`). In the model-holds case this
        // is a no-op.
        receiver.close();
        let mut drained = 0usize;
        while let Ok(msg) = receiver.try_recv() {
            let sender_path = msg.reply_to.clone().unwrap_or_else(|| Path::new("/"));
            push_dead_letter(
                dead_letters,
                DeadLetter {
                    sender_path,
                    original_target: msg.target.clone(),
                    resolved_target: path.clone(),
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::CellInactive,
                },
            );
            drained += 1;
        }
        if drained > 0 {
            tracing::warn!(
                path = path.as_str(),
                drained,
                "Sleep ghost-arm drained a non-empty parked receiver to the DLQ \
                 (cell_inactive) — unexpected under the No-Delete model (L-1)"
            );
        }
        return;
    };
    if !receiver.is_empty() {
        // Race: message buffered between `is_empty()`-check in `cell_task_stateful`
        // and this arm. `WakeFn` re-spawns synchronously (no `.await` between
        // status set and send).
        //
        // Phase-13.5 Lifecycle-3b Task 7.5: store the woken task's live
        // `(stop_tx, death_ack_rx)` so a later disconnect can peace-stop it +
        // drain its mailbox remainder to the DLQ.
        if let Some(wake) = entry.wake.as_ref() {
            let (stop_tx, death_ack_rx) = wake(receiver);
            entry.stop_tx = Some(stop_tx);
            entry.death_ack_rx = Some(death_ack_rx);
            entry.status = CellStatus::Awake;
        } else {
            // F1-KH2 Schicht 2 (defense-in-depth): no wake mechanic — only a
            // RUNNING stateful cell can emit Sleep, and running stateful cells
            // carry a real wake, so this arm is unreachable today. Should the
            // model drift: drain the buffered remainder LOUDLY to the DLQ
            // (`cell_inactive`, order preserved — mirrors the ghost-arm) and
            // park; never drop messages silently.
            tracing::error!(
                path = path.as_str(),
                "Sleep with a non-empty receiver on an entry without a wake \
                 mechanic — draining to the DLQ (cell_inactive)"
            );
            receiver.close();
            while let Ok(msg) = receiver.try_recv() {
                let sender_path = msg.reply_to.clone().unwrap_or_else(|| Path::new("/"));
                push_dead_letter(
                    dead_letters,
                    DeadLetter {
                        sender_path,
                        original_target: msg.target.clone(),
                        resolved_target: path.clone(),
                        message: msg,
                        reason: crate::dead_letter::DeadLetterReason::CellInactive,
                    },
                );
            }
            entry.status = CellStatus::Asleep { receiver };
        }
    } else {
        entry.status = CellStatus::Asleep { receiver };
    }
    let _ = inbox_self_tx; // kept for symmetry with `handle_register`; future uses
}

/// Mailbox capacity for the fresh channel pair swapped in at disconnect (A3).
/// Mirrors the production cell-mailbox convention (1000, see cell factories).
const DISCONNECT_MAILBOX_CAPACITY: usize = 1000;

/// Phase-13.5 Lifecycle-3b Task 4 (A3): `ColonyMsg::Stopped` arm handler.
///
/// The cell-task already fired `peace_tx` (→ watcher silent, no `CellDied`, no
/// `registry.remove`) and closed `cell.db` (→ `death_ack`). This is the full
/// post-stop state machine:
///
/// 1. **Drain remainder → DLQ**: every still-buffered mailbox message becomes a
///    `cell_inactive` dead-letter (order preserved; `sender` = the message's
///    `reply_to`, falling back to root). These messages were addressed to a cell
///    that is now disconnected — they cannot be processed.
/// 2. **Channel-swap**: install a FRESH `(tx, rx)` pair so the registry's sender
///    stays ALIVE for a later reconnect-wake (no dead sender). `entry.handle`
///    takes the new `tx`; `entry.status = NotYetSpawned { receiver: new_rx }`;
///    `entry.active = false`. The drained old receiver is dropped on return.
///
/// Handles a `ColonyMsg::Stopped` event: drains the returned mailbox remainder
/// into the DLQ as `cell_inactive` dead-letters (order preserved, sender = each
/// message's `reply_to`), then swaps a fresh channel pair into the registry entry
/// so a later reconnect-wake hits a live sender.
///
/// Ghost-path: an unknown path (entry already removed, e.g. by a racing
/// `remove_nodes`) drains the remainder to the DLQ and returns — no insert.
async fn handle_stopped(
    registry: &mut HashMap<Path, RegistryEntry>,
    dead_letters: &mut VecDeque<DeadLetter>,
    path: Path,
    mut receiver: mpsc::Receiver<Message>,
) {
    receiver.close();

    // Drain remainder into the DLQ (order preserved).
    while let Ok(msg) = receiver.try_recv() {
        let sender_path = msg.reply_to.clone().unwrap_or_else(|| Path::new("/"));
        push_dead_letter(
            dead_letters,
            DeadLetter {
                sender_path,
                original_target: msg.target.clone(),
                resolved_target: path.clone(),
                message: msg,
                reason: crate::dead_letter::DeadLetterReason::CellInactive,
            },
        );
    }

    // 2. Channel-swap so the registry sender stays alive for a reconnect-wake.
    let Some(entry) = registry.get_mut(&path) else {
        // Entry already removed (racing remove_nodes). Remainder is in the DLQ;
        // nothing to swap.
        return;
    };
    let (new_tx, new_rx) = mpsc::channel::<Message>(DISCONNECT_MAILBOX_CAPACITY);
    entry.handle = ActorHandle::new(path.clone(), new_tx);
    entry.status = CellStatus::NotYetSpawned { receiver: new_rx };
    entry.active = false;
}

/// Parks a `failed` (or otherwise crashed-out) entry as non-running: installs a
/// FRESH `(tx, rx)` pair so the registry sender stays alive for a later
/// reconnect/resume, and sets `status = NotYetSpawned { receiver }`. Mirrors the
/// channel-swap half of `handle_stopped`. Clears the stale `Awake` lifecycle so a
/// later `add_nodes`-resume passes the `resume_requires_stopped_cell` gate.
/// `active`/`failed` are already set by the `handle_cell_died` corridor.
fn park_entry_non_running(entry: &mut RegistryEntry, path: &Path) {
    let (new_tx, new_rx) = mpsc::channel::<Message>(DISCONNECT_MAILBOX_CAPACITY);
    entry.handle = ActorHandle::new(path.clone(), new_tx);
    entry.status = CellStatus::NotYetSpawned { receiver: new_rx };
}

/// GH #18: preserve a rescued mailbox that found no successor.
///
/// Same shape and same reason as the disconnect drain in `handle_stopped`: the
/// remainder is kept in the DLQ, in order, rather than dropped silently.
fn dead_letter_rescued(
    dead_letters: &mut VecDeque<DeadLetter>,
    path: &Path,
    messages: impl IntoIterator<Item = Message>,
) {
    for msg in messages {
        let sender_path = msg.reply_to.clone().unwrap_or_else(|| Path::new("/"));
        push_dead_letter(
            dead_letters,
            DeadLetter {
                sender_path,
                original_target: msg.target.clone(),
                resolved_target: path.clone(),
                message: msg,
                reason: crate::dead_letter::DeadLetterReason::CellInactive,
            },
        );
    }
}

/// GH #18: hand the mailbox rescued from a dying cell to its successor.
///
/// Called at the `CellDied` call-site **after** the byte-frozen
/// `handle_cell_died` corridor returned, which is what makes the delivery
/// possible at all: only then does `entry.handle` carry the fresh mailbox
/// sender of the respawned task. `restarted == false` means the death left no
/// successor (normal end → entry removed, or the restart limit was exhausted)
/// and the remainder goes to the DLQ.
///
/// Order is preserved, and a failing send stops the delivery: everything from
/// that point on is dead-lettered rather than re-ordered behind later traffic.
async fn deliver_rescued_mailbox(
    registry: &HashMap<Path, RegistryEntry>,
    rescued: &mut HashMap<Path, Vec<Message>>,
    dead_letters: &mut VecDeque<DeadLetter>,
    path: &Path,
    restarted: bool,
) {
    let Some(messages) = rescued.remove(path) else {
        return;
    };
    let mut queue: VecDeque<Message> = messages.into();
    if restarted && let Some(handle) = registry.get(path).map(|e| e.handle.clone()) {
        let total = queue.len();
        while let Some(msg) = queue.pop_front() {
            if let Err(e) = handle.send(msg).await {
                queue.push_front(e.0);
                break;
            }
        }
        tracing::info!(
            path = %path.as_str(),
            delivered = total - queue.len(),
            "rescued mailbox messages handed to the respawned cell"
        );
    }
    if !queue.is_empty() {
        tracing::warn!(
            path = %path.as_str(),
            count = queue.len(),
            "rescued mailbox messages have no successor — dead-lettering"
        );
        dead_letter_rescued(dead_letters, path, queue);
    }
}

/// Phase-13.5 Slice 4 T5: `ColonyMsg::StopWiringRestored` arm handler.
///
/// Puts a fresh `(stop_tx, death_ack_rx)` pair back onto the cell's
/// `RegistryEntry` fields — the persistent home of its stop-wiring, `take()`n to
/// `None` on disconnect. After restoration the colony can peace-stop the cell on
/// a later disconnect again.
///
/// Ghost-path: an unknown path (entry already removed, e.g. by a racing
/// `remove_nodes`) drops the pair and returns — no insert.
fn handle_stop_wiring_restored(
    registry: &mut HashMap<Path, RegistryEntry>,
    path: Path,
    stop_tx: tokio::sync::oneshot::Sender<()>,
    death_ack_rx: tokio::sync::oneshot::Receiver<()>,
) {
    if let Some(entry) = registry.get_mut(&path) {
        entry.stop_tx = Some(stop_tx);
        entry.death_ack_rx = Some(death_ack_rx);
    } else {
        // Cell gone from registry (racing remove_nodes). Drop the pair.
        tracing::debug!(
            path = %path.as_str(),
            "StopWiringRestored for unknown cell — dropping the oneshot pair"
        );
    }
}

/// Outcome of a `handle_cell_died` corridor pass. The corridor stays await-free and
/// only mutates in-memory registry state; the caller acts on this (e.g. persists the
/// `failed` status) AFTER the corridor returns.
#[derive(Debug, PartialEq)]
pub enum CellDiedOutcome {
    /// Cell was restarted (Panic/Backstop under the restart limit).
    Restarted,
    /// Cell ended normally (or path already gone) and was removed from the registry.
    Removed,
    /// Cell exhausted its restart limit → marked `failed` in-memory, entry RETAINED.
    Failed { path: Path },
}

/// Handle a `ColonyMsg::CellDied` event: restart cells that ended via panic OR
/// the `message_timeout` B-backstop up to `restart_limit` times with a fresh
/// mpsc pair; remove normally-exiting cells; mark restart-exhausted cells `failed`.
///
/// **Paket-3 P3-B-restart (sanctioned corridor break #1, ruling 2026-06-07)**: the
/// remove-vs-restart branch keys on `DeathKind` instead of the binary
/// `was_panic`. `Normal` → remove; `Panic` OR `Backstop` → one_for_one restart.
/// The `message_timeout` backstop is the first legitimate NON-panic death that
/// must restart (spec § Timeouts B). AUDIT-PRE14-001: panic priority is enforced
/// in `spawn_watcher` (a panic yields `Panic` regardless of the backstop signal).
///
/// **Paket-6 P7-failed (sanctioned corridor break #2, ruling 2026-06-07)**: the
/// restart-EXHAUSTION branch (`restart_count > restart_limit`) marks the entry
/// `failed` in-memory (`failed=true; active=false`) and RETAINS it (No-Delete)
/// instead of `registry.remove`; signature returns `CellDiedOutcome`. The corridor
/// stays await-free — the `SetRegistryStatus{"failed"}` persist and the non-running
/// status parking happen at the `colony_task` call-site, never between respawn and
/// sender-swap (an exhausted cell has no respawn).
///
/// `#[rustfmt::skip]`: this corridor is frozen against a character-exact fixture
/// gate (`plans/paket-6-fixtures/expected_handle_cell_died_body.txt`) — rustfmt
/// must not reformat it.
#[rustfmt::skip]
async fn handle_cell_died(
    registry: &mut HashMap<Path, RegistryEntry>,
    inbox_self_tx: &mpsc::Sender<ColonyMsg>,
    path: Path,
    death_kind: DeathKind,
) -> CellDiedOutcome {
    let Some(entry) = registry.get_mut(&path) else {
        tracing::warn!(path = %path.as_str(), "cell-died for unknown path (already removed)");
        return CellDiedOutcome::Removed;
    };
    if matches!(death_kind, DeathKind::Normal) {
        tracing::info!(path = %path.as_str(), "cell ended normally, removing from registry");
        registry.remove(&path);
        return CellDiedOutcome::Removed;
    }
    // Panic OR Backstop → one_for_one restart (restart_count counts both).
    entry.restart_count += 1;
    if entry.restart_count > entry.restart_limit {
        tracing::error!(
            path = %path.as_str(),
            restarts = entry.restart_count,
            limit = entry.restart_limit,
            "cell exceeded restart limit, marking failed (entry retained, no respawn)"
        );
        entry.failed = true;
        entry.active = false;
        return CellDiedOutcome::Failed { path };
    }
    tracing::warn!(
        path = %path.as_str(),
        restart = entry.restart_count,
        ?death_kind,
        "restarting cell with fresh mpsc pair"
    );
    let (new_sender, new_join, new_peace_rx, new_backstop_rx) = (entry.respawn)();
    entry.handle = ActorHandle::new(path.clone(), new_sender);
    spawn_watcher(inbox_self_tx, path, new_join, new_peace_rx, new_backstop_rx);
    CellDiedOutcome::Restarted
}

/// Configuration for [`colony_task`]. Bundles the (formerly positional) startup
/// inputs so future optional inputs can be added as defaulted builder fields
/// without rippling every call-site. Construct via [`ColonyTaskConfig::new`] with
/// the required inputs; opt into the F3 watchdog via
/// [`ColonyTaskConfig::with_heartbeat`].
pub struct ColonyTaskConfig {
    /// For watchers to feed `CellDied` back.
    pub inbox_self_tx: mpsc::Sender<ColonyMsg>,
    /// Colony inbox receiver.
    pub inbox: mpsc::Receiver<ColonyMsg>,
    /// Phase 6 T18 — for the Mutation-arm spawn-loop.
    pub outputs_tx: mpsc::Sender<CellEmission>,
    /// Cell output envelope receiver.
    pub outputs_rx: mpsc::Receiver<CellEmission>,
    /// Persistence handle (`colony.db`).
    pub colony_db: crate::persist::colony_db::ColonyDb,
    /// Cell factory registry (Phase 6 Mutation arm).
    pub factories: crate::CellFactoryRegistry,
    /// Staging-dir base for the Mutation arm.
    pub root: std::path::PathBuf,
    /// Phase-13.5 A7 — idle-default + blob threshold.
    pub colony_config: crate::ColonyConfig,
    /// Phase-13.5 A8 — delivery-boundary resolution.
    pub blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    /// U8 (RULED A8) — the colony remembers its env source from startup.
    pub env_source: Option<std::path::PathBuf>,
    /// Deep-Audit F3 — liveness signal emitted from the loop; `None` disables the
    /// watchdog (test spawns).
    pub heartbeat_tx: Option<mpsc::Sender<()>>,
    /// stdio-Bridge (Direct-Mode): optional egress sink. When set, a message that
    /// is unroutable at the root hive `/` (HiveNoRoute) goes here instead of the
    /// DLQ. `None` (default) → unchanged DLQ behaviour.
    pub egress_tx: Option<mpsc::Sender<Message>>,
    /// Test-only deterministic sync hook. When set, the colony fires one tick on
    /// this channel right before it begins the inline death-ack-wait of a
    /// disconnect mutation (i.e. peace-stops sent, about to block). Lets a test
    /// release a wedged cell at exactly that point instead of guessing with a
    /// wall-clock sleep. `None` (default, production) → never touched, so the
    /// runtime path is byte-identical to before. Same opt-in pattern as
    /// `egress_tx`/`heartbeat_tx`.
    pub death_ack_wait_tx: Option<mpsc::Sender<()>>,
}

impl ColonyTaskConfig {
    /// Required startup inputs (same set + order as the historical positional
    /// arguments). Optional inputs default to off: `heartbeat_tx = None` (no
    /// watchdog). Use the builder methods to opt in.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inbox_self_tx: mpsc::Sender<ColonyMsg>,
        inbox: mpsc::Receiver<ColonyMsg>,
        outputs_tx: mpsc::Sender<CellEmission>,
        outputs_rx: mpsc::Receiver<CellEmission>,
        colony_db: crate::persist::colony_db::ColonyDb,
        factories: crate::CellFactoryRegistry,
        root: std::path::PathBuf,
        colony_config: crate::ColonyConfig,
        blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
        env_source: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            inbox_self_tx,
            inbox,
            outputs_tx,
            outputs_rx,
            colony_db,
            factories,
            root,
            colony_config,
            blob_store,
            env_source,
            heartbeat_tx: None,
            egress_tx: None,
            death_ack_wait_tx: None,
        }
    }

    /// Opt into the Deep-Audit F3 heartbeat watchdog: the loop emits a liveness
    /// tick on `tx` ~10×/s.
    pub fn with_heartbeat(mut self, tx: mpsc::Sender<()>) -> Self {
        self.heartbeat_tx = Some(tx);
        self
    }

    /// Opt into the Direct-Mode stdio egress sink (root-hive HiveNoRoute → stdout).
    pub fn with_egress(mut self, tx: mpsc::Sender<Message>) -> Self {
        self.egress_tx = Some(tx);
        self
    }

    /// Test-only: opt into the deterministic death-ack-wait sync signal (see
    /// [`ColonyTaskConfig::death_ack_wait_tx`]). Production never calls this, so
    /// the field stays `None` and the runtime path is unchanged.
    pub fn with_death_ack_wait_signal(mut self, tx: mpsc::Sender<()>) -> Self {
        self.death_ack_wait_tx = Some(tx);
        self
    }
}

/// Issue #7: project the colony-owned I/O-liveness map into the read DTO.
///
/// `now` is passed in rather than read here so the projection is a pure function
/// (testable without sleeping). A mark in the future — a clock step backwards
/// between marking and reading — reports as `0` rather than wrapping into a
/// gigantic age.
fn build_liveness_reply(
    marks: &HashMap<Path, Option<std::time::SystemTime>>,
    now: std::time::SystemTime,
) -> crate::api_dto::ReadLivenessReply {
    let mut entries: Vec<crate::api_dto::IoLivenessDto> = marks
        .iter()
        .map(|(path, at)| crate::api_dto::IoLivenessDto {
            path: path.as_str().to_string(),
            last_success_secs: at.map(|t| {
                now.duration_since(t)
                    .map(|d| d.as_secs())
                    .unwrap_or_default()
            }),
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    crate::api_dto::ReadLivenessReply { entries }
}

/// Colony task: runs indefinitely, processing `ColonyMsg`s and cell output envelopes.
pub async fn colony_task(cfg: ColonyTaskConfig) {
    let ColonyTaskConfig {
        inbox_self_tx,
        mut inbox,
        outputs_tx,
        mut outputs_rx,
        colony_db,
        factories,
        root,
        colony_config,
        blob_store,
        env_source,
        heartbeat_tx,
        egress_tx,
        death_ack_wait_tx,
    } = cfg;
    #[cfg(debug_assertions)]
    meclaw_core::init_validator();
    let mut registry: HashMap<Path, RegistryEntry> = HashMap::new();
    // Consumed by handle_mutation (Slice-1 Task 1.4) and the central emits
    // check (Slice 3) — populated here via ColonyMsg::SetNodeContract.
    let mut node_contracts: HashMap<Path, NodeContract> = HashMap::new();
    let mut edges: EdgeTable = EdgeTable::new();
    let mut hive_scopes: HiveScopeTable = HiveScopeTable::new();
    // Issue #7: per-I/O-task progress marks, owned by this task alone. Key = the
    // cell's path, value = when its I/O sub-task last completed a successful
    // external round trip (`None` = announced, none yet). Written only by the
    // `IoLiveness` arm, read only by the `ReadLiveness` arm.
    let mut io_liveness: HashMap<Path, Option<std::time::SystemTime>> = HashMap::new();
    // W6d (A6): transient hand-off buffer only — flushed to the durable
    // `dead_letters` table after every handled event (`persist_dead_letters`).
    // Not a store, not bounded (no drop-oldest), never a second source of truth.
    let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
    // GH #18: mailboxes rescued from dying cell tasks, keyed by cell path. A
    // `MailboxRescued` always arrives BEFORE the matching `CellDied` (the guard
    // hands over while the task is being dropped, the watcher only speaks once
    // the join handle resolved), so an entry lives here for exactly the span
    // between the two events and is emptied by `deliver_rescued_mailbox`.
    let mut rescued_mailboxes: HashMap<Path, Vec<Message>> = HashMap::new();
    // Reboot hydration: classify the boot state, load from DB on a reboot.
    let is_reboot: bool = match colony_db.boot_state() {
        Ok(crate::bootstrap::BootState::FirstBoot) => {
            // FirstBoot: the InitialApply handler works normally (in-memory + persist).
            false
        }
        Ok(crate::bootstrap::BootState::Reboot) => {
            // Reboot: hydrate EdgeTable + HiveScopeTable from DB, ignore hints.
            // Phase-13.5-Durable-Edges: persisted edges carry CEL condition/modifier;
            // read_edges re-parses them, hard-fail on corruption.
            let persisted_edges = colony_db.read_edges().unwrap_or_else(|e| {
                // Hard-fail (F5): corrupt persisted CEL is worse than a boot failure
                // (otherwise routing is silently falsified). {e:?} carries the variant
                // name (ConditionParseFailed/ModifierJsonInvalid) + edge_id + source
                // into the panic message — demo tests (task 7) assert on it via the
                // JoinHandle error.
                panic!("colony.db edge hydration failed: {e:?}");
            });
            for e in persisted_edges {
                edges.insert(crate::edge_table::Edge {
                    id: e.id,
                    from: e.from,
                    to: e.to,
                    condition: e.condition,
                    modifier: e.modifier,
                });
            }
            // Hard-fail (symmetric to read_edges above): a corrupt persisted
            // hive_scopes table is silent routing corruption — the colony would
            // boot scope-blind with an empty/partial hive-scope table. {e:?}
            // carries the rusqlite error into the panic message; the corrupt-
            // hive_scopes demo (phase_16_hive_scope_hydration_hard_fail) asserts
            // on it via JoinHandle-Error, exactly like the edge-hydration path.
            let persisted_scopes = colony_db.read_hive_scopes().unwrap_or_else(|e| {
                panic!("colony.db hive_scope hydration failed: {e:?}");
            });
            for p in persisted_scopes {
                hive_scopes.register(HiveScope { path: p });
            }
            tracing::info!("hydrated edges/hive_scopes from colony.db; params.graph hints ignored");
            true
        }
        Ok(crate::bootstrap::BootState::Inconsistent { reason }) => {
            tracing::error!(reason = %reason, "inconsistent colony.db on boot");
            panic!("inconsistent colony.db: {reason}");
        }
        Err(e) => {
            tracing::error!(error = %format!("{e:?}"), "boot_state probe failed");
            panic!("boot_state probe failed: {e:?}");
        }
    };

    // Deep-Audit F3: heartbeat liveness clock. The interval arm (last in the
    // biased select below) wakes the loop ~10×/s while idle; the actual heartbeat
    // is emitted at the TOP of every iteration (message-driven OR interval-driven)
    // so a saturated inbox never starves it. Panic → loop gone → heartbeat stops;
    // a handler wedged in `.await` → loop stuck → heartbeat stops. Both detected by
    // the supervisor. NOTE: this arm lives in the select-LOOP, NOT in `route()` /
    // `handle_cell_died` (both stay byte-frozen).
    let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_millis(100));

    loop {
        // Deep-Audit F3: emit a liveness tick. `try_send` never blocks the loop; a
        // full channel just means the supervisor hasn't drained yet (it needs only
        // ≥1 tick per period). `None` → watchdog disabled (test spawns).
        if let Some(hb) = &heartbeat_tx {
            let _ = hb.try_send(());
        }
        // W6d (A6): flush any DLQ pushes left over from the previous iteration into
        // the durable `dead_letters` table. The post-select flush at the bottom
        // catches the normal paths, but the outputs-arm has `continue` statements
        // (no_route emission, invalid-UBF, /colony re-enqueue) that skip it — this
        // top-of-loop drain is the robust catch-all: nothing pushed survives past
        // the start of the next event unpersisted, so a Read/Drain (always the next
        // event) sees it after its fence. Empty buffer ⇒ no-op (no await).
        persist_dead_letters(&mut dead_letters, &colony_db.writer_tx).await;
        tokio::select! {
            biased;
            Some(m) = inbox.recv() => {
                match m {
                    ColonyMsg::Shutdown { ack } => {
                        // Phase-5 minimal shutdown (E6):
                        // 1. Close inbox — no new sends accepted.
                        inbox.close();
                        // 2. Drain buffered items.
                        while let Ok(m) = inbox.try_recv() {
                            match m {
                                ColonyMsg::Shutdown { .. } => {} // skip nested Shutdown in drain
                                ColonyMsg::Register { path, sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx, respawn, wake, restart_limit, cell_id, cell_type, active, ack: reg_ack } => {
                                    handle_register(&mut registry, &inbox_self_tx, &colony_db.writer_tx, &colony_db.queue_depth, path, sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx, respawn, wake, restart_limit, cell_id, cell_type, active, reg_ack).await;
                                }
                                ColonyMsg::RegisterDormant { path, sender, receiver, respawn, wake, restart_limit, cell_id, cell_type, active, failed, eager_on_reconnect, ack: reg_ack } => {
                                    handle_register_dormant(&mut registry, &colony_db.writer_tx, &colony_db.queue_depth, path, sender, receiver, respawn, wake, restart_limit, cell_id, cell_type, active, failed, eager_on_reconnect, reg_ack).await;
                                }
                                ColonyMsg::AddEdge { id, from, to, ack: edge_ack } => {
                                    edges.insert(Edge { id, from, to, condition: None, modifier: None });
                                    let _ = edge_ack.send(());
                                }
                                ColonyMsg::SetNodeContract { path, contract, ack: nc_ack } => {
                                    node_contracts.insert(path, contract);
                                    let _ = nc_ack.send(());
                                }
                                ColonyMsg::SetRegistryProvenance { path, provenance, ack: prov_ack } => {
                                    send_registry_provenance(&colony_db.writer_tx, &colony_db.queue_depth, path, provenance).await;
                                    let _ = prov_ack.send(());
                                }
                                ColonyMsg::AddHiveScope { path, ack: scope_ack } => {
                                    hive_scopes.register(HiveScope { path });
                                    let _ = scope_ack.send(());
                                }
                                ColonyMsg::IoLiveness { path, at } => {
                                    io_liveness.insert(path, at);
                                }
                                ColonyMsg::ReadLiveness { ack: lv_ack } => {
                                    // Shutdown-drain: Read is best-effort; drop ack silently.
                                    drop(lv_ack);
                                }
                                ColonyMsg::Route { sender_path, msg } => {
                                    let mut work: VecDeque<(Path, Message)> = VecDeque::new();
                                    work.push_back((sender_path, msg));
                                    while let Some((s, m)) = work.pop_front() {
                                        match route_with_log(&mut registry, &hive_scopes, &mut dead_letters, &colony_db.writer_tx, s, m, &blob_store, colony_config.blob_inline_max_bytes).await {
                                            RouteAction::Done => {}
                                            RouteAction::Cascade { sender, msg } => {
                                                work.push_back((sender, msg));
                                            }
                                            RouteAction::ColonyDispatch { endpoint, msg, sender } => {
                                                // T3: the real dispatcher — pre-extract every `&ColonyDb` sub-ref
                                                // SYNCHRONOUSLY, then await. ColonyDb is !Sync → no
                                                // `&ColonyDb` across an .await boundary. Templates
                                                // snapshot + pre-extracted template rows + rescan-future
                                                // prologue all obtained from the sync borrow.
                                                let templates_rows = colony_db.read_templates().unwrap_or_default();
                                                let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(
                                                    templates_rows.clone().into_iter()
                                                        .map(|r| crate::templates::TemplateEntry {
                                                            template_id: r.template_id,
                                                            name: r.name,
                                                            version: r.version,
                                                            filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                                                        }).collect(),
                                                );
                                                let rescan_future = Box::pin(crate::colony_dispatch::handle_rescan_templates(&colony_db, &root));
                                                let db_path = colony_db.db_path().to_path_buf();
                                                let follow = crate::colony_dispatch::dispatch_colony_endpoint(
                                                    &mut registry, &mut hive_scopes, &mut edges, &mut node_contracts, &mut dead_letters,
                                                    &colony_db.writer_tx, &db_path,
                                                    templates_snapshot, templates_rows, rescan_future,
                                                    &factories, &root,
                                                    &inbox_self_tx, &outputs_tx,
                                                    endpoint, msg, sender,
                                                    colony_config.idle_timeout_default_ms,
                                                    colony_config.message_timeout_default_ms,
                                                    colony_config.mailbox_default_capacity,
                                                    colony_config.strict_validation,
                                                    blob_store.clone(),
                                                    colony_config.blob_inline_max_bytes,
                                                    env_source.as_deref(),
                                                ).await;
                                                enqueue_dispatch_follow(&mut work, &mut dead_letters, follow);
                                            }
                                            RouteAction::HiveTransit { hive_path, msg } => {
                                                enqueue_hive_transit(&mut work, &mut dead_letters, &edges, hive_path, msg, egress_tx.as_ref(), colony_config.message_default_ttl);
                                            }
                                        }
                                    }
                                }
                                ColonyMsg::CellDied { path, death_kind } => {
                                    // GH #18: keep the path — the corridor consumes it,
                                    // and the mailbox rescue is keyed on it.
                                    let died = path.clone();
                                    let outcome = handle_cell_died(&mut registry, &inbox_self_tx, path, death_kind).await;
                                    let restarted = matches!(outcome, CellDiedOutcome::Restarted);
                                    if let CellDiedOutcome::Failed { path } = outcome {
                                        let now = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .expect("system time")
                                            .as_secs() as i64;
                                        let _ = colony_db
                                            .writer_tx
                                            .send(crate::persist::writer::ColonyWriteOp::SetRegistryStatus {
                                                path: path.clone(),
                                                status: "failed".into(),
                                                updated_at: now,
                                            })
                                            .await;
                                        if let Some(e) = registry.get_mut(&path) {
                                            park_entry_non_running(e, &path);
                                        }
                                    }
                                    deliver_rescued_mailbox(&registry, &mut rescued_mailboxes, &mut dead_letters, &died, restarted).await;
                                }
                                ColonyMsg::DrainDeadLetters { ack: dl_ack } => {
                                    // W6d (A6): shutdown-drain has no post-select
                                    // flush between buffered messages, so flush any
                                    // in-loop pushes to the DB first, THEN drain it.
                                    persist_dead_letters(&mut dead_letters, &colony_db.writer_tx).await;
                                    fence(&colony_db.writer_tx).await;
                                    let drained = crate::colony_dispatch::handle_drain_dead_letters(&colony_db);
                                    let (del_tx, del_rx) = tokio::sync::oneshot::channel();
                                    let _ = colony_db
                                        .writer_tx
                                        .send(crate::persist::writer::ColonyWriteOp::DeleteAllDeadLetters { ack: Some(del_tx) })
                                        .await;
                                    let _ = del_rx.await;
                                    let _ = dl_ack.send(drained);
                                }
                                ColonyMsg::DeadLetterMessage { message, reason } => {
                                    let target = message.target.clone();
                                    push_dead_letter(
                                        &mut dead_letters,
                                        DeadLetter {
                                            sender_path: target.clone(),
                                            original_target: target.clone(),
                                            resolved_target: target,
                                            message,
                                            reason,
                                        },
                                    );
                                }
                                ColonyMsg::InitialApply { edges: ia_edges, hive_scopes: ia_scopes, ack: ia_ack } => {
                                    if !is_reboot {
                                        // FirstBoot: in-memory inserts + persist.
                                        for s in &ia_scopes {
                                            hive_scopes.register(HiveScope { path: s.clone() });
                                        }
                                        for e in &ia_edges {
                                            // Phase 13.5-A1: carry condition/modifier over from PlannedEdge.
                                            edges.insert(Edge {
                                                id: e.id,
                                                from: e.from.clone(),
                                                to: e.to.clone(),
                                                condition: e.condition.clone(),
                                                modifier: e.modifier.clone(),
                                            });
                                        }
                                        // Direct send via writer_tx (NOT &ColonyDb across .await,
                                        // see the handle_register rationale).
                                        colony_db
                                            .queue_depth
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        let depth = colony_db
                                            .queue_depth
                                            .load(std::sync::atomic::Ordering::Relaxed);
                                        if depth > 1000 {
                                            tracing::warn!(
                                                depth,
                                                "colony.db writer backlog > 1000"
                                            );
                                        }
                                        colony_db
                                            .writer_tx
                                            .send(crate::persist::writer::ColonyWriteOp::InitialApply {
                                                edges: ia_edges,
                                                hive_scopes: ia_scopes,
                                            })
                                            .await
                                            .expect("writer thread dead");
                                    }
                                    // Reboot: skip — edges/hive_scopes were hydrated at boot start.
                                    let _ = ia_ack.send(());
                                }
                                ColonyMsg::BeginInitialApply { ack } => {
                                    // Shutdown-drain: no marker write — the boot is
                                    // aborting anyway; just unblock the waiting apply.
                                    let _ = ack.send(());
                                }
                                ColonyMsg::RescanTemplates { templates_root, ack } => {
                                    if let Err(e) = crate::colony_dispatch::handle_rescan_templates(&colony_db, &templates_root).await {
                                        tracing::error!(error = ?e, "rescan failed (drain)");
                                    }
                                    let _ = ack.send(());
                                }
                                ColonyMsg::ReadRegistry { ack, .. } => {
                                    // Shutdown-drain: Read is best-effort; drop ack silently
                                    // so the caller's `rx.await` resolves with RecvError.
                                    drop(ack);
                                }
                                ColonyMsg::ReadDeadLetters { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::ReadTemplates { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::ReadMutationsAudit { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::ReadGraph { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::ReadTrace { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::ReadMessages { ack, .. } => {
                                    drop(ack);
                                }
                                ColonyMsg::Mutation { payload, reply_to, trace_id, parent_message_id, ack } => {
                                    // Phase-11 T16: Templates-Snapshot SYNCHRON vor handle_mutation
                                    // (ColonyDb is !Sync → no &ColonyDb across an .await boundary).
                                    let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(
                                        colony_db.read_templates().unwrap_or_default().into_iter()
                                            .map(|r| crate::templates::TemplateEntry {
                                                template_id: r.template_id,
                                                name: r.name,
                                                version: r.version,
                                                filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                                            }).collect(),
                                    );
                                    let outcome = handle_mutation(
                                        &mut registry, &mut hive_scopes, &mut edges, &mut node_contracts, &mut dead_letters,
                                        &colony_db.writer_tx, templates_snapshot, &factories, &root, &inbox_self_tx,
                                        &outputs_tx,
                                        payload, reply_to, trace_id, parent_message_id,
                                        colony_config.idle_timeout_default_ms,
                                        colony_config.message_timeout_default_ms,
                                        colony_config.mailbox_default_capacity,
                                        colony_config.strict_validation,
                                        blob_store.clone(),
                                        colony_config.blob_inline_max_bytes,
                                        env_source.as_deref(),
                                        death_ack_wait_tx.as_ref(),
                                    ).await;
                                    let _ = ack.send(outcome);
                                }
                                ColonyMsg::Sleep { path, receiver } => {
                                    handle_sleep(&mut registry, &mut dead_letters, &inbox_self_tx, path, receiver).await;
                                }
                                ColonyMsg::Stopped { path, receiver } => {
                                    handle_stopped(&mut registry, &mut dead_letters, path, receiver).await;
                                }
                                ColonyMsg::StopWiringRestored { path, stop_tx, death_ack_rx } => {
                                    handle_stop_wiring_restored(&mut registry, path, stop_tx, death_ack_rx);
                                }
                                ColonyMsg::MailboxRescued { path, messages } => {
                                    // Shutdown-drain: no successor is coming, so
                                    // the rescue goes straight to the DLQ (the
                                    // flush below this loop still catches it).
                                    dead_letter_rescued(&mut dead_letters, &path, messages);
                                }
                            }
                        }
                        // GH #18: a rescue whose `CellDied` never arrived (shutdown
                        // cut in between) has no successor to wait for — preserve it
                        // rather than let the map die with the task.
                        for (path, messages) in rescued_mailboxes.drain() {
                            dead_letter_rescued(&mut dead_letters, &path, messages);
                        }
                        // W6d (A6): flush any DLQ pushes from the shutdown-drain
                        // loop BEFORE the writer is torn down — the Shutdown arm
                        // breaks out of the loop, so the post-select drain never
                        // runs for it. FIFO guarantees these land before the
                        // writer's own Shutdown op.
                        persist_dead_letters(&mut dead_letters, &colony_db.writer_tx).await;
                        // 3. Shutdown writer thread (async variant — we are in a Tokio context).
                        colony_db.shutdown_async().await;
                        // 4. Ack + break.
                        let _ = ack.send(());
                        break;
                    }
                    ColonyMsg::Register { path, sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx, respawn, wake, restart_limit, cell_id, cell_type, active, ack } => {
                        handle_register(&mut registry, &inbox_self_tx, &colony_db.writer_tx, &colony_db.queue_depth, path, sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx, respawn, wake, restart_limit, cell_id, cell_type, active, ack).await;
                    }
                    ColonyMsg::RegisterDormant { path, sender, receiver, respawn, wake, restart_limit, cell_id, cell_type, active, failed, eager_on_reconnect, ack } => {
                        handle_register_dormant(&mut registry, &colony_db.writer_tx, &colony_db.queue_depth, path, sender, receiver, respawn, wake, restart_limit, cell_id, cell_type, active, failed, eager_on_reconnect, ack).await;
                    }
                    ColonyMsg::AddEdge { id, from, to, ack } => {
                        edges.insert(Edge { id, from, to, condition: None, modifier: None });
                        let _ = ack.send(());
                    }
                    ColonyMsg::SetNodeContract { path, contract, ack } => {
                        node_contracts.insert(path, contract);
                        let _ = ack.send(());
                    }
                    ColonyMsg::SetRegistryProvenance { path, provenance, ack } => {
                        send_registry_provenance(&colony_db.writer_tx, &colony_db.queue_depth, path, provenance).await;
                        let _ = ack.send(());
                    }
                    ColonyMsg::AddHiveScope { path, ack } => {
                        hive_scopes.register(HiveScope { path });
                        let _ = ack.send(());
                    }
                    // Issue #7: an I/O sub-task reports on itself. Pure in-memory
                    // upsert — no DB, no await, so a marking cell never competes
                    // with routing for loop time.
                    ColonyMsg::IoLiveness { path, at } => {
                        io_liveness.insert(path, at);
                    }
                    ColonyMsg::ReadLiveness { ack } => {
                        let _ = ack.send(build_liveness_reply(&io_liveness, std::time::SystemTime::now()));
                    }
                    ColonyMsg::Route { sender_path, msg } => {
                        let mut work: VecDeque<(Path, Message)> = VecDeque::new();
                        work.push_back((sender_path, msg));
                        while let Some((s, m)) = work.pop_front() {
                            match route_with_log(&mut registry, &hive_scopes, &mut dead_letters, &colony_db.writer_tx, s, m, &blob_store, colony_config.blob_inline_max_bytes).await {
                                RouteAction::Done => {}
                                RouteAction::Cascade { sender, msg } => {
                                    work.push_back((sender, msg));
                                }
                                RouteAction::ColonyDispatch { endpoint, msg, sender } => {
                                    // T3: the real dispatcher — pre-extract every `&ColonyDb` sub-ref
                                    // SYNCHRONOUSLY, then await. ColonyDb is !Sync → no
                                    // `&ColonyDb` across an .await boundary. Templates
                                    // snapshot + pre-extracted template rows + rescan-future
                                    // prologue all obtained from the sync borrow.
                                    let templates_rows = colony_db.read_templates().unwrap_or_default();
                                    let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(
                                        templates_rows.clone().into_iter()
                                            .map(|r| crate::templates::TemplateEntry {
                                                template_id: r.template_id,
                                                name: r.name,
                                                version: r.version,
                                                filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                                            }).collect(),
                                    );
                                    let rescan_future = Box::pin(crate::colony_dispatch::handle_rescan_templates(&colony_db, &root));
                                    let db_path = colony_db.db_path().to_path_buf();
                                    let follow = crate::colony_dispatch::dispatch_colony_endpoint(
                                        &mut registry, &mut hive_scopes, &mut edges, &mut node_contracts, &mut dead_letters,
                                        &colony_db.writer_tx, &db_path,
                                        templates_snapshot, templates_rows, rescan_future,
                                        &factories, &root,
                                        &inbox_self_tx, &outputs_tx,
                                        endpoint, msg, sender,
                                        colony_config.idle_timeout_default_ms,
                                        colony_config.message_timeout_default_ms,
                                        colony_config.mailbox_default_capacity,
                                        colony_config.strict_validation,
                                        blob_store.clone(),
                                        colony_config.blob_inline_max_bytes,
                                        env_source.as_deref(),
                                    ).await;
                                    enqueue_dispatch_follow(&mut work, &mut dead_letters, follow);
                                }
                                RouteAction::HiveTransit { hive_path, msg } => {
                                    enqueue_hive_transit(&mut work, &mut dead_letters, &edges, hive_path, msg, egress_tx.as_ref(), colony_config.message_default_ttl);
                                }
                            }
                        }
                    }
                    ColonyMsg::CellDied { path, death_kind } => {
                        // GH #18: keep the path — the corridor consumes it, and the
                        // mailbox rescue is keyed on it.
                        let died = path.clone();
                        let outcome = handle_cell_died(&mut registry, &inbox_self_tx, path, death_kind).await;
                        let restarted = matches!(outcome, CellDiedOutcome::Restarted);
                        if let CellDiedOutcome::Failed { path } = outcome {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("system time")
                                .as_secs() as i64;
                            let _ = colony_db
                                .writer_tx
                                .send(crate::persist::writer::ColonyWriteOp::SetRegistryStatus {
                                    path: path.clone(),
                                    status: "failed".into(),
                                    updated_at: now,
                                })
                                .await;
                            if let Some(e) = registry.get_mut(&path) {
                                park_entry_non_running(e, &path);
                            }
                        }
                        deliver_rescued_mailbox(&registry, &mut rescued_mailboxes, &mut dead_letters, &died, restarted).await;
                    }
                    ColonyMsg::DrainDeadLetters { ack } => {
                        // W6d (A6): drain from the DB (source of truth). Fence so
                        // prior fire-and-forget inserts are durable, snapshot+
                        // reconstruct (sync — `!Sync` borrow, no await), then clear
                        // the table. No `&ColonyDb` is held across an await; the
                        // single-owner task runs nothing between read and DELETE.
                        fence(&colony_db.writer_tx).await;
                        let drained = crate::colony_dispatch::handle_drain_dead_letters(&colony_db);
                        let (del_tx, del_rx) = tokio::sync::oneshot::channel();
                        let _ = colony_db
                            .writer_tx
                            .send(crate::persist::writer::ColonyWriteOp::DeleteAllDeadLetters { ack: Some(del_tx) })
                            .await;
                        let _ = del_rx.await;
                        let _ = ack.send(drained);
                    }
                    ColonyMsg::DeadLetterMessage { message, reason } => {
                        let target = message.target.clone();
                        push_dead_letter(
                            &mut dead_letters,
                            DeadLetter {
                                sender_path: target.clone(),
                                original_target: target.clone(),
                                resolved_target: target,
                                message,
                                reason,
                            },
                        );
                    }
                    ColonyMsg::InitialApply { edges: ia_edges, hive_scopes: ia_scopes, ack } => {
                        if !is_reboot {
                            // FirstBoot: in-memory inserts + persist.
                            for s in &ia_scopes {
                                hive_scopes.register(HiveScope { path: s.clone() });
                            }
                            for e in &ia_edges {
                                // Phase 13.5-A1: carry condition/modifier over from PlannedEdge.
                                edges.insert(Edge {
                                    id: e.id,
                                    from: e.from.clone(),
                                    to: e.to.clone(),
                                    condition: e.condition.clone(),
                                    modifier: e.modifier.clone(),
                                });
                            }
                            // op-before-ack. Direct send via writer_tx (NOT &ColonyDb
                            // across .await, see the handle_register rationale).
                            colony_db
                                .queue_depth
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let depth = colony_db
                                .queue_depth
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if depth > 1000 {
                                tracing::warn!(depth, "colony.db writer backlog > 1000");
                            }
                            colony_db
                                .writer_tx
                                .send(crate::persist::writer::ColonyWriteOp::InitialApply {
                                    edges: ia_edges,
                                    hive_scopes: ia_scopes,
                                })
                                .await
                                .expect("writer thread dead");
                        }
                        // Reboot: skip — edges/hive_scopes were hydrated at boot start.
                        let _ = ack.send(());
                    }
                    ColonyMsg::BeginInitialApply { ack } => {
                        if !is_reboot {
                            // Durable marker write: ack only AFTER the writer
                            // committed — the apply must not spawn a single cell
                            // before the marker is on disk.
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("system time")
                                .as_secs() as i64;
                            let (marker_tx, marker_rx) = oneshot::channel();
                            colony_db
                                .queue_depth
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            colony_db
                                .writer_tx
                                .send(crate::persist::writer::ColonyWriteOp::SetBootstrapInFlight {
                                    created_at: now,
                                    ack: Some(marker_tx),
                                })
                                .await
                                .expect("writer thread dead");
                            marker_rx.await.expect("writer thread dead (marker ack)");
                        }
                        let _ = ack.send(());
                    }
                    ColonyMsg::RescanTemplates { templates_root, ack } => {
                        if let Err(e) = crate::colony_dispatch::handle_rescan_templates(&colony_db, &templates_root).await {
                            tracing::error!(error = ?e, "rescan failed");
                        }
                        let _ = ack.send(());
                    }
                    ColonyMsg::ReadTrace { trace_id, path_prefix, correlation_id, only_error, since, limit, ack } => {
                        let db_path = colony_db.db_path().to_path_buf();
                        let reply = crate::colony_dispatch::handle_read_trace(
                            &db_path, trace_id, path_prefix, correlation_id, only_error, since, limit,
                        ).await;
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadMessages { filter, ack } => {
                        let db_path = colony_db.db_path().to_path_buf();
                        let reply = crate::colony_dispatch::handle_read_messages(&db_path, filter).await;
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadGraph { scope, ack } => {
                        let reply = crate::colony_dispatch::handle_read_graph(&registry, &edges, scope);
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadMutationsAudit { since, limit, ack } => {
                        let reply = crate::colony_dispatch::handle_read_mutations_audit(&colony_db, since, limit);
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadTemplates { cell_type, name, limit, ack } => {
                        let reply = crate::colony_dispatch::handle_read_templates(&colony_db, cell_type, name, limit);
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadDeadLetters { since, error_code, limit, ack } => {
                        // W6d (A6): fence (await on Send `&writer_tx`) so prior
                        // fire-and-forget InsertDeadLetter ops are durable, THEN the
                        // sync read (no `&ColonyDb` across await — it is `!Sync`).
                        fence(&colony_db.writer_tx).await;
                        let reply = crate::colony_dispatch::handle_read_dead_letters(&colony_db, since, error_code, limit);
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::ReadRegistry { path, path_prefix, cell_type, active, limit, ack } => {
                        let reply = crate::colony_dispatch::handle_read_registry(&registry, path, path_prefix, cell_type, active, limit);
                        let _ = ack.send(reply);
                    }
                    ColonyMsg::Mutation { payload, reply_to, trace_id, parent_message_id, ack } => {
                        // Phase-11 T16: Templates-Snapshot SYNCHRON vor handle_mutation
                        // (ColonyDb is !Sync → no &ColonyDb across an .await boundary).
                        let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(
                            colony_db.read_templates().unwrap_or_default().into_iter()
                                .map(|r| crate::templates::TemplateEntry {
                                    template_id: r.template_id,
                                    name: r.name,
                                    version: r.version,
                                    filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                                }).collect(),
                        );
                        let outcome = handle_mutation(
                            &mut registry, &mut hive_scopes, &mut edges, &mut node_contracts, &mut dead_letters,
                            &colony_db.writer_tx, templates_snapshot, &factories, &root, &inbox_self_tx,
                            &outputs_tx,
                            payload, reply_to, trace_id, parent_message_id,
                            colony_config.idle_timeout_default_ms,
                            colony_config.message_timeout_default_ms,
                            colony_config.mailbox_default_capacity,
                            colony_config.strict_validation,
                            blob_store.clone(),
                            colony_config.blob_inline_max_bytes,
                            env_source.as_deref(),
                            death_ack_wait_tx.as_ref(),
                        ).await;
                        let _ = ack.send(outcome);
                    }
                    ColonyMsg::Sleep { path, receiver } => {
                        handle_sleep(&mut registry, &mut dead_letters, &inbox_self_tx, path, receiver).await;
                    }
                    ColonyMsg::Stopped { path, receiver } => {
                        handle_stopped(&mut registry, &mut dead_letters, path, receiver).await;
                    }
                    ColonyMsg::StopWiringRestored { path, stop_tx, death_ack_rx } => {
                        handle_stop_wiring_restored(&mut registry, path, stop_tx, death_ack_rx);
                    }
                    ColonyMsg::MailboxRescued { path, messages } => {
                        // GH #18: park until the matching `CellDied` decided whether
                        // there IS a successor. Delivery happens there, never here —
                        // at this moment `entry.handle` still points at the dead
                        // task's channel.
                        rescued_mailboxes.entry(path).or_default().extend(messages);
                    }
                }
            }
            Some(em) = outputs_rx.recv() => {
                // TTL slice (2026-06-11): a source emission (parent_message_id ==
                // None — the OriginSink shape of timer/proxy/mcp) gets its fresh
                // TTL from colony.json `message_default_ttl` here. Envelope-Setter-
                // Authority (spec § Message model): Colony stamps `ttl` anew on
                // source messages; the OriginSink `input_ttl` is only the constant
                // seed. Follow-up emissions inherit the consumed input's TTL.
                let em = if em.parent_message_id.is_none() {
                    CellEmission { input_ttl: colony_config.message_default_ttl, ..em }
                } else {
                    em
                };
                // W2b (Ruling A1, ruling 2026-06-12): a substrate-generated error
                // reply addressed to a known sender (`direct_reply` — consumes_violation
                // ingress check / message_timeout backstop) is delivered DIRECTLY to
                // its `target` (== the input's reply_to) via route_with_log, exactly
                // like the contract_violation path below — registry lookup, NOT the
                // sender's out-edges. It is feedback to a known absender, not a routing
                // emission, so the A1 no_route rule must not divert or DLQ it. This
                // runs before the emits/UBF checks (substrate-built, already valid).
                if em.direct_reply {
                    let (err_hop, err_body) = split_content_header(em.content.clone());
                    let err_msg = MessageBuilder::new(em.target.clone())
                        .trace_id(em.trace_id)
                        .parent_message_id_opt(em.parent_message_id)
                        .reply_to(em.sender_path.clone())
                        .ttl(em.input_ttl)
                        .hop(err_hop)
                        .body(Body::Inline(err_body))
                        .build();
                    let mut work: VecDeque<(Path, Message)> = VecDeque::new();
                    work.push_back((em.sender_path.clone(), err_msg));
                    while let Some((s, m)) = work.pop_front() {
                        // Only Cascade is continued — the reply addresses a cell.
                        // Unresolvable reply_to → route_with_log dead-letters it.
                        if let RouteAction::Cascade { sender, msg } = route_with_log(&mut registry, &hive_scopes, &mut dead_letters, &colony_db.writer_tx, s, m, &blob_store, colony_config.blob_inline_max_bytes).await {
                            work.push_back((sender, msg));
                        }
                    }
                    continue;
                }
                // (1+2) Header-split preview + validation — debug-only. In release
                //       the whole block disappears → true zero overhead.
                //       build_follow_up_message performs the split again internally
                //       for the merge; the duplicated preview is debug-only and
                //       uncritical.
                #[cfg(debug_assertions)]
                {
                    let (_cell_headers, body_candidate) = split_content_header(em.content.clone());
                    if let Err(errors) = validate_ubf_body(&body_candidate) {
                        tracing::warn!(
                            sender = %em.sender_path.as_str(),
                            target = %em.target.as_str(),
                            trace_id = %em.trace_id,
                            reason = "InvalidUbfBody",
                            errors = %errors,
                            "cell emitted invalid UBF body — direct-DLQ, no reply"
                        );
                        let bad_msg = MessageBuilder::new(em.target.clone())
                            .trace_id(em.trace_id)
                            .parent_message_id_opt(em.parent_message_id)
                            .reply_to(em.sender_path.clone())
                            .ttl(em.input_ttl)
                            .body(Body::Inline(body_candidate))
                            .build();
                        push_dead_letter(
                            &mut dead_letters,
                            DeadLetter {
                                sender_path: em.sender_path.clone(),
                                original_target: em.target.clone(),
                                resolved_target: em.target.clone(),
                                message: bad_msg,
                                reason: crate::dead_letter::DeadLetterReason::InvalidUbfBody,
                            },
                        );
                        continue;
                    }
                }

                // Slice 3 (roadmap Z.135): central emits check for NON-code emitters,
                // flag-gated (resolve_validate_emits at spawn → NodeContract.validate_emits).
                // `code` is excluded here — its always-on two-pass in-cell validation
                // (cell-types.md Z.264) stays the trust boundary; central skip prevents
                // double-reject semantics. A violating emission is DROPPED: with an
                // input_reply_to an error reply (same canonical token `contract_violation`)
                // is routed instead; otherwise the emission dead-letters.
                if let Some(nc) = node_contracts.get(&em.sender_path) {
                    let is_code = registry
                        .get(&em.sender_path)
                        .is_some_and(|e| e.cell_type == "code");
                    if !is_code
                        && nc.validate_emits
                        && let Some(compiled) = &nc.emits
                        && let Err(reason) = meclaw_core::validate_emits(&em.content, compiled)
                    {
                        tracing::warn!(
                            sender = %em.sender_path.as_str(),
                            target = %em.target.as_str(),
                            trace_id = %em.trace_id,
                            error = %reason,
                            "emission violates contract.emits — dropped (contract_violation)"
                        );
                        if let Some(reply_target) = em.input_reply_to.clone() {
                            // Error reply instead of the violating emission. The
                            // `content.header` section travels in the hop compartment
                            // (split_content_header) — same wire shape the outputs arm
                            // produces for cell emissions (consumes_violation pattern).
                            let content = meclaw_core::serde_json::json!({
                                "header": {"finish_reason": "error", "error_code": "contract_violation"},
                                "messages": [{"origin": "assistant", "type": "text", "text": reason}]
                            });
                            let (err_hop, err_body) = split_content_header(content);
                            let err_msg = MessageBuilder::new(reply_target)
                                .trace_id(em.trace_id)
                                .parent_message_id_opt(em.parent_message_id)
                                .reply_to(em.sender_path.clone())
                                .ttl(em.input_ttl)
                                .hop(err_hop)
                                .body(Body::Inline(err_body))
                                .build();
                            let mut work: VecDeque<(Path, Message)> = VecDeque::new();
                            work.push_back((em.sender_path.clone(), err_msg));
                            while let Some((s, m)) = work.pop_front() {
                                // Plan sanction (Task 3.2): only `Cascade` is continued —
                                // the reply addresses a cell; ColonyDispatch/HiveTransit
                                // are deliberately not pursued for this error reply.
                                // Non-Cascade actions (ColonyDispatch/HiveTransit reply_to)
                                // are consciously dropped here — exotic reply_to targets,
                                // POC-accepted (Slice-3 review note).
                                if let RouteAction::Cascade { sender, msg } = route_with_log(&mut registry, &hive_scopes, &mut dead_letters, &colony_db.writer_tx, s, m, &blob_store, colony_config.blob_inline_max_bytes).await {
                                    work.push_back((sender, msg));
                                }
                            }
                        } else {
                            // direct-DLQ, no reply (no input_reply_to present — mirrors the UBF block above)
                            let bad_msg = MessageBuilder::new(em.target.clone())
                                .trace_id(em.trace_id)
                                .parent_message_id_opt(em.parent_message_id)
                                .reply_to(em.sender_path.clone())
                                .ttl(em.input_ttl)
                                .body(Body::Inline(em.content.clone()))
                                .build();
                            push_dead_letter(
                                &mut dead_letters,
                                DeadLetter {
                                    sender_path: em.sender_path.clone(),
                                    original_target: em.target.clone(),
                                    resolved_target: em.target.clone(),
                                    message: bad_msg,
                                    reason: crate::dead_letter::DeadLetterReason::ContractViolation,
                                },
                            );
                        }
                        continue;
                    }
                }

                // (3) Valid → edge hook + unified cascade loop. Phase 4: implicit
                //     identity decision when no edge matches; otherwise the edge
                //     overlays the target. Fan-out: one follow_up per decision.
                let from = em.sender_path.clone();

                // Two-compartment decay: context travels through, hop = isolated cell output.
                // input.hop is dropped (structural freshness, ADR-0001).
                let (cell_hop, _body_unused) = split_content_header(em.content.clone());
                let merged_headers = em.input_headers.carry_context_with_hop(cell_hop);

                // W2b (Ruling A1 + the spec owner 2026-06-12): a cell emission targeting a
                // /colony/* VIRTUAL service endpoint (EDA — cell-emitted mutation/read,
                // phase-13.5 A6) is DISPATCHED DIRECTLY, before apply_edges. /colony/*
                // are virtual endpoints (overview § Behavior on routing errors), not topology nodes,
                // so no out-edge is needed or possible and the A1 no_route rule does not
                // apply to them. Re-enqueue as ColonyMsg::Route so the Route handler runs
                // the full ColonyDispatch machinery (unknown endpoint ⇒
                // ColonyEndpointUnimplemented). Pre-W2a this worked only by accident via
                // the identity-fallback (the W2a fallback inventory misclassified it as
                // "Cell→/colony/* DLQs already").
                if em.target.as_str() == "/colony" || em.target.as_str().starts_with("/colony/") {
                    let follow_up = build_follow_up_with(em.clone(), em.target.clone(), merged_headers);
                    let _ = inbox_self_tx
                        .send(ColonyMsg::Route { sender_path: from.clone(), msg: follow_up })
                        .await;
                    continue;
                }

                let matched = apply_edges(&edges, &from, &merged_headers);
                if matched.is_empty() {
                    // Ruling A1: a cell emission that matches no out-edge no
                    // longer identity-routes to its own emission target — it
                    // dead-letters as `no_route` (the Cell analogue to
                    // `hive_no_route`). Default routing is a settable catch-all
                    // out-edge. The follow-up message carries the trace_id +
                    // created_at that make the entry self-locating (A2); the
                    // dying edge is sender→target.
                    let follow_up =
                        build_follow_up_with(em.clone(), em.target.clone(), merged_headers);
                    tracing::warn!(
                        sender = %from.as_str(),
                        target = %em.target.as_str(),
                        trace_id = %em.trace_id,
                        reason = "NoRoute",
                        "cell emission matched no out-edge — dead-letter (no_route)"
                    );
                    push_dead_letter(
                        &mut dead_letters,
                        DeadLetter {
                            sender_path: from.clone(),
                            original_target: em.target.clone(),
                            resolved_target: em.target.clone(),
                            message: follow_up,
                            reason: crate::dead_letter::DeadLetterReason::NoRoute,
                        },
                    );
                    continue;
                }
                let decisions: Vec<EdgeDecision> = matched;

                for dec in decisions {
                    let restores = dec.restore_ttl;
                    let mut follow_up = build_follow_up_with(em.clone(), dec.target, dec.headers_out);
                    // GH #82: a restoring edge lifts the follow-up's routing budget
                    // back to `colony.json message_default_ttl`. Post-build, outside
                    // the frozen corridor — Colony stays the sole envelope setter.
                    if restores {
                        follow_up.ttl = restore_edge_ttl(follow_up.ttl, colony_config.message_default_ttl);
                    }
                    let mut work: VecDeque<(Path, Message)> = VecDeque::new();
                    work.push_back((from.clone(), follow_up));
                    while let Some((s, m)) = work.pop_front() {
                        match route_with_log(&mut registry, &hive_scopes, &mut dead_letters, &colony_db.writer_tx, s, m, &blob_store, colony_config.blob_inline_max_bytes).await {
                            RouteAction::Done => {}
                            RouteAction::Cascade { sender, msg } => {
                                work.push_back((sender, msg));
                            }
                            RouteAction::ColonyDispatch { endpoint, msg, sender } => {
                                // T3: the real dispatcher — pre-extract every `&ColonyDb` sub-ref
                                // SYNCHRONOUSLY, then await. ColonyDb is !Sync → no
                                // `&ColonyDb` across an .await boundary. Templates
                                // snapshot + pre-extracted template rows + rescan-future
                                // prologue all obtained from the sync borrow.
                                let templates_rows = colony_db.read_templates().unwrap_or_default();
                                let templates_snapshot = crate::templates::TemplatesRegistry::from_entries(
                                    templates_rows.clone().into_iter()
                                        .map(|r| crate::templates::TemplateEntry {
                                            template_id: r.template_id,
                                            name: r.name,
                                            version: r.version,
                                            filesystem_path: std::path::PathBuf::from(r.filesystem_path),
                                        }).collect(),
                                );
                                let rescan_future = Box::pin(crate::colony_dispatch::handle_rescan_templates(&colony_db, &root));
                                let db_path = colony_db.db_path().to_path_buf();
                                let follow = crate::colony_dispatch::dispatch_colony_endpoint(
                                    &mut registry, &mut hive_scopes, &mut edges, &mut node_contracts, &mut dead_letters,
                                    &colony_db.writer_tx, &db_path,
                                    templates_snapshot, templates_rows, rescan_future,
                                    &factories, &root,
                                    &inbox_self_tx, &outputs_tx,
                                    endpoint, msg, sender,
                                    colony_config.idle_timeout_default_ms,
                                    colony_config.message_timeout_default_ms,
                                    colony_config.mailbox_default_capacity,
                                    colony_config.strict_validation,
                                    blob_store.clone(),
                                    colony_config.blob_inline_max_bytes,
                                    env_source.as_deref(),
                                ).await;
                                enqueue_dispatch_follow(&mut work, &mut dead_letters, follow);
                            }
                            RouteAction::HiveTransit { hive_path, msg } => {
                                enqueue_hive_transit(&mut work, &mut dead_letters, &edges, hive_path, msg, egress_tx.as_ref(), colony_config.message_default_ttl);
                            }
                        }
                    }
                }
            }
            // Deep-Audit F3: heartbeat wake. Last in the biased select → fires only
            // when inbox/outputs are idle, waking the loop ~10×/s so the top-of-loop
            // heartbeat keeps flowing during quiescence. Guarded by `is_some()` so a
            // watchdog-less spawn (`heartbeat_tx == None`, tests) keeps byte-identical
            // select behaviour. Body empty: the liveness emit happens at loop top.
            _ = heartbeat_interval.tick(), if heartbeat_tx.is_some() => {}
            else => break,
        }
        // W6d (A6): flush this iteration's DLQ pushes — from the inbox arms
        // (Route/Sleep/Stopped/DeadLetterMessage + the byte-frozen `route()`
        // corridor via `route_with_log`) AND the outputs-arm emission routing —
        // into the durable `dead_letters` table. The single-owner `colony_task`
        // is the sole flush authority, so the DB is the one DLQ truth (Read/Drain
        // query the DB, never the now-transient `VecDeque`). The Shutdown arm
        // breaks before reaching here and drains separately above.
        persist_dead_letters(&mut dead_letters, &colony_db.writer_tx).await;
    }
}

/// Central routing function. Per spec § Routing-Symmetrie both the Route-arm and
/// the Outputs-arm funnel through here.
/// Does (0) TTL-check/decrement, (1) resolution, (2) `/colony`-endpoint escape-hatch,
/// (3) registry lookup.
///
/// Returns `None` in all terminal cases (dead-letter, successful send, colony endpoint).
/// Returns `Some((next_sender, next_msg))` for iterative cascade (Task 10+).
///
/// Signature and body are byte-identical to Phase-4-done. Message-log emission
/// is handled by the caller via `route_with_log` (E11, Phase-5-fix).
///
/// `#[rustfmt::skip]`: this corridor is frozen against a character-exact tag gate
/// (A6 fixture `expected_route_body.txt`) — rustfmt must not reformat it.
#[rustfmt::skip]
async fn route(
    registry: &HashMap<Path, RegistryEntry>,
    hive_scopes: &HiveScopeTable,
    dead_letters: &mut VecDeque<DeadLetter>,
    sender_path: Path,
    msg: Message,
) -> RouteAction {
    // TTL-Dekrement zuerst (Spec § TTL-Semantik: "Colony dekrementiert
    // bei jeder Routing-Entscheidung"). ttl == 0 vor Lookup → TtlExpired.
    if msg.ttl == 0 {
        tracing::warn!(
            sender = %sender_path.as_str(),
            target = %msg.target.as_str(),
            trace_id = %msg.trace_id,
            reason = "TtlExpired",
            "ttl exhausted dead-letter"
        );
        push_dead_letter(
            dead_letters,
            DeadLetter {
                sender_path,
                original_target: msg.target.clone(),
                resolved_target: msg.target.clone(),
                message: msg,
                reason: crate::dead_letter::DeadLetterReason::TtlExpired,
            },
        );
        return RouteAction::Done;
    }
    let msg = Message {
        ttl: msg.ttl - 1,
        ..msg
    };

    let original_target = msg.target.clone();
    let resolved = Path::resolve(&sender_path, msg.target.as_str());

    // /colony bare → DLQ-only (spec Z.410: nicht adressierbar).
    if resolved.as_str() == "/colony" {
        handle_colony_target(dead_letters, sender_path, original_target, resolved, msg);
        return RouteAction::Done;
    }
    // /colony/<endpoint> → defer to outputs-arm-dispatch (state-rich callsite).
    // sender + endpoint + msg werden mitgegeben, damit der callsite Pre-T2-Verhalten
    // exakt spiegeln kann (Stub callt handle_colony_target unverändert) oder ab T3
    // den echten Dispatcher aufruft.
    if resolved.starts_with("/colony/") {
        return RouteAction::ColonyDispatch { endpoint: resolved, msg, sender: sender_path };
    }

    match registry.get(&resolved) {
        Some(entry) => {
            let routed = Message {
                target: resolved,
                ..msg
            };
            if let Err(e) = entry.handle.send(routed).await {
                tracing::warn!(target = %e.0.target.as_str(), "route send failed (receiver dropped)");
            }
            RouteAction::Done
        }
        None => {
            if hive_scopes.get(&resolved).is_some() { return RouteAction::HiveTransit { hive_path: resolved, msg }; }
            match handle_unresolved(dead_letters, sender_path, original_target, resolved, msg) {
            Some((sp, m)) => RouteAction::Cascade { sender: sp, msg: m },
            None => RouteAction::Done,
        }},
    }
}

/// Wrapper around `route` that emits an `InsertMessageLog` op to the writer
/// channel for every successfully routable hop (E11, Phase-5-fix).
///
/// Takes `log_tx: &tokio::sync::mpsc::Sender<ColonyWriteOp>` (Send+Sync) so the
/// async frame stays Send. The borrow ends at `.send(...).await` return; only
/// owned values (e.g. oneshot::Receiver) cross the await boundary.
///
/// Phase-12-Pre: `.send().await` propagates cooperative backpressure backwards
/// into the routing loop when the writer channel is full. `route()` itself stays
/// byte-identical (it does not send; tripwire diff gate against phase-10c-done).
/// Phase-13.5 A8 (F2/F5): auto-offload an oversized inline body to a `Body::Blob`
/// before the message is logged + delivered. Spec Z.1361 (write-rule) + A1
/// canonical `>=` threshold (`Body`-enum Z.893): an inline body whose serialized
/// length is `>= threshold` is written via `write_streaming` and the body is
/// replaced by `Body::Blob(uuid)`. Idempotent: an existing `Body::Blob` is left
/// untouched (the per-hop re-check is a cheap `to_vec`-len). With no store wired
/// (`None`) the body always passes through unchanged. A write failure keeps the
/// body inline (best-effort — the delivery boundary still sees a usable body).
async fn offload_oversized(
    mut msg: Message,
    blob_store: &Option<std::sync::Arc<crate::DiskBlobStore>>,
    blob_inline_max_bytes: usize,
) -> Message {
    // Idempotent skip: a Body already offloaded stays a blob (transit re-check).
    let meclaw_core::Body::Inline(value) = &msg.body else {
        return msg;
    };
    let Some(store) = blob_store else {
        return msg;
    };
    let bytes = match meclaw_core::serde_json::to_vec(value) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "offload: body serialize failed — keeping inline");
            return msg;
        }
    };
    if bytes.len() < blob_inline_max_bytes {
        return msg;
    }
    match store
        .write_streaming(bytes.as_slice(), "application/json", None)
        .await
    {
        Ok(blob_ref) => {
            msg.body = meclaw_core::Body::Blob(blob_ref.blob_id);
        }
        Err(e) => {
            tracing::error!(error = %e, "offload: blob write failed — keeping inline");
        }
    }
    msg
}

#[allow(clippy::too_many_arguments)]
async fn route_with_log(
    registry: &mut HashMap<Path, RegistryEntry>,
    hive_scopes: &HiveScopeTable,
    dead_letters: &mut VecDeque<DeadLetter>,
    log_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    sender_path: Path,
    msg: Message,
    blob_store: &Option<std::sync::Arc<crate::DiskBlobStore>>,
    blob_inline_max_bytes: usize,
) -> RouteAction {
    let resolved_target = Path::resolve(&sender_path, msg.target.as_str());
    let is_colony_endpoint =
        resolved_target.as_str() == "/colony" || resolved_target.starts_with("/colony/");
    // `pre_routable` gates the Wake-Pre-Send (registry-only — hives have no
    // cell-task to wake). Unchanged from Phase 13.
    let pre_routable =
        !is_colony_endpoint && msg.ttl > 0 && registry.contains_key(&resolved_target);
    // Phase-13.5-hive-transit (Auflage 1, Variante B): the INCOMING hive-hop is a
    // regular routing hop and gets a message-log row, so the `parent_message_id`
    // chain stays unbroken from source → hive-hop → transit follow-up. Logging is
    // gated separately from `pre_routable` because a hive target IS loggable but
    // must NOT trigger the Wake-Pre-Send (it has no cell-task).
    let should_log = !is_colony_endpoint
        && msg.ttl > 0
        && (registry.contains_key(&resolved_target) || hive_scopes.get(&resolved_target).is_some());

    // Phase-13.5 A8 (F2): auto-offload BEFORE the log-row build so `body_kind`
    // is logged correctly (Demo b). Gated on `should_log` — i.e. only for
    // messages bound for a real cell or hive-transit (NOT colony endpoints,
    // whose bodies are control-plane payloads parsed inline, and NOT unresolved
    // targets). The blob ref then rides transparently through transit/DLQ (F4);
    // the cell-delivery boundary resolves it back to inline (T6, Z.1363).
    let msg = if should_log {
        offload_oversized(msg, blob_store, blob_inline_max_bytes).await
    } else {
        msg
    };

    let log_row_opt = if should_log {
        // Source-Messages (parent_message_id IS NULL) get the @external sentinel.
        let from_path = if msg.parent_message_id.is_none() {
            "@external".to_string()
        } else {
            sender_path.as_str().to_string()
        };
        // ttl - 1 because route() will decrement before logging.
        Some(build_message_log_row_from_msg(
            &msg,
            from_path,
            &resolved_target,
        ))
    } else {
        None
    };

    // Phase-13.5 Lifecycle-3b Task 5 (F3, SCOPE 3): inactive-routing short-circuit.
    // BEFORE the Wake-Pre-Send block: if the resolved target exists in the
    // registry AND is `active == false` (and is not a colony endpoint, TTL > 0),
    // dead-letter as `cell_inactive` and stop — `route()` is NOT called, so the
    // inactive cell is neither woken nor sent to. This lives in the WRAPPER, not
    // in `route()` (which stays byte-identical, Tripwire 1). `cell_inactive` is
    // trennscharf to `unresolved_path` (target never existed) and `hive_no_route`
    // (a hive was reachable, the graph just had no onward edge).
    //
    // Order: the inactive-check goes BEFORE the Wake-Pre-Send — an inactive node
    // must never be woken.
    if let Some(entry) = registry.get(&resolved_target)
        && !entry.active
        && !is_colony_endpoint
        && msg.ttl > 0
    {
        push_dead_letter(
            dead_letters,
            DeadLetter {
                sender_path,
                original_target: msg.target.clone(),
                resolved_target,
                message: msg,
                reason: crate::dead_letter::DeadLetterReason::CellInactive,
            },
        );
        return RouteAction::Done;
    }

    // Phase-13 wake-pre-send (slice 13-I-1): before `route()` does the send,
    // call the WakeFn on Asleep/NotYetSpawned and hand the receiver over to the
    // cell task. The status is set to Awake. Sync ops only — no `.await` between
    // the status set and the subsequent `route(...)` call that does the
    // `entry.handle.send(msg).await`. `route()` itself stays byte-identical to
    // phase-12-done (tripwire 1).
    //
    // Gate: only when `pre_routable` (no colony endpoint, TTL > 0, target exists
    // in the registry) — otherwise `route()` would not send anyway, and a status
    // flip would be wrong.
    //
    // Today (phase-13-I-1) the wake arm is logically correct but practically
    // never executed: every cell is registered as `Awake` on the active path
    // (variant a, 13-G-2). Wake goes live from 13-K-2 (stateful → dormant +
    // NotYetSpawned initial status).
    if pre_routable && let Some(entry) = registry.get_mut(&resolved_target) {
        // F1-KH2 Schicht 2 (defense-in-depth): a PARKED cell whose registration
        // carries NO wake mechanic (`wake == None` — eager kinds, exceptional
        // fallbacks) can never be woken by a delivery. Fail LOUDLY instead of
        // silently: dead-letter the message (`cell_inactive`) and leave the
        // parked status untouched (NO false `Awake`). Checked BEFORE the status
        // mem::replace so the parked receiver is never consumed. Pre-fix this
        // was an inert closure that dropped the receiver — silent loss with a
        // lying lifecycle status (K-H2 finding F1).
        if entry.wake.is_none() && !matches!(entry.status, CellStatus::Awake) {
            tracing::error!(
                target = %resolved_target.as_str(),
                "delivery to a parked cell without a wake mechanic — \
                 dead-lettering (cell_inactive) instead of waking"
            );
            push_dead_letter(
                dead_letters,
                DeadLetter {
                    sender_path,
                    original_target: msg.target.clone(),
                    resolved_target,
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::CellInactive,
                },
            );
            return RouteAction::Done;
        }
        match std::mem::replace(&mut entry.status, CellStatus::Awake) {
            CellStatus::Awake => {} // no-op
            CellStatus::Asleep { receiver } | CellStatus::NotYetSpawned { receiver } => {
                // Phase-13.5 Lifecycle-3b Task 7.5: the WakeFn spawns the cell-
                // task and returns its live `(stop_tx, death_ack_rx)`. Store them
                // so a later disconnect can peace-stop the woken cell + drain its
                // mailbox remainder to the DLQ (overview Z.1434/Z.1448). Sync
                // field assignment only — no `.await`, route() stays byte-
                // identical.
                //
                // A parked entry reaching this arm has a real wake — the
                // `None`-wake guard above dead-letters and returns first.
                if let Some(wake) = entry.wake.as_ref() {
                    let (stop_tx, death_ack_rx) = wake(receiver);
                    entry.stop_tx = Some(stop_tx);
                    entry.death_ack_rx = Some(death_ack_rx);
                }
            }
        }
    }

    // GH #82: TTL exhaustion is terminal by spec — `route()` puts the message
    // straight into the dead-letter queue and deliberately bypasses the
    // `reply_to` cascade, so NOTHING is emitted toward the turn's origin. Inside
    // a fan-in that reads as a silent stall: the collector never completes, the
    // caller waits out its own timeout, and the topology has nothing to route
    // on. The corridor's own line is terse and frozen (`warn`, no message id);
    // this pre-check is its loud, identifying twin, and it says what the budget
    // is for. Pre-check before the call, nothing added inside the corridor —
    // the `route()` gate stays byte-identical.
    if msg.ttl == 0 {
        tracing::error!(
            message_id = %msg.id,
            sender = %sender_path.as_str(),
            target = %resolved_target.as_str(),
            trace_id = %msg.trace_id,
            reason = "TtlExpired",
            "message died of TTL exhaustion — terminal, direct to the dead-letter \
             queue, nothing reaches the origin. Size colony.json message_default_ttl \
             to the hop cost of the topology (one store-backed tool round costs about \
             a dozen routing hops) and bound a tool loop with an iteration counter in \
             context, not with TTL"
        );
    }

    let next = route(registry, hive_scopes, dead_letters, sender_path, msg).await;

    if let Some(row) = log_row_opt {
        let _ = log_tx
            .send(crate::persist::writer::ColonyWriteOp::InsertMessageLog(row))
            .await;
    }
    next
}

/// Befund 8 — sweep the filesystem residue of a POST-RENAME mutation reject so
/// the reject is spurless (spec § Validation l.279: "no partial commit").
///
/// `apply_mutation` renames ALL staged cells into their final paths BEFORE the
/// spawn loop, so a spawn reject mid-loop leaves the not-yet-registered cells
/// stranded in `{root}` — and the identical follow-up mutation would then see a
/// stranded directory as a resume (overview Z.170-180) and commit without a
/// real spawn/registry. Every `StagedDir` is a FRESH instantiation (a resume
/// produces NO `StagedDir`: `stage.rs` skips an existing `final_path`), so a
/// staged cell that is NOT in the registry was created by THIS rejected
/// mutation and is safe to remove — this does not touch any live cell, so the
/// No-Delete-Policy (which governs registered cells) is not engaged. The
/// already-registered staged cells (earlier loop iterations) stay
/// registry/FS-consistent. The `.staging/<id>` dir is always swept.
fn sweep_reject_residue(
    staged: &[crate::mutation::stage::StagedDir],
    registry: &HashMap<Path, RegistryEntry>,
    root: &std::path::Path,
    id: &str,
) {
    for sd in staged {
        // A5b 2b (Phase-16 W1b): never delete a PRE-EXISTING adoption target. A
        // fresh add_nodes/swap residue (`preexisting_target == false`) was just
        // renamed in from staging and is safe to remove on reject. An `adopt`
        // entry's `final_path` is the builder's pre-placed directory (with its
        // `cell.db`) — removing it would be a No-Delete-Policy violation + data
        // loss. The adopted dir stays; only the staging residue is swept below.
        if !sd.preexisting_target && !registry.contains_key(&sd.absolute_path) {
            let _ = std::fs::remove_dir_all(&sd.final_path);
        }
    }
    let _ = std::fs::remove_dir_all(root.join(".staging").join(id));
}

/// Inert RespawnFn for the exceptional subtree fallbacks (no factory /
/// spawn error). Never invoked for a parked, non-eager cell.
fn subtree_inert_respawn() -> crate::RespawnFn {
    Box::new(|| {
        tracing::error!(
            "inert RespawnFn invoked on a subtree inactive non-eager cell — \
             should never happen (parked NotYetSpawned, reconnect is \
             wake-on-message). No-op."
        );
        let (s, _r) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        let (_pt, pr) = tokio::sync::oneshot::channel::<()>();
        let (_bt, br) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(async {});
        (s, join, pr, br)
    })
}

/// Phase-6 T13: Mutation request handler.
///
/// Pipeline (current scope): substitute → validate. On success returns
/// `Committed` with a fresh mutation id; T14 wires the durable `in_flight` /
/// `committed` log-row updates. On substitute or validate failure: sends an
/// EDA error-reply via `route_with_log` (if `reply_to` is set) and returns
/// `Rejected`. Phase-16 W3 (A6): a validate-stage reject now ALSO writes a
/// durable `status='rejected'` `mutation_log` row (via `send_eda_reject`) — the
/// reject is no longer invisible in the `/colony/mutations` audit, while the
/// synchronous reject-reply is unchanged. (It never gets an `in_flight` row;
/// `in_flight`/`failed` belong to the post-validate Apply stage.)
///
/// All error paths use `return MutationOutcome::Rejected { ... }`; no flag
/// patterns, no labelled continues (correction 1).
///
/// **Phase-5 borrow-pattern**: the `ColonyDb` itself is non-Send (rusqlite
/// `Connection` is not Sync), so it is NEVER borrowed across `.await`. The
/// arm passes `&colony_db.writer_tx` (clonable, Send+Sync) instead.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_mutation(
    registry: &mut HashMap<Path, RegistryEntry>,
    hive_scopes: &mut HiveScopeTable,
    edges: &mut crate::edge_table::EdgeTable,
    node_contracts: &mut HashMap<Path, NodeContract>, // Hardening Slice 1 (Task 1.4) — 14-B live source + mutation-spawn fill
    dead_letters: &mut VecDeque<DeadLetter>,
    log_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    templates: crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    root: &std::path::Path,
    inbox_self_tx: &mpsc::Sender<ColonyMsg>,
    outputs_tx: &mpsc::Sender<CellEmission>,
    payload: meclaw_core::JsonValue,
    reply_to: Option<Path>,
    trace_id: Uuid,
    parent_message_id: Uuid,
    idle_default_ms: u64, // Phase-13.5 A7 — colony.json idle-default for mutation-spawn
    message_timeout_default_ms: u64, // P3-B-plumb-2 — colony.json B-backstop default for mutation-spawn
    mailbox_default_capacity: usize, // Paket-1 T20 — colony.json mailbox-default for mutation-spawn
    strict_validation: bool, // paket-7 B5 — colony.json strict_validation for mutation-spawn validate_emits resolution
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>, // Phase-13.5 A8 — delivery-boundary resolution
    blob_inline_max_bytes: usize, // Phase-13.5 A8 (F2) — offload threshold for EDA error-reply paths
    env_source: Option<&std::path::Path>, // U8 (RULED A8) — the env source remembered from startup; None ⇒ default `<root>/.env`
    death_ack_wait_tx: Option<&mpsc::Sender<()>>, // test-only deterministic sync hook; None in production (byte-identical prod path)
) -> crate::mutation::MutationOutcome {
    use crate::mutation::MutationOutcome;

    let id = Uuid::now_v7().to_string();
    let diff_raw = payload
        .get("diff")
        .cloned()
        .unwrap_or(meclaw_core::serde_json::Value::Object(Default::default()));
    let ctx: std::collections::HashMap<String, String> = payload
        .get("ctx")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.into())))
                .collect()
        })
        .unwrap_or_default();
    // U8 (RULED A8): the env source remembered from colony startup. Identical to
    // the boot substitution (`plan_bootstrap_with_env`): the override wins,
    // otherwise the default `<root>/.env`. That way boot, mutation and 2b
    // adoption read from the same source.
    let env_path = env_source
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| root.join(".env"));
    let env: std::collections::HashMap<String, String> = match crate::env_file::load_env(&env_path)
    {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                path = %env_path.display(),
                error = ?e,
                "load .env failed; using empty map"
            );
            std::collections::HashMap::new()
        }
    };

    // Step 1: substitute. GH #20 -- class-split: the two slots that are written
    // into an instance `config.json` (`add_nodes[].override_params`,
    // `swap_nodes[].with.params`) keep their environment placeholders literally;
    // the rest of the diff is fully substituted as before.
    let diff_subst =
        match crate::mutation::substitute::substitute_mutation_diff(&diff_raw, &env, &ctx) {
            Ok(d) => d,
            Err(err) => {
                send_eda_reject(
                    &id,
                    &err,
                    reply_to.as_ref(),
                    trace_id,
                    parent_message_id,
                    registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    &blob_store,
                    blob_inline_max_bytes,
                    &payload,
                )
                .await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: err.error_code().into(),
                    details: format!("{err:?}"),
                };
            }
        };

    // Step 1a (Phase-13.5 Lifecycle-3a, Auflagen A1/A2): Resume-Detect + Awake-Guard.
    // `add_nodes` at an EXISTING path is a Reconnect/Resume (overview Z.170-180),
    // not an instantiation. Resume is decided by FS-existence of the target dir
    // (mirrors `build_staging_tree_from_templates`, which skips existing
    // final_paths). For each Resume target:
    //   - status `Awake` (running task) → reject `resume_requires_stopped_cell`:
    //     a running task cannot race-free release its live `cell.db` (A2). No
    //     apply, no FS-touch — registry + cell.db + FS stay untouched.
    //   - otherwise (NotYetSpawned/Asleep or no registry entry) → collect the
    //     short-name in `resume_names`. These are excluded from the
    //     naming-collision validation below: Resume at an existing name is legal
    //     (the post-state node is the SAME node, identity preserved), not a
    //     duplicate. Staging + spawn skip the node automatically (no config
    //     rewrite, A1).
    let guard_scope = payload.get("scope").and_then(|v| v.as_str()).unwrap_or("/");
    let mut resume_names: Vec<String> = Vec::new();
    // Paket 6 Block D: single-cell `add_nodes`-Resume targets. A single-cell
    // Resume produces NO `StagedDir` (stage.rs skips an existing final_path), so
    // — unlike the subtree path's `subtree.existing` — it would never seed the
    // recompute `involved` set. A Resume IS a direct diff-address of the node
    // (overview Z.170-180), so the sticky discriminator must treat it as such:
    // collect the resolved target paths here and seed them into `involved` once
    // it is declared (step 9), so a `failed` cell resumed on its own path is
    // reactivated (cleared + reset) exactly like an `add_edges` reconnect.
    let mut resume_targets: Vec<Path> = Vec::new();
    if let Some(adds) = diff_subst.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let Some(name) = n.get("name").and_then(|v| v.as_str()) else {
                continue; // schema-validate (Step 2) reports the missing name.
            };
            // Spec overview Z.331: anchor the logical path under the single root
            // cell directory (the root-cell-dir name is stripped from logical
            // paths). Shared with `mutation/stage.rs` via `path_truth` so Resume-
            // detect checks the SAME directory bootstrap instantiated under.
            let final_path = crate::path_truth::resolve_cell_dir(root, guard_scope, name);
            // A5b 2b (Phase-16 W1b, Ruling 2026-06-12): an `add_nodes` entry with
            // an `adopt` block is an ADOPTION of an existing UNregistered on-disk
            // node — never a resume. The pure grammar (adopt object + mandatory
            // `type`, mutually exclusive with `template`) is enforced in the
            // validate stage; here we run the FS/registry-dependent, pre-destructive
            // checks: the path must exist (something to adopt), must be
            // unregistered (a registered path uses resume), and the on-disk
            // identity must match the declared expectation — `cell.type`
            // mandatory, `contract.version` optional (resume_type_mismatch-analog,
            // own reason text). A malformed adopt (non-object / missing `type`)
            // falls through to the validate stage's `schema` reject; we only
            // ensure it is NOT mis-treated as a resume. Staging then instantiates
            // it from the existing dir (same pipeline minus template lookup/copy).
            if n.get("adopt").is_some() {
                if let Some(adopt) = n.get("adopt").and_then(|v| v.as_object()) {
                    let target = crate::mutation::resolve_scoped_path(guard_scope, name);
                    let adopt_err: Option<crate::mutation::MutationError> = if !final_path.exists()
                    {
                        Some(crate::mutation::MutationError::Schema(format!(
                            "adopt: no existing node at {} to adopt",
                            target.as_str()
                        )))
                    } else if registry.contains_key(&target) {
                        Some(crate::mutation::MutationError::Schema(format!(
                            "adopt: {} is already registered (adoption is for unregistered \
                             paths only)",
                            target.as_str()
                        )))
                    } else if let Some(exp) = adopt.get("type").and_then(|v| v.as_str()) {
                        let actual = crate::mutation::subtree::read_on_disk_cell_type(&final_path);
                        if actual.as_deref() != Some(exp) {
                            Some(crate::mutation::MutationError::ResumeTypeMismatch(format!(
                                "{}: adopt expected type '{exp}', on-disk {actual:?}",
                                target.as_str()
                            )))
                        } else if let Some(exp_ver) = adopt.get("version").and_then(|v| v.as_str())
                        {
                            let actual_ver =
                                crate::mutation::subtree::read_on_disk_contract_version(
                                    &final_path,
                                );
                            if actual_ver.as_deref() != Some(exp_ver) {
                                Some(crate::mutation::MutationError::ResumeTypeMismatch(format!(
                                    "{}: adopt expected version '{exp_ver}', on-disk {actual_ver:?}",
                                    target.as_str()
                                )))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None // missing `type` → validate stage rejects as `schema`.
                    };
                    if let Some(err) = adopt_err {
                        send_eda_reject(
                            &id,
                            &err,
                            reply_to.as_ref(),
                            trace_id,
                            parent_message_id,
                            registry,
                            hive_scopes,
                            dead_letters,
                            log_tx,
                            &blob_store,
                            blob_inline_max_bytes,
                            &payload,
                        )
                        .await;
                        return MutationOutcome::Rejected {
                            id: Some(id),
                            error_code: err.error_code().into(),
                            details: format!("{err:?}"),
                        };
                    }
                }
                continue; // adopt is never a resume.
            }
            if !final_path.exists() {
                continue; // fresh instantiation — normal path.
            }
            let target = crate::mutation::resolve_scoped_path(guard_scope, name);
            if matches!(
                registry.get(&target).map(|e| &e.status),
                Some(CellStatus::Awake)
            ) {
                let err = crate::mutation::MutationError::ResumeRequiresStoppedCell(
                    target.as_str().to_string(),
                );
                send_eda_reject(
                    &id,
                    &err,
                    reply_to.as_ref(),
                    trace_id,
                    parent_message_id,
                    registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    &blob_store,
                    blob_inline_max_bytes,
                    &payload,
                )
                .await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: err.error_code().into(),
                    details: format!("{err:?}"),
                };
            }
            // Paket-5 T11 (F2-Ruling): resume type-compat. The existing node is a
            // Resume target (path exists, cell not Awake). Its on-disk `cell.type`
            // MUST match the template being resumed — a type change at the same
            // path would silently reinterpret the preserved `cell.db`/identity.
            // Both reads are pre-destructive (nothing has been staged or renamed),
            // so the reject leaves registry + cell.db + FS untouched. Checked
            // AFTER the cheap Awake registry lookup, BEFORE any apply. The shared
            // helper `resume_type_compatible` is reused by the subtree path (T12).
            let tpl_ref = n.get("template").and_then(|v| v.as_str()).unwrap_or("");
            if let Ok(entry) = templates.resolve(tpl_ref) {
                let template_type =
                    crate::mutation::subtree::read_on_disk_cell_type(&entry.filesystem_path);
                let existing_type = crate::mutation::subtree::read_on_disk_cell_type(&final_path);
                if let (Some(existing), Some(template)) = (&existing_type, &template_type)
                    && !crate::mutation::resume_type_compatible(existing, template)
                {
                    let err = crate::mutation::MutationError::ResumeTypeMismatch(
                        target.as_str().to_string(),
                    );
                    send_eda_reject(
                        &id,
                        &err,
                        reply_to.as_ref(),
                        trace_id,
                        parent_message_id,
                        registry,
                        hive_scopes,
                        dead_letters,
                        log_tx,
                        &blob_store,
                        blob_inline_max_bytes,
                        &payload,
                    )
                    .await;
                    return MutationOutcome::Rejected {
                        id: Some(id),
                        error_code: err.error_code().into(),
                        details: format!("{err:?}"),
                    };
                }
            }
            resume_names.push(name.to_string());
            resume_targets.push(target);
        }
    }

    // Step 1b: lazy check (overview Z.1163 path 2, T18).
    // For each add_nodes entry that references a template: if the registry knows
    // the template but its filesystem_path no longer exists on disk, fire a
    // fire-and-forget RemoveTemplate write op and immediately reject.
    if let Some(adds) = diff_subst.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let tpl_ref = n.get("template").and_then(|v| v.as_str()).unwrap_or("");
            if tpl_ref.is_empty() {
                continue;
            }
            if let Ok(entry) = templates.resolve(tpl_ref)
                && !entry.filesystem_path.exists()
            {
                tracing::warn!(
                    template = %tpl_ref,
                    path = ?entry.filesystem_path,
                    "lazy-check: filesystem_path gone, auto-removing"
                );
                let _ = log_tx
                    .send(crate::persist::writer::ColonyWriteOp::RemoveTemplate {
                        template_id: entry.template_id.clone(),
                        ack: None,
                    })
                    .await;
                let err = crate::mutation::MutationError::TemplateMissing(tpl_ref.to_string());
                send_eda_reject(
                    &id,
                    &err,
                    reply_to.as_ref(),
                    trace_id,
                    parent_message_id,
                    registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    &blob_store,
                    blob_inline_max_bytes,
                    &payload,
                )
                .await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: err.error_code().into(),
                    details: format!("{err:?}"),
                };
            }
            // Err from resolve: fall through — validate will catch it as template_missing.
        }
    }

    // Step 2: validate (post-state checks against current registry + edges).
    // Phase-11 T17: additive validation via validate_post_state_with_templates.
    // Build template→cell.type mapping from each template's config.json (sync FS
    // read; skip on IO/parse error — validator will reject as TemplateMissing).
    // Phase-13.5 Lifecycle-3a (A1): exclude Resume targets (Step 1a) from the
    // registry-name set so the naming-collision check does NOT flag a Resume at
    // an existing name as a duplicate. Resume keeps the identity — the existing
    // node IS the "new" node; the post-state still has the name exactly once via
    // `add_names`. A name not hit by a Resume-add stays and collides normally.
    // paket-2 T1 A2 scope-binding: only names whose full path lives DIRECTLY
    // within `guard_scope` (parent path == guard_scope) are visible to the
    // validator. A node at `/other/foo` does NOT contribute `"foo"` when the
    // mutation scope is `/main` — preventing cross-scope `with.name` slipthrough.
    let scope_prefix = canonical_scope_prefix(guard_scope);
    let registry_names: Vec<String> = registry
        .keys()
        .filter(|p| {
            // Retain only paths whose parent is exactly `scope_prefix`.
            // e.g. scope="/main", path="/main/foo" → parent="/main" ✓
            //      scope="/main", path="/other/foo" → parent="/other" ✗
            //      scope="/",     path="/foo"        → parent="/"     ✓
            let s = p.as_str();
            if let Some(last_slash) = s.rfind('/') {
                let parent = if last_slash == 0 {
                    "/"
                } else {
                    &s[..last_slash]
                };
                parent == scope_prefix
            } else {
                false
            }
        })
        .filter_map(|p| p.as_str().rsplit('/').next().map(|s| s.to_string()))
        .filter(|n| !resume_names.contains(n))
        .collect();
    // Phase 13.5 step-6: hive short-names are valid edge endpoints too — collect
    // them (last path segment, mirroring `registry_names`) so `add_edges` may
    // reference an existing hive symmetrically to a cell.
    // NOTE: hive names are colony-global and intentionally NOT scope-filtered
    // (unlike `registry_names`): hives define transit scopes reachable from
    // anywhere in the colony, so they must be visible regardless of guard_scope.
    let hive_endpoint_names: Vec<String> = hive_scopes
        .paths()
        .filter_map(|p| p.as_str().rsplit('/').next().map(|s| s.to_string()))
        .collect();
    // paket-5 T4 (P10b companion): SCOPE-FILTERED hive short-names for the
    // `swap_nodes` `match.name` existence check. Mirrors the `registry_names`
    // parent==scope_prefix filter above: only hives whose full path lives DIRECTLY
    // within `guard_scope` contribute their short-name. A `match.name` is scope-bound
    // (spec Z.265 "Names are unique per scope"), so a hive in a FOREIGN scope must
    // NOT satisfy a short-name match — otherwise validate passes a node that the
    // scope-correct apply-side (`resolve_scoped_path`) cannot resolve (finding
    // Paket-2-b'). This is DISTINCT from `hive_endpoint_names` (global, for add_edges
    // endpoints, which are scope-relative and may legitimately reference any hive).
    let hive_match_names: Vec<String> = hive_scopes
        .paths()
        .filter(|p| {
            let s = p.as_str();
            if let Some(last_slash) = s.rfind('/') {
                let parent = if last_slash == 0 {
                    "/"
                } else {
                    &s[..last_slash]
                };
                parent == scope_prefix
            } else {
                false
            }
        })
        .filter_map(|p| p.as_str().rsplit('/').next().map(|s| s.to_string()))
        .collect();
    let existing_edges: Vec<(String, String)> = edges
        .iter()
        .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
        .collect();
    // R12: pre-state absolute paths at ANY depth (registry ∪ hive scopes) for
    // the depth-endpoint membership test (spec Z.227: edge paths are
    // scope-relative WITHOUT a depth restriction). Colony-global is safe here:
    // `validate_scope_containment` runs BEFORE the post-state validation and
    // rejects `..`/absolute endpoints, so a resolved depth endpoint is
    // scope-contained by construction.
    let deep_endpoint_paths: Vec<String> = registry
        .keys()
        .map(|p| p.as_str().to_string())
        .chain(hive_scopes.paths().map(|p| p.as_str().to_string()))
        .collect();
    let template_to_cell_type: Vec<(String, String)> = templates
        .entries_iter()
        .filter_map(|t| {
            let cfg_path = t.filesystem_path.join("config.json");
            let raw = std::fs::read_to_string(&cfg_path).ok()?;
            let val: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&raw).ok()?;
            let ct = val
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|v| v.as_str())?;
            Some((t.name.clone(), ct.to_string()))
        })
        .collect();
    // Paket-5 T12 (P9 per-node subtree resume): pre-resolve + pre-destructively
    // validate SUBTREE `add_nodes` BEFORE staging. For each add_nodes entry whose
    // template is a subtree (`cells.len() > 1`):
    //   - `classify_subtree_nodes` (PURE): partition the template nodes against the
    //     live FS into `missing` (instantiate) vs `existing` (resume). This REPLACES
    //     the former F4 `subtree_root_conflict` reject — a subtree at a partially or
    //     fully existing root path is now a per-node resume, not a rejection.
    //   - Awake-Schranke (T7): any `existing` node whose cell is currently `Awake`
    //     → reject `resume_requires_stopped_cell` (spec Z.296), pre-destructive.
    //   - F2 type-compat (T11): each `existing` node's on-disk `cell.type` MUST be
    //     compatible with the template node's `cell.type` at the same rel-path; a
    //     mismatch → reject `resume_type_mismatch` via the SHARED helper.
    //   - `resolve_subtree` (PURE): contribute the subtree's absolute node endpoints
    //     (existing ∪ missing — every template cell) + resolved internal edges to
    //     the validator, so an internal edge referencing an EXISTING node validates
    //     and the merged graph (cycle / out-of-subtree edge) is rejected up front —
    //     leaving NO `.staging` leak and nothing in registry/db on reject.
    let mut all_subtree_node_endpoints: Vec<String> = Vec::new();
    let mut all_subtree_internal_edges: Vec<(String, String)> = Vec::new();
    if let Some(adds) = diff_subst.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let (Some(name), Some(tpl_ref)) = (
                n.get("name").and_then(|v| v.as_str()),
                n.get("template").and_then(|v| v.as_str()),
            ) else {
                continue; // schema-validate below reports the missing field.
            };
            let Ok(tpl) = templates.resolve(tpl_ref) else {
                continue; // validate below rejects as template_missing.
            };
            let is_subtree = crate::mutation::subtree::parse_subtree(&tpl.filesystem_path)
                .map(|t| t.cells.len() > 1)
                .unwrap_or(false);
            if !is_subtree {
                continue;
            }
            // Per-node classification (T6) — partition against the live FS.
            let partition = match crate::mutation::subtree::classify_subtree_nodes(
                root,
                guard_scope,
                name,
                &tpl.filesystem_path,
            ) {
                Ok(p) => p,
                Err(err) => {
                    send_eda_reject(
                        &id,
                        &err,
                        reply_to.as_ref(),
                        trace_id,
                        parent_message_id,
                        registry,
                        hive_scopes,
                        dead_letters,
                        log_tx,
                        &blob_store,
                        blob_inline_max_bytes,
                        &payload,
                    )
                    .await;
                    return MutationOutcome::Rejected {
                        id: Some(id),
                        error_code: err.error_code().into(),
                        details: format!("{err:?}"),
                    };
                }
            };
            // Awake-Schranke (T7): an EXISTING node with a running task cannot be
            // resumed (it cannot race-free release its live `cell.db`). Maps the
            // colony `CellStatus` into the helper's minimal `AwakeState`.
            if let Err(err) =
                crate::mutation::subtree::subtree_resume_awake_check(&partition.existing, |p| {
                    registry.get(p).map(|e| match e.status {
                        CellStatus::Awake => crate::mutation::subtree::AwakeState::Awake,
                        _ => crate::mutation::subtree::AwakeState::NotAwake,
                    })
                })
            {
                send_eda_reject(
                    &id,
                    &err,
                    reply_to.as_ref(),
                    trace_id,
                    parent_message_id,
                    registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    &blob_store,
                    blob_inline_max_bytes,
                    &payload,
                )
                .await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: err.error_code().into(),
                    details: format!("{err:?}"),
                };
            }
            // F2 type-compat (T11): each existing node's on-disk `cell.type` MUST be
            // compatible with the template node's `cell.type`. SAME shared helper as
            // the single-cell resume path. Both reads are pre-destructive.
            for ex in &partition.existing {
                if let (Some(existing_type), Some(template_type)) =
                    (&ex.on_disk_cell_type, &ex.template_cell_type)
                    && !crate::mutation::resume_type_compatible(existing_type, template_type)
                {
                    let err = crate::mutation::MutationError::ResumeTypeMismatch(
                        ex.absolute_path.as_str().to_string(),
                    );
                    send_eda_reject(
                        &id,
                        &err,
                        reply_to.as_ref(),
                        trace_id,
                        parent_message_id,
                        registry,
                        hive_scopes,
                        dead_letters,
                        log_tx,
                        &blob_store,
                        blob_inline_max_bytes,
                        &payload,
                    )
                    .await;
                    return MutationOutcome::Rejected {
                        id: Some(id),
                        error_code: err.error_code().into(),
                        details: format!("{err:?}"),
                    };
                }
            }
            // PURE resolution (no FS writes). Containment failure (edge escaping
            // the subtree root) rejects HERE — before staging.
            match crate::mutation::subtree::resolve_subtree(&tpl.filesystem_path, guard_scope, name)
            {
                Ok(resolved) => {
                    all_subtree_node_endpoints.extend(resolved.node_endpoints);
                    all_subtree_internal_edges.extend(resolved.internal_edges);
                }
                Err(err) => {
                    send_eda_reject(
                        &id,
                        &err,
                        reply_to.as_ref(),
                        trace_id,
                        parent_message_id,
                        registry,
                        hive_scopes,
                        dead_letters,
                        log_tx,
                        &blob_store,
                        blob_inline_max_bytes,
                        &payload,
                    )
                    .await;
                    return MutationOutcome::Rejected {
                        id: Some(id),
                        error_code: err.error_code().into(),
                        details: format!("{err:?}"),
                    };
                }
            }
        }
    }

    // Befund 22: scope-containment guard — reject any scoped name whose resolved
    // path escapes `guard_scope` (e.g. via `..`) BEFORE any FS/registry mutation,
    // parallel to naming_collision/match_no_hit. ScopeOutOfBounds → 422.
    if let Err(err) =
        crate::mutation::validate::validate_scope_containment(&diff_subst, guard_scope)
    {
        send_eda_reject(
            &id,
            &err,
            reply_to.as_ref(),
            trace_id,
            parent_message_id,
            registry,
            hive_scopes,
            dead_letters,
            log_tx,
            &blob_store,
            blob_inline_max_bytes,
            &payload,
        )
        .await;
        return MutationOutcome::Rejected {
            id: Some(id),
            error_code: err.error_code().into(),
            details: format!("{err:?}"),
        };
    }

    if let Err(err) = crate::mutation::validate::validate_post_state_with_templates_scoped(
        &diff_subst,
        &templates,
        factories,
        &registry_names,
        &existing_edges,
        &template_to_cell_type,
        &hive_endpoint_names,
        &hive_match_names,
        &all_subtree_node_endpoints,
        &all_subtree_internal_edges,
        guard_scope,
        &deep_endpoint_paths,
    ) {
        send_eda_reject(
            &id,
            &err,
            reply_to.as_ref(),
            trace_id,
            parent_message_id,
            registry,
            hive_scopes,
            dead_letters,
            log_tx,
            &blob_store,
            blob_inline_max_bytes,
            &payload,
        )
        .await;
        return MutationOutcome::Rejected {
            id: Some(id),
            error_code: err.error_code().into(),
            details: format!("{err:?}"),
        };
    }

    // Paket-5 T1/T2 (P10a / D-031): validate-time reject for malformed / no-hit
    // `remove_edges` patterns — parity with remove_nodes / swap_nodes (Z.272).
    // Build the F6 edge-view (absolute endpoints + stored condition/modifier
    // sources) so validate compares EXACTLY like the apply-time arm below.
    let remove_edges_view: Vec<crate::mutation::validate::EdgeMatchView> = edges
        .iter()
        .map(crate::mutation::validate::EdgeMatchView::from)
        .collect();
    if let Err(err) = crate::mutation::validate::validate_remove_edges(
        &diff_subst,
        guard_scope,
        &remove_edges_view,
    ) {
        send_eda_reject(
            &id,
            &err,
            reply_to.as_ref(),
            trace_id,
            parent_message_id,
            registry,
            hive_scopes,
            dead_letters,
            log_tx,
            &blob_store,
            blob_inline_max_bytes,
            &payload,
        )
        .await;
        return MutationOutcome::Rejected {
            id: Some(id),
            error_code: err.error_code().into(),
            details: format!("{err:?}"),
        };
    }

    // Slice 1 (roadmap Z.138): 14-B header-contract locality on the FULL
    // hypothetical post_state — the same pure check the bootstrap runs, fed
    // from live views instead of config.json. Pre-destructive: reject before
    // staging/FS/registry are touched.
    match crate::mutation::header_views::build_post_state_header_views(
        &*node_contracts,
        &*edges,
        &diff_subst,
        guard_scope,
        &templates,
        hive_scopes,
    ) {
        Ok((post_nodes, post_edges, post_hives)) => {
            if let Err(err) = crate::mutation::validate::validate_header_contract_locality(
                &post_nodes,
                &post_edges,
                &post_hives,
            ) {
                send_eda_reject(
                    &id,
                    &err,
                    reply_to.as_ref(),
                    trace_id,
                    parent_message_id,
                    registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    &blob_store,
                    blob_inline_max_bytes,
                    &payload,
                )
                .await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: err.error_code().into(),
                    details: format!("{err:?}"),
                };
            }
        }
        Err(err) => {
            send_eda_reject(
                &id,
                &err,
                reply_to.as_ref(),
                trace_id,
                parent_message_id,
                registry,
                hive_scopes,
                dead_letters,
                log_tx,
                &blob_store,
                blob_inline_max_bytes,
                &payload,
            )
            .await;
            return MutationOutcome::Rejected {
                id: Some(id),
                error_code: err.error_code().into(),
                details: format!("{err:?}"),
            };
        }
    }

    // Apply sequence step 5: durable in_flight insert.
    // The `&log_tx` borrow ends at `.send(...)` return; only `rx` (Send) crosses
    // the await — keeping the `colony_task` Future Send on multi_thread runtimes.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let payload_json = meclaw_core::serde_json::to_string(&payload).unwrap_or_default();
    let scope = payload
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("/")
        .to_string();

    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        log_tx
            .send(crate::persist::writer::ColonyWriteOp::MutationLogInsert {
                id: id.clone(),
                scope: scope.clone(),
                payload_json,
                created_at: now,
                ack: Some(tx),
            })
            .await
            .expect("writer thread dead");
        if let Err(e) = rx.await {
            tracing::error!(error = ?e, "durable in_flight insert failed");
            return MutationOutcome::Rejected {
                id: Some(id),
                error_code: "db".into(),
                details: format!("{e:?}"),
            };
        }
    }

    // Apply sequence steps 6+7 (T17): stage + atomic rename.
    // Steps 9 (spawn) + 10 (edges) come in T18 + T21.
    // The templates snapshot was built by the caller (colony_task) and passed in as a
    // parameter (phase-11 T16: ColonyDb is !Sync, hence no &ColonyDb across an
    // .await boundary).
    let (staged, staged_subtrees) = match crate::mutation::apply::apply_mutation(
        root,
        &id,
        &scope,
        &diff_subst,
        &templates,
        &env,
        &ctx,
    ) {
        Ok(s) => s,
        Err(crate::mutation::MutationError::LiveTreeMutated(detail)) => {
            // Deep-Audit F2: a `rename(2)` failed AFTER an earlier one already
            // landed — the live tree is PARTIALLY mutated (renames 1..i committed,
            // i+1..N not), no rollback. This is NOT the clean pre-destructive
            // reject below; reporting `Rejected{live tree untouched}` would lie.
            // Strict-fail loudly: panic the colony_task. The half-state surfaces on
            // the next boot as unregistered orphan dirs (bootstrap orphan-report),
            // never silently adopted. mutation_log stays `in_flight` (the boot
            // signal — NOT updated to `failed`, unlike the clean-reject path). The
            // panic tears the colony_task → MeClaw stops (F3 heartbeat-watchdog
            // detects the lost heartbeat).
            panic!(
                "mid-rename strict-fail (mutation {id}): live tree partially mutated, no rollback: {detail}"
            );
        }
        Err(e) => {
            // Pre-destructive reject (paket-1 T20 / Phase-6 recovery-audit): a
            // staging failure happens BEFORE the atomic rename, so the live tree
            // is untouched. Discard the half-built `.staging/<id>/` tree right
            // away so no debris survives a reject (a `cell.mailbox_size: 0`
            // reject, for example, aborts mid-`build_staging_tree_from_templates`
            // after the template dir was copied). Boot-recovery would also sweep
            // this, but cleaning up here keeps the reject leak-free immediately.
            let _ = std::fs::remove_dir_all(root.join(".staging").join(&id));
            // Best-effort: mark mutation_log as failed. Fire-and-forget update
            // (recovery on next boot would catch a missed update anyway).
            let now2 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = log_tx
                .send(crate::persist::writer::ColonyWriteOp::MutationLogUpdate {
                    id: id.clone(),
                    status: "failed".into(),
                    committed_at: now2,
                    failure_reason: Some(format!("{e:?}")),
                    ack: Some(tx),
                })
                .await;
            let _ = rx.await;
            send_eda_reject(
                &id,
                &e,
                reply_to.as_ref(),
                trace_id,
                parent_message_id,
                registry,
                hive_scopes,
                dead_letters,
                log_tx,
                &blob_store,
                blob_inline_max_bytes,
                &payload,
            )
            .await;
            return MutationOutcome::Rejected {
                id: Some(id),
                error_code: e.error_code().into(),
                details: format!("{e:?}"),
            };
        }
    };

    // Apply sequence step 7: optional crash-injection hook.
    crate::mutation::hook::park_after_rename().await;

    // Apply sequence step 8: remove_nodes — Phase-13.5-Lifecycle-3b Task 6
    // (SCOPE 4, spec Z.260): `remove_nodes` = Disconnect, NOT Delete. The node's
    // registry entry STAYS (No-Delete: `cell_id`, FS, `cell.db` untouched). We
    // only COLLECT the resolved paths here; their edges (from + to) are removed
    // in step 10 via the SAME A5 buffer/rollback machinery (the buffer and
    // rollback vectors are declared there), and each path is seeded into the
    // recompute `affected_scope` so the recompute-hook flips `active→false`,
    // stops the running task, and drains the mailbox remainder to the DLQ.
    // Remove-before-add ordering (spec): allows remove+add at the same path.
    let mut removed_node_paths: Vec<Path> = Vec::new();
    if let Some(rems) = diff_subst.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            // validate guaranteed match.name is present and matches ≥1 registry cell.
            let name = r
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .expect("validate guaranteed remove_nodes[].match.name");
            let p = crate::mutation::resolve_scoped_path(&scope, name);
            removed_node_paths.push(p);
        }
    }

    // Apply sequence step 9: cell spawn + DIRECT registry.insert (NO self-send).
    // correction 2: the Mutation arm already holds `&mut registry`. A
    // self-send (`inbox_self_tx.send(ColonyMsg::Register).await`) would deadlock
    // because the select! loop cannot process its own message while the Mutation
    // arm is still executing. Direct in-memory insert + fire-and-forget
    // UpsertRegistry write-op instead.
    let now_spawn = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Paket-3 P3-C1 (P8 fix, reviewer requirement A3): build the POST-STATE
    // edge view this diff will produce, so the eager-spawn gate below can derive
    // a staged cell's activity against the edges as they WILL be after the diff
    // applies. The spawn loop runs BEFORE steps 9b/9c/10 mutate `edges`, so
    // computing activity from the current `edges` would always look inactive for
    // a fresh cell. The view = `edges ∪ add_edges − (remove_edges ∪ remove_nodes
    // edges)`, mirroring exactly what step 10b recomputes against (see
    // `connectivity::post_state_edges`). Scope resolution mirrors the step-10
    // `add_edges`/`remove_edges` blocks verbatim (`resolve_scoped_path`).
    let post_add_edges: Vec<(Path, Path)> = diff_subst
        .get("add_edges")
        .and_then(|v| v.as_array())
        .map(|adds| {
            adds.iter()
                .filter_map(|e| {
                    let f = e.get("from").and_then(|v| v.as_str())?;
                    let t = e.get("to").and_then(|v| v.as_str())?;
                    Some((
                        crate::mutation::resolve_scoped_path(&scope, f),
                        crate::mutation::resolve_scoped_path(&scope, t),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let post_remove_edges: Vec<(Path, Path)> = diff_subst
        .get("remove_edges")
        .and_then(|v| v.as_array())
        .map(|rems| {
            rems.iter()
                .filter_map(|r| {
                    let f = r
                        .get("match")
                        .and_then(|m| m.get("from"))
                        .and_then(|v| v.as_str())?;
                    let t = r
                        .get("match")
                        .and_then(|m| m.get("to"))
                        .and_then(|v| v.as_str())?;
                    Some((
                        crate::mutation::resolve_scoped_path(&scope, f),
                        crate::mutation::resolve_scoped_path(&scope, t),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let post_state_view = crate::connectivity::post_state_edges(
        edges,
        &post_add_edges,
        &post_remove_edges,
        &removed_node_paths,
    );

    for sd in &staged {
        let factory = match factories.get(&sd.template).cloned() {
            Some(f) => f,
            None => {
                // Should be impossible: validate_post_state checked template_missing.
                tracing::error!(
                    template = %sd.template,
                    "factory disappeared between validate and spawn"
                );
                // Befund 8: spurless reject — sweep the renamed-but-unregistered
                // staged dirs + staging before returning.
                sweep_reject_residue(&staged, registry, root, &id);
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = log_tx
                    .send(crate::persist::writer::ColonyWriteOp::MutationLogUpdate {
                        id: id.clone(),
                        status: "failed".into(),
                        committed_at: now_spawn,
                        failure_reason: Some(format!("factory missing for {}", sd.template)),
                        ack: Some(tx),
                    })
                    .await;
                let _ = rx.await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: "spawn".into(),
                    details: format!("factory missing for {}", sd.template),
                };
            }
        };
        // Phase-13.5 Lifecycle-3b Task 7 (A2): mutation-spawn timeout mapping,
        // mirroring `bootstrap_apply.rs` EXACTLY. `cell.timeout == 0` →
        // idle-default (or per-cell `idle_timeout_ms` override); other values
        // (`-1` persistent / runs forever, `> 0` one-shot) get NO idle timer.
        // Inputs come from `StagedDir.cell_timeout` / `StagedDir.idle_timeout_ms`
        // (substituted `config.json`), replacing the former hardcode (13-L-1).
        let mut_cell_timeout: i64 = sd.cell_timeout;
        let mut_idle_timeout = match sd.cell_timeout {
            0 => Some(std::time::Duration::from_millis(
                sd.idle_timeout_ms.unwrap_or(idle_default_ms),
            )),
            _ => None,
        };

        // Paket-3 P3-C1 (P8 fix): activity gate before the EAGER spawn.
        // If this cell would be INACTIVE in the POST-STATE edge view (e.g. its
        // diff `add_edges` wires it under an inactive parent hive) AND it is an
        // EAGER kind (the factory offers a real `build_boot_inactive_respawn` —
        // the SAME discriminator the bootstrap boot-inactive path uses), do NOT
        // eager-spawn it: registering it inactive + `NotYetSpawned` WITHOUT
        // building the task avoids the transient real side effect (mcp
        // subprocess / proxy connection) that step 10b would peace-stop a
        // sub-second later. We mirror `bootstrap_apply::register_inactive_non_spawned`
        // EXACTLY: the real respawn carries `eager_on_reconnect == true`, so a
        // later `add_edges` reconnect's `(entry.respawn)()` (step 10b) spawns
        // the task immediately. A cell that would be ACTIVE (Grace — pure
        // edge-less `add_nodes` under an active/root scope), or a lazy/Dormant
        // kind (`build_boot_inactive_respawn` returns `None`, never eager-spawns
        // anyway), falls through to the unchanged `spawn_cell` path below.
        // CRITICAL — Grace preservation (spec Z.1463ff): the gate fires ONLY for
        // a cell that is CONNECTED in the post-state (a diff edge reaches it) but
        // derived inactive because an ancestor hive is inactive. An edge-LESS
        // fresh cell has no post-state edge → `is_connected == false` →
        // `compute_active == false` too, but that is Grace-active (spawn), NOT a
        // disconnect. Gating on `is_connected && !compute_active` keeps Grace.
        let connected = crate::connectivity::is_connected(&sd.absolute_path, &post_state_view);
        let would_be_inactive = connected
            && !crate::connectivity::compute_active(
                &sd.absolute_path,
                &post_state_view,
                hive_scopes,
            );
        // paket-7 B5 (Auflage A3): resolve the effective emits-validation flag
        // BEFORE either spawn path constructs its RespawnFn / reconnect-hook
        // closure (both `build_boot_inactive_respawn` and `spawn_cell` clone this
        // `ContractView`), so crash-restarted AND reconnect-respawned mutation
        // cells carry the resolved flag.
        let mut contract_view = sd.contract_view.clone();
        contract_view.validate_emits =
            crate::bootstrap_apply::resolve_validate_emits(strict_validation);
        // Hardening Slice 1 (Task 1.4): per-cell contract data for the colony's
        // `node_contracts` map — built BEFORE `contract_view` moves into
        // `spawn_cell`, inserted AFTER the registry insert of each spawn path
        // (the point where the arm knows registration succeeded).
        let node_contract = NodeContract {
            header_view: sd.header_view.clone(),
            emits: contract_view.emits.clone(),
            validate_emits: contract_view.validate_emits,
        };
        if would_be_inactive
            && let Some(real_respawn) = factory.clone().build_boot_inactive_respawn(
                sd.absolute_path.clone(),
                sd.params.clone(),
                outputs_tx.clone(),
                sd.final_path.clone(),
                contract_view.clone(),
                inbox_self_tx.clone(),
                mut_idle_timeout,
                mut_cell_timeout,
                crate::resolve_message_timeout(sd.message_timeout, message_timeout_default_ms),
                blob_store.clone(),
                sd.mailbox_size.unwrap_or(mailbox_default_capacity),
            )
        {
            // Fresh throwaway mailbox pair: parked in `NotYetSpawned`, never used
            // while the cell stays inactive (inactive-routing short-circuits).
            let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
            let handle_actor = ActorHandle::new(sd.absolute_path.clone(), sender);
            // No wake mechanic: an eager cell is re-spawned (not woken). A stray
            // delivery dead-letters loudly (F1-KH2 Schicht 2).
            let wake: Option<crate::WakeFn> = None;
            let cell_id = Uuid::now_v7();
            registry.insert(
                sd.absolute_path.clone(),
                RegistryEntry {
                    handle: handle_actor,
                    respawn: real_respawn,
                    wake,
                    restart_count: 0,
                    restart_limit: DEFAULT_RESTART_LIMIT,
                    cell_id,
                    cell_type: sd.template.clone(),
                    status: CellStatus::NotYetSpawned { receiver },
                    // Eager kind → eager re-spawn on reconnect (step 10b).
                    eager_on_reconnect: true,
                    // POST-STATE derives this cell inactive → register inactive,
                    // NO task. Step 10b confirms (no flip) until a reconnect.
                    active: false,
                    failed: false,
                    stop_tx: None,
                    death_ack_rx: None,
                },
            );
            // Hardening Slice 1 (Task 1.4): registration succeeded → register
            // the per-cell contract data for the 14-B post-state live source.
            node_contracts.insert(sd.absolute_path.clone(), node_contract);
            // Writer-Op (fire-and-forget — durable by FIFO before committed-update).
            let _ = log_tx
                .send(crate::persist::writer::ColonyWriteOp::UpsertRegistry {
                    path: sd.absolute_path.clone(),
                    cell_id: cell_id.to_string(),
                    cell_type: sd.template.clone(),
                    created_at: now_spawn,
                    updated_at: now_spawn,
                })
                .await;
            // GH #62: index the instantiation's provenance (FIFO — the upsert
            // above created the row).
            if let Some(prov) = sd.provenance.clone() {
                let _ = log_tx
                    .send(
                        crate::persist::writer::ColonyWriteOp::SetRegistryProvenance {
                            path: sd.absolute_path.clone(),
                            provenance: prov,
                        },
                    )
                    .await;
            }
            continue;
        }

        let spawned = match factory.spawn_cell(
            sd.absolute_path.clone(),
            sd.params.clone(),
            outputs_tx.clone(),
            sd.final_path.clone(),
            contract_view,
            inbox_self_tx.clone(),
            mut_idle_timeout,
            mut_cell_timeout,
            // P3-B-plumb-2: resolve the active B-backstop from the per-cell
            // `cell.message_timeout` (substituted config.json) against the colony
            // `message_timeout_default_ms`. `>0` → backstop, `0`/`-1` → None.
            crate::resolve_message_timeout(sd.message_timeout, message_timeout_default_ms),
            blob_store.clone(),
            sd.mailbox_size.unwrap_or(mailbox_default_capacity),
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, path = %sd.absolute_path.as_str(), "spawn_cell failed");
                // Befund 8: spurless reject — sweep the renamed-but-unregistered
                // staged dirs + staging so no stranded directory poisons an
                // identical follow-up mutation (commit-as-resume).
                sweep_reject_residue(&staged, registry, root, &id);
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = log_tx
                    .send(crate::persist::writer::ColonyWriteOp::MutationLogUpdate {
                        id: id.clone(),
                        status: "failed".into(),
                        committed_at: now_spawn,
                        failure_reason: Some(format!("spawn: {e}")),
                        ack: Some(tx),
                    })
                    .await;
                let _ = rx.await;
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: "spawn".into(),
                    details: e,
                };
            }
        };

        // Phase-13-K-2: branch per CellKind. Active → DIRECT insert (stateless,
        // status Awake). Dormant → DIRECT insert (stateful, status
        // NotYetSpawned). NO self-send (correction 2) — the mutation arm holds
        // &mut registry, and ColonyMsg::Register/RegisterDormant would block the
        // select! loop.
        let cell_id = Uuid::now_v7();
        match spawned {
            crate::SpawnedCellKind::Active {
                sender,
                join,
                peace_rx,
                // Phase-13.5 Lifecycle-3b Task 4 (F2): stored in the registry so
                // the recompute-hook can fire a peace-stop on disconnect.
                stop_tx,
                death_ack_rx,
                // Paket-3 P3-B-restart: forwarded to the watcher so a backstop
                // death of THIS spawned cell classifies as DeathKind::Backstop.
                backstop_rx,
                respawn,
            } => {
                let handle_actor = ActorHandle::new(sd.absolute_path.clone(), sender);
                // Stateless/long-running: status stays Awake → no wake mechanic
                // (F1-KH2 Schicht 2: a stray parked delivery dead-letters loudly).
                let wake: Option<crate::WakeFn> = None;
                registry.insert(
                    sd.absolute_path.clone(),
                    RegistryEntry {
                        handle: handle_actor,
                        respawn,
                        wake,
                        restart_count: 0,
                        restart_limit: DEFAULT_RESTART_LIMIT,
                        cell_id,
                        cell_type: sd.template.clone(),
                        status: CellStatus::Awake,
                        // Active = eager kind → eager re-spawn on reconnect.
                        eager_on_reconnect: true,
                        // Mutation-spawn = fresh spawn → active (spawn = active).
                        active: true,
                        failed: false,
                        stop_tx: Some(stop_tx),
                        death_ack_rx: Some(death_ack_rx),
                    },
                );
                // Watcher for CellDied events — same wiring as handle_register.
                spawn_watcher(
                    inbox_self_tx,
                    sd.absolute_path.clone(),
                    join,
                    peace_rx,
                    backstop_rx,
                );
            }
            crate::SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                // Phase-13.5 Lifecycle-3b Task 4: dropped — a dormant cell has no
                // running task. A wake spawns a fresh task with its own stop pair;
                // before wake a disconnect just flips `active=false` (Task-4.2).
                stop_tx: _,
                death_ack_rx: _,
                respawn,
            } => {
                let handle_actor = ActorHandle::new(sd.absolute_path.clone(), sender);
                registry.insert(
                    sd.absolute_path.clone(),
                    RegistryEntry {
                        handle: handle_actor,
                        respawn,
                        // Lazy kind: the factory's REAL wake (wake-on-message).
                        wake: Some(wake),
                        restart_count: 0,
                        restart_limit: DEFAULT_RESTART_LIMIT,
                        cell_id,
                        cell_type: sd.template.clone(),
                        status: CellStatus::NotYetSpawned { receiver },
                        // Dormant = lazy kind → reconnect flips active only.
                        eager_on_reconnect: false,
                        // Mutation-spawn = fresh spawn → active (spawn = active).
                        active: true,
                        failed: false,
                        stop_tx: None,
                        death_ack_rx: None,
                    },
                );
                // NO spawn_watcher — cell-task is parked, no join handle yet.
                // First wake-pre-send (route_with_log, 13-I-1) starts the task
                // + watcher analogously to the bootstrap path.
            }
        }

        // Hardening Slice 1 (Task 1.4): registration succeeded (both Active and
        // Dormant arms inserted above) → register the per-cell contract data
        // for the 14-B post-state live source.
        node_contracts.insert(sd.absolute_path.clone(), node_contract);

        // Writer-Op (fire-and-forget — durable by FIFO before committed-update).
        let _ = log_tx
            .send(crate::persist::writer::ColonyWriteOp::UpsertRegistry {
                path: sd.absolute_path.clone(),
                cell_id: cell_id.to_string(),
                cell_type: sd.template.clone(),
                created_at: now_spawn,
                updated_at: now_spawn,
            })
            .await;
        // GH #62: index the instantiation's provenance (FIFO — the upsert above
        // created the row).
        if let Some(prov) = sd.provenance.clone() {
            let _ = log_tx
                .send(
                    crate::persist::writer::ColonyWriteOp::SetRegistryProvenance {
                        path: sd.absolute_path.clone(),
                        provenance: prov,
                    },
                )
                .await;
        }
    }

    // Apply sequence step 10 (A5 atomicity restructure): edge ops are NO LONGER
    // fire-and-forget. Instead we (1) apply edge changes IN-RAM (needed for the
    // recompute below) while tracking rollback info, and (2) collect the matching
    // `InsertEdge`/`RemoveEdge` WriteOps into a LOCAL buffer. The recompute-hook
    // (step 10b) appends `SetRegistryStatus` ops to the same buffer and may
    // trigger a death-ack-wait (F5-Variante-A). Only on SUCCESS is the buffer
    // enqueued in order — BEFORE the durable committed-update (FIFO durability,
    // decision 8a). On term-timeout the in-RAM changes are rolled back and the
    // buffer is discarded: `colony.db` stays untouched (no half-disconnect).
    let now_edges = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Local WriteOp buffer — flushed on success, discarded on timeout.
    let mut write_buffer: Vec<crate::persist::writer::ColonyWriteOp> = Vec::new();
    // Rollback tracking for the in-RAM edge changes.
    let mut inserted_edge_ids: Vec<Uuid> = Vec::new();
    let mut removed_edges_saved: Vec<crate::edge_table::Edge> = Vec::new();
    // Paths directly involved in this mutation's edge ops (recompute seeds).
    let mut involved: Vec<Path> = Vec::new();
    // Paket 6 Block D: single-cell `add_nodes`-Resume targets are direct
    // diff-addresses (overview Z.170-180) but produce no `StagedDir`; seed them
    // so the recompute reaches them and the Sticky-Diskriminator reactivates a
    // resumed `failed`/inactive cell on its own path.
    involved.extend(resume_targets.iter().cloned());

    // Apply sequence step 9b (Paket-2 T4): swap_nodes graph-swap lowering.
    //
    // Runs AFTER Step 9's registry.insert loop (t3 already staged + spawned +
    // registered via the SAME machinery as add_nodes) and feeds the EXISTING
    // Step-10 edge buffers (`edges` / `write_buffer` / `inserted_edge_ids` /
    // `removed_edges_saved` / `involved`). For each swap entry it resolves
    // t2 = scope+match.name and t3 = scope+with.name, then swings every external
    // edge of t2 onto t3 via the pure T3 helper `plan_edge_swing`:
    //   - condition/modifier are carried verbatim (cloned compiled + source);
    //   - INSERT-BEFORE-REMOVE (set-vor-delete invariant): new edges are
    //     inserted first, then the old edges removed;
    //   - both t2 and t3 are seeded into `involved` so the Step-10b recompute
    //     reaches both. t2 becomes edge-less → recompute derives it inactive →
    //     the 3b stop-mechanic stops it + drains its remainder to DLQ
    //     `cell_inactive`. t2 stays on disk (No-Delete); its `cell_id`/`cell.db`
    //     are untouched. t3 is fresh (template form) or an existing referenced
    //     cell (`{name}` form — no staging, just the edge swing).
    if let Some(swaps) = diff_subst.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let t2_name = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .expect("validate guaranteed swap_nodes[].match.name");
            let t3_name = s
                .get("with")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .expect("validate guaranteed swap_nodes[].with.name");
            let t2 = crate::mutation::resolve_scoped_path(&scope, t2_name);
            let t3 = crate::mutation::resolve_scoped_path(&scope, t3_name);
            let plan = crate::mutation::swap::plan_edge_swing(&t2, &t3, edges);
            // INSERT-BEFORE-REMOVE: add the swung edges first.
            for sw in plan.inserts {
                let edge_id = Uuid::now_v7();
                edges.insert(crate::edge_table::Edge {
                    id: edge_id,
                    from: sw.from.clone(),
                    to: sw.to.clone(),
                    condition: sw.condition,
                    modifier: sw.modifier,
                });
                inserted_edge_ids.push(edge_id);
                write_buffer.push(crate::persist::writer::ColonyWriteOp::InsertEdge {
                    id: edge_id.to_string(),
                    from: sw.from.as_str().into(),
                    to: sw.to.as_str().into(),
                    created_at: now_edges,
                    condition: sw.cond_src,
                    modifier: sw.mod_src,
                });
            }
            // THEN remove the old edges (fetch each removed Edge for rollback,
            // mirroring the remove_edges block's matched-fetch-then-remove).
            for old_id in plan.remove_ids {
                let removed = edges.iter().find(|e| e.id == old_id).cloned();
                if let Some(edge) = removed {
                    edges.remove(&old_id);
                    write_buffer.push(crate::persist::writer::ColonyWriteOp::RemoveEdge {
                        id: old_id.to_string(),
                    });
                    removed_edges_saved.push(edge);
                }
            }
            // Seed BOTH endpoints so the recompute reaches t2 (now edge-less →
            // inactive + stop) AND t3 (now wired).
            involved.push(t2);
            involved.push(t3);
        }
    }

    // Apply sequence step 9c (Paket-5 T12, P9 per-node subtree merge resume):
    // instantiate each merge-staged SUBTREE. ONLY the missing rename-roots were
    // staged + renamed into place by `apply_mutation`; existing nodes were never
    // FS-touched (F1). Here we wire it into the live substrate so the EXISTING
    // step-10b recompute sees it:
    //   - every MISSING (rename-root) NON-hive cell is registered INACTIVE-non-
    //     spawned (mirroring `bootstrap_apply::register_inactive_non_spawned`: a
    //     real respawn from `build_boot_inactive_respawn` for eager kinds, else an
    //     inert fallback; `active=false`, `NotYetSpawned`, NO `spawn_watcher`),
    //     plus a fire-and-forget `UpsertRegistry` write-op like the single-cell
    //     spawn loop;
    //   - every MISSING hive marker is registered in `hive_scopes` (in-memory) +
    //     an `InsertHiveScope` write-op into the SAME A5 `write_buffer`;
    //   - every EXISTING node + EXISTING hive is left untouched (cell_id /
    //     config.json / cell.db preserved) — only seeded into `involved`. Existing
    //     hive scopes re-emit an idempotent `InsertHiveScope` (INSERT OR IGNORE);
    //   - every internal edge is inserted into the `EdgeTable` with T8 dedup +
    //     tracked in `inserted_edge_ids` (rollback) + an `InsertEdge` write-op.
    // Every node path (missing AND existing), hive path and edge endpoint is seeded
    // into `involved` so the recompute reaches BOTH the freshly-instantiated and
    // the resumed (existing inactive) nodes, reactivating those now connected by
    // internal edges (F4 resume meaning). Without a connecting incoming edge the
    // subtree root stays inactive (no eager spawn) until a later `add_edges`.
    for subtree in &staged_subtrees {
        // (1) Inactive-non-spawned registration per MISSING spawnable (non-hive)
        // cell — flattened across the rename-roots' staged sub-trees.
        for cell in subtree.rename_roots.iter().flat_map(|r| r.cells.iter()) {
            let factory = factories.get(&cell.cell_type).cloned();
            // Idle-timeout mapping mirrors `register_inactive_non_spawned` and the
            // single-cell spawn loop: `cell.timeout == 0` → idle-default (or the
            // per-cell `idle_timeout_ms` override); other values get NO idle timer.
            let idle_timeout = match cell.cell_timeout {
                0 => Some(std::time::Duration::from_millis(
                    cell.idle_timeout_ms.unwrap_or(idle_default_ms),
                )),
                _ => None,
            };
            // paket-7 B5 (Auflage A3): resolve the effective emits-validation flag
            // BEFORE the subtree reconnect-hook captures this `ContractView`, so a
            // reconnect-respawned subtree cell carries the resolved flag.
            let mut contract_view = cell.contract_view.clone();
            contract_view.validate_emits =
                crate::bootstrap_apply::resolve_validate_emits(strict_validation);
            // F1-KH2 kind discriminator: declared on the trait, so the kind is
            // known WITHOUT building a task (an eager `spawn_cell` would be a
            // real transient side effect — the same hazard the P3-C1 activity
            // gate avoids on the single-cell path).
            let is_lazy = factory.as_ref().map(|f| f.is_lazy()).unwrap_or(false);
            let real_respawn = factory.as_ref().filter(|_| !is_lazy).and_then(|f| {
                f.clone().build_boot_inactive_respawn(
                    cell.absolute_path.clone(),
                    cell.params.clone(),
                    outputs_tx.clone(),
                    cell.final_path.clone(),
                    contract_view.clone(),
                    inbox_self_tx.clone(),
                    idle_timeout,
                    cell.cell_timeout,
                    // P3-B-plumb-1: behavior-neutral — message_timeout resolved later.
                    None,
                    blob_store.clone(),
                    cell.mailbox_size.unwrap_or(mailbox_default_capacity),
                )
            });
            // F1-KH2 kind split (pre-R12 fix): the old path installed an INERT
            // WakeFn for EVERY subtree cell — first delivery to a lazy (Dormant)
            // cell after the R12 same-mutation activation dropped the parked
            // receiver (silent loss + false `Awake`). Eager kinds keep the
            // parked-throwaway shape (reconnect re-spawns via `respawn`); lazy
            // kinds now get the SAME real Hot/Cold wiring as the single-cell
            // `spawn_cell` path: real mailbox pair + the factory's WakeFn
            // (wake-on-message after the recompute reconnects them).
            let (handle_actor, parked_receiver, wake, respawn, eager_on_reconnect): (
                ActorHandle,
                tokio::sync::mpsc::Receiver<meclaw_core::Message>,
                Option<crate::WakeFn>,
                crate::RespawnFn,
                bool,
            ) = if let Some(real_respawn) = real_respawn {
                // EAGER kind: parked throwaway pair, never used while inactive
                // (inactive-routing short-circuits); reconnect calls `respawn`.
                // No wake mechanic — a stray delivery dead-letters loudly.
                let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                (
                    ActorHandle::new(cell.absolute_path.clone(), sender),
                    receiver,
                    None,
                    real_respawn,
                    true,
                )
            } else if is_lazy {
                let spawned = factory.map(|f| {
                    f.spawn_cell(
                        cell.absolute_path.clone(),
                        cell.params.clone(),
                        outputs_tx.clone(),
                        cell.final_path.clone(),
                        contract_view.clone(),
                        inbox_self_tx.clone(),
                        idle_timeout,
                        cell.cell_timeout,
                        crate::resolve_message_timeout(
                            cell.message_timeout,
                            message_timeout_default_ms,
                        ),
                        blob_store.clone(),
                        cell.mailbox_size.unwrap_or(mailbox_default_capacity),
                    )
                });
                match spawned {
                    Some(Ok(crate::SpawnedCellKind::Dormant {
                        sender,
                        receiver,
                        wake,
                        // Dormant placeholder stop wiring belongs to the PRE-wake
                        // state — dropped exactly like the single-cell Dormant arm.
                        stop_tx: _,
                        death_ack_rx: _,
                        respawn,
                    })) => (
                        ActorHandle::new(cell.absolute_path.clone(), sender),
                        receiver,
                        // Lazy kind: the factory's REAL wake (wake-on-message).
                        Some(wake),
                        respawn,
                        false,
                    ),
                    Some(Ok(crate::SpawnedCellKind::Active {
                        sender: _,
                        join: _,
                        peace_rx: _,
                        stop_tx,
                        death_ack_rx: _,
                        backstop_rx: _,
                        respawn,
                    })) => {
                        // Unreachable in practice: every eager built-in implements
                        // `build_boot_inactive_respawn`. Best-effort: peace-stop the
                        // transient task and register the eager parked shape with
                        // the REAL respawn so a reconnect still works.
                        tracing::error!(
                            path = %cell.absolute_path.as_str(),
                            "subtree spawn: factory returned Active without a \
                             boot-inactive hook — stopping the transient task"
                        );
                        let _ = stop_tx.send(());
                        let (sender, receiver) =
                            tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                        (
                            ActorHandle::new(cell.absolute_path.clone(), sender),
                            receiver,
                            None,
                            respawn,
                            true,
                        )
                    }
                    Some(Err(e)) => {
                        // Params were validated at staging (parser invariant) — a
                        // spawn error here is exceptional. Register the inert
                        // fallback; deliveries fail LOUDLY (defense layer).
                        tracing::error!(
                            error = %e,
                            path = %cell.absolute_path.as_str(),
                            "subtree spawn_cell failed — registering inert fallback"
                        );
                        let (sender, receiver) =
                            tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                        (
                            ActorHandle::new(cell.absolute_path.clone(), sender),
                            receiver,
                            None,
                            subtree_inert_respawn(),
                            false,
                        )
                    }
                    None => {
                        // `is_lazy == true` implies the factory exists — defensive only.
                        tracing::error!(
                            path = %cell.absolute_path.as_str(),
                            cell_type = %cell.cell_type,
                            "subtree spawn: no factory — registering inert fallback"
                        );
                        let (sender, receiver) =
                            tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                        (
                            ActorHandle::new(cell.absolute_path.clone(), sender),
                            receiver,
                            None,
                            subtree_inert_respawn(),
                            false,
                        )
                    }
                }
            } else {
                // Eager kind WITHOUT a boot-inactive hook (or factory missing):
                // no task is built; after a reconnect such a cell stays parked
                // and deliveries dead-letter loudly (`cell_inactive`, defense
                // layer) — never silent loss.
                let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                (
                    ActorHandle::new(cell.absolute_path.clone(), sender),
                    receiver,
                    None,
                    subtree_inert_respawn(),
                    false,
                )
            };
            let cell_id = Uuid::now_v7();
            registry.insert(
                cell.absolute_path.clone(),
                RegistryEntry {
                    handle: handle_actor,
                    respawn,
                    wake,
                    restart_count: 0,
                    restart_limit: DEFAULT_RESTART_LIMIT,
                    cell_id,
                    cell_type: cell.cell_type.clone(),
                    status: CellStatus::NotYetSpawned {
                        receiver: parked_receiver,
                    },
                    eager_on_reconnect,
                    // Parentless subtree → inactive until a later add_edges connects it.
                    active: false,
                    failed: false,
                    stop_tx: None,
                    death_ack_rx: None,
                },
            );
            // Hardening Slice 1 (Task 1.4): registration succeeded → register
            // the per-cell contract data for the 14-B post-state live source.
            node_contracts.insert(
                cell.absolute_path.clone(),
                NodeContract {
                    header_view: cell.header_view.clone(),
                    emits: contract_view.emits.clone(),
                    validate_emits: contract_view.validate_emits,
                },
            );
            // Writer-Op (fire-and-forget — durable by FIFO before committed-update).
            let _ = log_tx
                .send(crate::persist::writer::ColonyWriteOp::UpsertRegistry {
                    path: cell.absolute_path.clone(),
                    cell_id: cell_id.to_string(),
                    cell_type: cell.cell_type.clone(),
                    created_at: now_spawn,
                    updated_at: now_spawn,
                })
                .await;
            // GH #62: every nested subtree cell indexes the subtree template it
            // came from (FIFO — the upsert above created the row).
            if let Some(prov) = cell.provenance.clone() {
                let _ = log_tx
                    .send(
                        crate::persist::writer::ColonyWriteOp::SetRegistryProvenance {
                            path: cell.absolute_path.clone(),
                            provenance: prov,
                        },
                    )
                    .await;
            }
            involved.push(cell.absolute_path.clone());
        }
        // (2) MISSING hive-scope markers: in-memory + InsertHiveScope into the A5
        // buffer — flattened across the rename-roots' staged sub-trees.
        for hive_path in subtree
            .rename_roots
            .iter()
            .flat_map(|r| r.hive_scopes.iter())
        {
            hive_scopes.register(crate::hive_scope::HiveScope {
                path: hive_path.clone(),
            });
            write_buffer.push(crate::persist::writer::ColonyWriteOp::InsertHiveScope {
                path: hive_path.clone(),
                created_at: now_spawn,
            });
            involved.push(hive_path.clone());
        }
        // (2b) EXISTING nodes + EXISTING hives (resume — NO FS, NO re-register):
        // seed into `involved` so the recompute reaches them. Existing hive scopes
        // re-emit an idempotent `InsertHiveScope` (INSERT OR IGNORE) — harmless if
        // already persisted, and ensures the in-mem `hive_scopes` carries it after
        // a fresh-boot resume race. Existing cells keep their registry entry +
        // cell_id untouched (F1).
        for ex in &subtree.existing {
            involved.push(ex.absolute_path.clone());
        }
        for ex_hive in &subtree.existing_hives {
            hive_scopes.register(crate::hive_scope::HiveScope {
                path: ex_hive.absolute_path.clone(),
            });
            write_buffer.push(crate::persist::writer::ColonyWriteOp::InsertHiveScope {
                path: ex_hive.absolute_path.clone(),
                created_at: now_spawn,
            });
            involved.push(ex_hive.absolute_path.clone());
        }
        // (3) Internal edges: EdgeTable insert + rollback-track + InsertEdge op.
        // condition/modifier arrive resolved (`Option<String>`/`Option<Value>`)
        // and are compiled the same way step 10 compiles `add_edges`.
        for edge in &subtree.internal_edges {
            let cel_condition = edge.condition.as_ref().map(|s| {
                crate::cel_eval::parse_condition(s)
                    .expect("subtree internal edge condition validated by stage_subtree")
            });
            let cel_modifier = edge.modifier.as_ref().map(|m| {
                let spec: crate::config::ModifierSpec =
                    meclaw_core::serde_json::from_value(m.clone())
                        .expect("subtree internal edge modifier is a serialized ModifierSpec");
                crate::cel_eval::parse_modifier(&spec)
                    .expect("subtree internal edge modifier validated by stage_subtree")
            });
            // Paket-5 T8 (A1): global edge-dedup, identical discipline to the
            // `add_edges` block below — skip insert + writer-op + fresh UUID for
            // a content-equal edge (identity = from+to+condition+modifier, spec
            // Z.265), but still seed `involved` so the recompute reaches the
            // endpoints.
            let candidate = crate::edge_table::Edge {
                id: Uuid::now_v7(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                condition: cel_condition,
                modifier: cel_modifier,
            };
            involved.push(edge.from.clone());
            involved.push(edge.to.clone());
            if edges.contains_equal(&candidate) {
                continue;
            }
            let edge_id = candidate.id;
            let cond_src = candidate.condition.as_ref().map(|c| c.source.clone());
            let mod_src = candidate
                .modifier
                .as_ref()
                .and_then(|m| meclaw_core::serde_json::to_string(&m.source).ok());
            edges.insert(candidate);
            inserted_edge_ids.push(edge_id);
            write_buffer.push(crate::persist::writer::ColonyWriteOp::InsertEdge {
                id: edge_id.to_string(),
                from: edge.from.as_str().into(),
                to: edge.to.as_str().into(),
                created_at: now_spawn,
                condition: cond_src,
                modifier: mod_src,
            });
        }
    }

    // Step 10 (task 6, SCOPE 4): `remove_nodes` = disconnect. For each removed
    // node, remove ALL of its edges (`from == p` OR `to == p`) in-RAM through the
    // SAME A5 machinery as `remove_edges`: clone the matched edges into
    // `removed_edges_saved` (rollback), push a `RemoveEdge` WriteOp to
    // `write_buffer`, and seed `p` into `involved` so `affected_scope` pulls in the
    // node (and, for a hive, its whole subtree). The registry entry is NOT removed
    // — the recompute-hook below flips it `active→false` and stops its task.
    for p in &removed_node_paths {
        let matched: Vec<crate::edge_table::Edge> = edges
            .iter()
            .filter(|e| e.from == *p || e.to == *p)
            .cloned()
            .collect();
        involved.push(p.clone());
        for edge in matched {
            // Seed BOTH endpoints into `involved` (like `remove_edges`) so the
            // recompute reaches the OTHER side too: removing the last gating edge
            // of a hive (e.g. `/x -> /h`) must recompute `/h` and — via
            // `affected_scope`'s subtree expansion — its whole subtree, not just
            // the removed node `p`.
            involved.push(edge.from.clone());
            involved.push(edge.to.clone());
            edges.remove(&edge.id);
            write_buffer.push(crate::persist::writer::ColonyWriteOp::RemoveEdge {
                id: edge.id.to_string(),
            });
            removed_edges_saved.push(edge);
        }
    }

    if let Some(adds) = diff_subst.get("add_edges").and_then(|v| v.as_array()) {
        for e in adds {
            let from_name = e
                .get("from")
                .and_then(|v| v.as_str())
                .expect("validate guaranteed add_edges[].from");
            let to_name = e
                .get("to")
                .and_then(|v| v.as_str())
                .expect("validate guaranteed add_edges[].to");
            let from_path = crate::mutation::resolve_scoped_path(&scope, from_name);
            let to_path = crate::mutation::resolve_scoped_path(&scope, to_name);
            // Phase 13.5-A1 T5: parse condition + modifier (validate already
            // CEL-parsed them → `expect` is safe). Persisting CEL state is not
            // part of A1; a reboot rehydrates without CEL.
            let cel_condition = e.get("condition").and_then(|v| v.as_str()).map(|s| {
                crate::cel_eval::parse_condition(s)
                    .expect("validate guaranteed add_edges[].condition is valid CEL")
            });
            let cel_modifier = e.get("modifier").and_then(|v| v.as_object()).map(|obj| {
                let mut spec = crate::config::ModifierSpec::default();
                let collect_set =
                    |obj: &meclaw_core::serde_json::Map<_, _>,
                     key: &str,
                     dst: &mut std::collections::BTreeMap<String, String>| {
                        if let Some(set_obj) = obj.get(key).and_then(|v| v.as_object()) {
                            for (k, v) in set_obj {
                                if let Some(expr) = v.as_str() {
                                    dst.insert(k.clone(), expr.to_string());
                                }
                            }
                        }
                    };
                let collect_delete =
                    |obj: &meclaw_core::serde_json::Map<_, _>, key: &str, dst: &mut Vec<String>| {
                        if let Some(del_arr) = obj.get(key).and_then(|v| v.as_array()) {
                            for d in del_arr {
                                if let Some(s) = d.as_str() {
                                    dst.push(s.to_string());
                                }
                            }
                        }
                    };
                collect_set(obj, "set_context", &mut spec.set_context);
                collect_set(obj, "set_hop", &mut spec.set_hop);
                collect_delete(obj, "delete_context", &mut spec.delete_context);
                collect_delete(obj, "delete_hop", &mut spec.delete_hop);
                // GH #82: the fifth field. This spec is rebuilt field by field
                // rather than deserialised, so a new field that is not picked up
                // HERE would validate and then silently do nothing -- the exact
                // foot-gun the modifier key allow-list exists to prevent.
                spec.restore_ttl = obj
                    .get("restore_ttl")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // GH #82: the fifth field. This spec is rebuilt field by field
                // rather than deserialised, so a new field that is not picked up
                // HERE would validate and then silently do nothing — the exact
                // foot-gun the modifier key allow-list exists to prevent.
                crate::cel_eval::parse_modifier(&spec)
                    .expect("validate guaranteed add_edges[].modifier.set_* is valid CEL")
            });
            // Paket-5 T8 (A1): global edge-dedup. Edge identity = from + to +
            // condition + modifier (spec Z.265, NOT the UUID). A re-applied
            // COMPLETE diff (Phase-15 builder) re-sends the same add_edges; a
            // duplicate insert would mean DOUBLE delivery on the routing
            // cascade. Skip the insert + the matching `InsertEdge` writer-op +
            // the fresh UUID — the existing equal edge keeps its old id
            // (identity-stable). `involved` IS still seeded below so the
            // recompute reaches both endpoints (the route still exists).
            let candidate = crate::edge_table::Edge {
                id: Uuid::now_v7(),
                from: from_path.clone(),
                to: to_path.clone(),
                condition: cel_condition,
                modifier: cel_modifier,
            };
            involved.push(from_path.clone());
            involved.push(to_path.clone());
            if edges.contains_equal(&candidate) {
                continue;
            }
            let edge_id = candidate.id;
            // Save the source strings BEFORE the move into edges.insert (durable edges).
            let cond_src = candidate.condition.as_ref().map(|c| c.source.clone());
            let mod_src = candidate
                .modifier
                .as_ref()
                .and_then(|m| meclaw_core::serde_json::to_string(&m.source).ok());
            edges.insert(candidate);
            inserted_edge_ids.push(edge_id);
            write_buffer.push(crate::persist::writer::ColonyWriteOp::InsertEdge {
                id: edge_id.to_string(),
                from: from_path.as_str().into(),
                to: to_path.as_str().into(),
                created_at: now_edges,
                condition: cond_src,
                modifier: mod_src,
            });
        }
    }
    if let Some(rems) = diff_subst.get("remove_edges").and_then(|v| v.as_array()) {
        for r in rems {
            // Phase-6-MVP match pattern: {from, to}. Both required. validate
            // doesn't reject unknown remove_edges (silent no-op), accepted.
            //
            // Phase 13.5-A1 F6: optional `condition` (string-equality on
            // `edge.condition.source`) and optional `modifier` (serde-JSON
            // equality on `edge.modifier.source`). If a key is present in the
            // match-pattern, it MUST equal the edge's stored source verbatim;
            // if it is absent, the edge's value (Some/None) is not constrained
            // — i.e. omitting `condition`/`modifier` falls back to the
            // pre-F6 behavior (match by from/to only).
            // Paket-5 T1/T2: a missing match.from/to is already rejected
            // PRE-destructively by `validate_remove_edges` above, so here it is
            // a defensive skip (unreachable on a validated diff).
            let Some(from_name) = r
                .get("match")
                .and_then(|v| v.get("from"))
                .and_then(|v| v.as_str())
            else {
                tracing::warn!("remove_edges entry missing match.from; skipping");
                continue;
            };
            let Some(to_name) = r
                .get("match")
                .and_then(|v| v.get("to"))
                .and_then(|v| v.as_str())
            else {
                tracing::warn!("remove_edges entry missing match.to; skipping");
                continue;
            };
            let pat_condition = r
                .get("match")
                .and_then(|v| v.get("condition"))
                .and_then(|v| v.as_str());
            let pat_modifier = r.get("match").and_then(|v| v.get("modifier"));
            let from_path = crate::mutation::resolve_scoped_path(&scope, from_name);
            let to_path = crate::mutation::resolve_scoped_path(&scope, to_name);
            // A5: clone the matched edges (not just ids) so a timeout can
            // re-insert exact copies on rollback. F6 match equality is the SAME
            // predicate validate uses (`remove_edges_pattern_hits`), so validate
            // and apply agree by construction.
            let matched: Vec<crate::edge_table::Edge> = edges
                .iter()
                .filter(|e| {
                    let view = crate::mutation::validate::EdgeMatchView::from(*e);
                    crate::mutation::validate::remove_edges_pattern_hits(
                        &view,
                        from_path.as_str(),
                        to_path.as_str(),
                        pat_condition,
                        pat_modifier,
                    )
                })
                .cloned()
                .collect();
            involved.push(from_path.clone());
            involved.push(to_path.clone());
            for edge in matched {
                edges.remove(&edge.id);
                write_buffer.push(crate::persist::writer::ColonyWriteOp::RemoveEdge {
                    id: edge.id.to_string(),
                });
                removed_edges_saved.push(edge);
            }
        }
    }

    // Apply sequence step 10b (F1+F2, A3): connectivity recompute + disconnect.
    // Build the affected scope over the involved endpoints + registry keys, then
    // per node compute the edge-derived activity. On a `true→false` transition
    // deactivate the cell: flip `entry.active` (rollback-tracked), buffer a
    // `SetRegistryStatus('inactive')`, and trigger a peace-stop for an
    // Awake/Active cell with a running task (move its `death_ack_rx` into the
    // side-map for the inline wait). NotYetSpawned/Asleep cells have no running
    // task → just flip + buffer, no death-ack-wait.
    let now_status = now_edges;
    let known_paths: Vec<Path> = registry.keys().cloned().collect();
    // R12: hive paths ride along so a depth-port endpoint pulls the subtree of
    // its hive ancestors into the recompute (crossing edges flip hive activity).
    let hive_paths: Vec<Path> = hive_scopes.paths().cloned().collect();
    let scope_set = crate::connectivity::affected_scope(&involved, &known_paths, &hive_paths);

    // Step-10 STOP-WIRING GUARD (pre-pass). MUST run BEFORE the flip/stop loop
    // below: that loop fires peace-stops as it iterates, and a stop fired for
    // an earlier node cannot be un-fired — so atomicity demands the whole
    // disconnect be vetted up front. For every node that would transition
    // `true→false` AND is `Awake` AND has NO live `stop_tx`, the disconnect
    // cannot be peace-stopped → honoring it would leave the task running while
    // the cell is marked inactive (a silent "task ⇔ active" zombie). Reject the
    // whole mutation atomically instead.
    //
    // `compute_active` is re-run here (pure, cheap) — duplicating it vs. the
    // main loop is intentional: merging would break atomicity (the main loop
    // fires stops mid-iteration). At this point NO `active` flip has happened
    // yet, so the reject only needs to roll back the in-RAM edge ops.
    //
    // F5 CONTRACT — guard is the PERMANENT backstop (guard-stays, the spec owner
    // 2026-06-07, P4-B closure; NOT interim, NOT unreachable):
    //   • Slice-4 T6 restored stop-wiring on the reconnect-eager path and the
    //     crash/backstop-restart path: both now call `renotify_stop_wiring`, so a
    //     subsequent disconnect finds a live `stop_tx` and the guard is NOT hit.
    //     Proved by `second_disconnect_after_renotify_commits_and_peace_stops`
    //     and `crash_restart_renotifies_stop_pair_then_disconnect_commits`.
    //   • The `term_timeout`-survivor path is now CLOSED (P4): a STATEFUL
    //     survivor with a finite `cell.message_timeout` SELF-HEALS — the backstop
    //     fires on the still-wedged `handle()` → `CellDied{Backstop}` → restart →
    //     `renotify_stop_wiring` restores a fresh `stop_tx` → a retry disconnect
    //     COMMITS, never reaching this guard. Proved by P4-B1
    //     `stateful_survivor_heals_via_backstop_then_retry_disconnect_commits`.
    //   • Three classes of survivor remain genuinely-unwired and ARE this guard's
    //     standing duty (it is their permanent backstop):
    //       (a) LONG-RUNNING survivors — no backstop per spec, so no self-heal.
    //           Proved by `second_disconnect_without_stop_wiring_is_rejected_not_zombie`.
    //       (b) `message_timeout = 0/-1` STATEFUL survivors — backstop disabled.
    //       (c) the tiny race window between an eager respawn and the
    //           `StopWiringRestored` message landing, and any future spawn path
    //           that forgets to call `renotify_stop_wiring`.
    //
    // A `NotYetSpawned`/`Asleep` cell with `stop_tx == None` is NOT guarded
    // (no running task → flipping inactive is correct) — the condition is
    // specifically `Awake && stop_tx.is_none()`.
    for node in &scope_set {
        let Some(entry) = registry.get(node) else {
            continue;
        };
        let would_deactivate =
            entry.active && !crate::connectivity::compute_active(node, edges, hive_scopes);
        if would_deactivate && matches!(entry.status, CellStatus::Awake) && entry.stop_tx.is_none()
        {
            tracing::warn!(
                path = %node.as_str(),
                "disconnect of an Awake cell without live stop-wiring — rejecting mutation \
                 (interim guard; Slice-4 restores stop-wiring)"
            );
            // In-RAM rollback: undo the edge ops only (no `active` flip has
            // happened yet at this pre-pass point). Discard the write_buffer by
            // returning before the flush → no durable committed update.
            for eid in &inserted_edge_ids {
                edges.remove(eid);
            }
            // `removed_edges_saved` is moved by the later term_timeout rollback,
            // but this branch RETURNS, so a conditional move is allowed by the
            // borrow checker and the fall-through path keeps it intact.
            for edge in std::mem::take(&mut removed_edges_saved) {
                edges.insert(edge);
            }
            return MutationOutcome::Rejected {
                id: Some(id),
                error_code: STOP_WIRING_UNAVAILABLE_ERROR_CODE.into(),
                details: format!(
                    "disconnect of Awake cell {} without live stop-wiring (interim guard)",
                    node.as_str()
                ),
            };
        }
    }

    // Rollback tracking for `entry.active` flips done in this hook. Each entry
    // is `(path, prior_active)` — the value to restore on a term-timeout
    // rollback (a true→false disconnect restores `true`; a false→true reconnect
    // restores `false`). Hardcoding `true` here would be wrong for a mixed
    // mutation that both reconnects and disconnects.
    let mut flipped_active: Vec<(Path, bool)> = Vec::new();
    // Side-map of death-ack receivers for deactivated Awake cells (F5-Var-A).
    let mut death_acks: HashMap<Path, oneshot::Receiver<()>> = HashMap::new();
    for node in &scope_set {
        let Some(entry) = registry.get_mut(node) else {
            continue; // hive paths + unregistered nodes carry no registry entry
        };
        let now_active = crate::connectivity::compute_active(node, edges, hive_scopes);
        if entry.active && !now_active {
            // true→false: Disconnect.
            entry.active = false;
            flipped_active.push((node.clone(), true));
            write_buffer.push(crate::persist::writer::ColonyWriteOp::SetRegistryStatus {
                path: node.clone(),
                status: "inactive".into(),
                updated_at: now_status,
            });
            // Trigger a peace-stop for a running (Awake) task. If there is a live
            // stop_tx, also move the death_ack_rx into the side-map for the wait.
            if matches!(entry.status, CellStatus::Awake)
                && let Some(stop) = entry.stop_tx.take()
            {
                let _ = stop.send(());
                if let Some(rx) = entry.death_ack_rx.take() {
                    death_acks.insert(node.clone(), rx);
                }
            }
            // false→false and NotYetSpawned/Asleep: no-op beyond the flip+buffer.
        } else if !entry.active && now_active {
            // Paket 6 Sticky-Diskriminator: a `failed` cell is reactivated ONLY when
            // the mutation directly addresses it (∈ `involved`). An incidental
            // recompute (neighbour mutation, hive cascade) that finds it now-connected
            // MUST leave it failed — it keeps its edges, so `now_active` would
            // otherwise wrongly revive it. The loop body ends right after this
            // if/else, so `continue` skips no required logic for this node.
            if entry.failed && !involved.contains(node) {
                continue;
            }
            // Phase-13.5 Lifecycle-3b Task 7 (F6): false→true Reconnect.
            // `add_edges`/`add_nodes` reconnected a previously-disconnected
            // subtree. Flip `active=true` + persist `SetRegistryStatus('active')`
            // for every reactivated node; the eager-vs-lazy spawn discriminator
            // decides whether the task starts NOW or waits for the first message.
            //
            // Deliberate reconnect (false→true flip): clear `failed` + reset the
            // restart budget (any genuine reconnect flip → restart_count = 0,
            // including a merely-inactive cell — broad reset).
            entry.active = true;
            entry.failed = false;
            entry.restart_count = 0;
            flipped_active.push((node.clone(), false));
            write_buffer.push(crate::persist::writer::ColonyWriteOp::SetRegistryStatus {
                path: node.clone(),
                status: "active".into(),
                updated_at: now_status,
            });
            if entry.eager_on_reconnect {
                // Eager kind (stateless/long-running): start the task IMMEDIATELY.
                // `handle_stopped` parked it as `NotYetSpawned` with a fresh live
                // mailbox sender; `(entry.respawn)()` re-opens `cell.db` (M1
                // Resume — counter survives, no reseed) and re-spawns the cell
                // task. We swap `entry.handle` to the new sender, set status
                // `Awake`, and spawn the watcher — exactly the `handle_cell_died`
                // respawn mechanic, but in the mutation path (NO corridor touch).
                //
                // Phase-13.5 Slice 4 T6: the `RespawnFn` 3-tuple still carries no
                // `stop_tx`/`death_ack_rx` (adding them would ripple into the
                // frozen `handle_cell_died`), but the re-spawn closure now
                // RE-NOTIFIES the colony with the fresh pair via
                // `renotify_stop_wiring` (`ColonyMsg::StopWiringRestored`). The
                // colony-task restores `entry.stop_tx`/`death_ack_rx`, so a
                // SUBSEQUENT disconnect of this reconnected cell can peace-stop it
                // and commits cleanly (no `stop_wiring_unavailable` reject). The
                // re-notify is fire-and-forget on the colony inbox; if it is
                // dropped (inbox full), the interim guard remains the backstop.
                if !matches!(entry.status, CellStatus::Awake) {
                    let (new_sender, new_join, new_peace_rx, new_backstop_rx) = (entry.respawn)();
                    entry.handle = ActorHandle::new(node.clone(), new_sender);
                    entry.status = CellStatus::Awake;
                    spawn_watcher(
                        inbox_self_tx,
                        node.clone(),
                        new_join,
                        new_peace_rx,
                        new_backstop_rx,
                    );
                }
            }
            // Lazy kind (stateful): leave status `NotYetSpawned` — the wake-pre-
            // send in `route_with_log` spawns the task on the first message
            // (wake-on-message). Only the `active` flip + status persist happen
            // here.
        }
    }

    // Apply sequence step 10c (F5 variant A): inline death-ack-wait with a
    // term-timeout per deactivated Awake cell. Happy path: every death_ack fires
    // < term_timeout() → proceed to flush+commit. Timeout on ANY cell → full in-RAM
    // rollback + discard buffer → Rejected{term_timeout} (colony.db untouched).
    // Test-only deterministic sync point: the peace-stops are sent and the colony
    // is about to block on the inline death-ack-wait. Fire ONE tick so a test can
    // release a wedged cell at exactly this moment (replacing a flaky wall-clock
    // sleep). `None` in production → this is a no-op and the path is unchanged.
    if !death_acks.is_empty()
        && let Some(tx) = death_ack_wait_tx
    {
        let _ = tx.try_send(());
    }
    for (node, rx) in death_acks {
        match tokio::time::timeout(term_timeout(), rx).await {
            Ok(_) => {
                // death_ack fired (or sender dropped — task gone either way).
            }
            Err(_) => {
                tracing::warn!(
                    path = %node.as_str(),
                    "death-ack term-timeout during disconnect — rolling back mutation"
                );
                // In-RAM rollback: remove inserted edges, re-insert removed
                // copies, reset every `entry.active` flip.
                for eid in &inserted_edge_ids {
                    edges.remove(eid);
                }
                for edge in removed_edges_saved {
                    edges.insert(edge);
                }
                for (p, prior_active) in &flipped_active {
                    if let Some(entry) = registry.get_mut(p) {
                        entry.active = *prior_active;
                        // NB: only `entry.active` is restored here — the cell's
                        // stop wiring (`stop_tx`/`death_ack_rx`) was already
                        // `take()`n into the side-map above and is NOT put back.
                        // So a SUBSEQUENT disconnect of the same (wedged) cell
                        // finds `stop_tx == None` and falls through to the
                        // no-stop "just flip active" branch — documented
                        // Variante-A behavior (no second death-ack-wait).
                    }
                }
                // Discard write_buffer (never sent) → no durable effect at all.
                // No durable committed update → recovery marks the in_flight row
                // failed on next boot.
                return MutationOutcome::Rejected {
                    id: Some(id),
                    error_code: TERM_TIMEOUT_ERROR_CODE.into(),
                    details: format!("death-ack term-timeout disconnecting {}", node.as_str()),
                };
            }
        }
    }

    // Success: flush the buffered edge + status WriteOps in FIFO order BEFORE the
    // durable committed-update (Entscheidung 8a).
    for op in write_buffer {
        let _ = log_tx.send(op).await;
    }

    // Apply sequence step 11: durable committed update.
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        log_tx
            .send(crate::persist::writer::ColonyWriteOp::MutationLogUpdate {
                id: id.clone(),
                status: "committed".into(),
                committed_at: now,
                failure_reason: None,
                ack: Some(tx),
            })
            .await
            .expect("writer thread dead");
        if rx.await.is_err() {
            tracing::error!(
                "durable committed update lost — mutation stays in_flight, recovery will mark failed"
            );
            // Don't propagate the error: the FS effect (none in T14) is already done.
            // Recovery on next boot will see the in_flight row and mark failed.
        }
    }

    MutationOutcome::Committed { id }
}

/// Helper for `handle_mutation`: route an EDA error-reply to `reply_to` (if set)
/// via `route_with_log`, draining the cascade.
#[allow(clippy::too_many_arguments)]
async fn send_eda_reject(
    id: &str,
    err: &crate::mutation::MutationError,
    reply_to: Option<&Path>,
    trace_id: Uuid,
    parent_message_id: Uuid,
    registry: &mut HashMap<Path, RegistryEntry>,
    hive_scopes: &HiveScopeTable,
    dead_letters: &mut VecDeque<DeadLetter>,
    log_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
    // F2 invariant: error-reply routing must apply the SAME auto-offload policy as
    // normal routing (route_with_log) — pass the real blob store + threshold, not
    // `&None, 0`. A large error-reply body then offloads identically, so the blob
    // ref rides through transit/DLQ/persist exactly like a normal message.
    blob_store: &Option<std::sync::Arc<crate::DiskBlobStore>>,
    blob_inline_max_bytes: usize,
    // A6: the raw mutation payload — `scope` + `payload_json` for the audit row.
    // Threaded through (not derived from `err`) because the reject row is the
    // forensic record of the *rejected request*, not just the error.
    payload: &meclaw_core::JsonValue,
) {
    // A6 (Phase-16 W3): durably log this validate-stage reject into mutation_log
    // as `status='rejected'` BEFORE replying. The audit row is independent of
    // `reply_to` (it is written even when no reply target is set), and the
    // synchronous reject-reply below stays byte-for-byte unchanged. This is the
    // single funnel all validate-stage rejects pass through, so one write here
    // covers them all and makes schema/scope/naming rejects visible in
    // `/colony/mutations` (closes the K-H2 radar gap). The ack-after-commit
    // wait keeps the row durable before `handle_mutation` returns.
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let scope = payload
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("/")
            .to_string();
        let payload_json = meclaw_core::serde_json::to_string(payload).unwrap_or_default();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        if log_tx
            .send(
                crate::persist::writer::ColonyWriteOp::MutationLogRejectInsert {
                    id: id.to_string(),
                    scope,
                    payload_json,
                    error_code: err.error_code().to_string(),
                    reason: format!("{err:?}"),
                    trace_id: trace_id.to_string(),
                    created_at: now,
                    ack: Some(ack_tx),
                },
            )
            .await
            .is_ok()
        {
            let _ = ack_rx.await;
        }
    }

    let Some(rt) = reply_to else {
        return;
    };
    let err_msg = crate::mutation::build_error_reply(
        id,
        err.error_code(),
        &format!("{err:?}"),
        rt.clone(),
        trace_id,
        parent_message_id,
    );
    let mut action = route_with_log(
        &mut *registry,
        hive_scopes,
        dead_letters,
        log_tx,
        Path::new("/colony"),
        err_msg,
        blob_store,
        blob_inline_max_bytes,
    )
    .await;
    loop {
        match action {
            RouteAction::Done => break,
            RouteAction::Cascade { sender, msg } => {
                action = route_with_log(
                    &mut *registry,
                    hive_scopes,
                    dead_letters,
                    log_tx,
                    sender,
                    msg,
                    blob_store,
                    blob_inline_max_bytes,
                )
                .await;
            }
            RouteAction::HiveTransit { hive_path, msg } => {
                // Degenerate case: an EDA error-reply's `reply_to` resolves to a
                // hive. `send_eda_reject` has no `edges` in scope and must NOT
                // transit here (no second edge evaluator) — and a hive can never
                // legitimately be a mutation-reply target (hives emit no
                // mutations), so this is practically unreachable. DLQ it as
                // `UnresolvedPath` (the reply could not be delivered), mirroring
                // the ColonyDispatch DLQ arm below.
                tracing::warn!(
                    hive = %hive_path.as_str(),
                    "EDA-reject reply_to resolves to a hive — not transiting; dead-lettering UnresolvedPath"
                );
                push_dead_letter(
                    dead_letters,
                    DeadLetter {
                        sender_path: hive_path.clone(),
                        original_target: msg.target.clone(),
                        resolved_target: hive_path,
                        message: msg,
                        reason: crate::dead_letter::DeadLetterReason::UnresolvedPath,
                    },
                );
                action = RouteAction::Done;
            }
            RouteAction::ColonyDispatch {
                endpoint,
                msg,
                sender,
            } => {
                // T3 (call site no. 4): an EDA error reply with `reply_to = /colony/<x>`
                // is a DLQ case — we can NOT call dispatch_colony_endpoint here
                // (no colony_db/factories/root in the send_eda_reject scope, and a
                // re-entry into /colony/mutations would be an endless loop). DLQ push
                // directly inline with sender pass-through (must-fix #2).
                tracing::warn!(
                    endpoint = %endpoint.as_str(),
                    sender = %sender.as_str(),
                    "EDA-reject reply_to is /colony/<endpoint> — dead-letter ColonyEndpointUnimplemented"
                );
                push_dead_letter(
                    dead_letters,
                    DeadLetter {
                        sender_path: sender,
                        original_target: endpoint.clone(),
                        resolved_target: endpoint,
                        message: msg,
                        reason: crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented,
                    },
                );
                action = RouteAction::Done;
            }
        }
    }
}

/// Build a `MessageLogRow` from a pre-route message snapshot.
///
/// `from_path`: "@external" sentinel or sender path string (resolved by caller).
/// `resolved_target`: the resolved destination path (used as `to_path`).
/// `ttl` is stored post-decrement (route() will decrement; we snapshot ttl-1).
fn build_message_log_row_from_msg(
    msg: &Message,
    from_path: String,
    resolved_target: &Path,
) -> crate::persist::writer::MessageLogRow {
    let (body_kind, body_payload) = match &msg.body {
        meclaw_core::Body::Inline(v) => (
            "inline".to_string(),
            Some(meclaw_core::serde_json::to_string(v).unwrap_or_default()),
        ),
        meclaw_core::Body::Blob(uuid) => ("blob".to_string(), Some(uuid.to_string())),
    };
    crate::persist::writer::MessageLogRow {
        id: msg.id.to_string(),
        trace_id: msg.trace_id.to_string(),
        parent_message_id: msg.parent_message_id.map(|u| u.to_string()),
        correlation_id: msg.correlation_id.map(|u| u.to_string()),
        // Post-decrement: route() subtracts 1; snapshot ttl-1 here.
        ttl: (msg.ttl - 1) as i64,
        from_path,
        to_path: resolved_target.as_str().to_string(),
        reply_to: msg.reply_to.as_ref().map(|p| p.as_str().to_string()),
        headers_json: meclaw_core::serde_json::to_string(&msg.headers).unwrap_or_default(),
        body_kind,
        body_payload,
        created_at: msg.created_at,
    }
}

/// Virtual `/colony/*` endpoint handling. Phase-2 reality:
///   - `/colony` (bare, no subpath): not addressable per spec Z. 407 → ColonyEndpointInvalid.
///   - `/colony/dead_letters`: spec-symmetric read+drain requires `reply_to` (Phase 3+).
///     In Phase 2 an incoming message here cannot be answered → ColonyEndpointUnimplemented.
///   - Any other `/colony/<x>` (registry, templates, mutations, graph, trace, events):
///     belongs to later phases → ColonyEndpointUnimplemented.
fn handle_colony_target(
    dead_letters: &mut VecDeque<DeadLetter>,
    sender_path: Path,
    original_target: Path,
    resolved_target: Path,
    message: Message,
) {
    let reason = if resolved_target.as_str() == "/colony" {
        crate::dead_letter::DeadLetterReason::ColonyEndpointInvalid
    } else {
        crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented
    };
    tracing::warn!(
        sender = %sender_path.as_str(),
        original = %original_target.as_str(),
        resolved = %resolved_target.as_str(),
        reason = ?reason,
        "colony endpoint dead-letter"
    );
    push_dead_letter(
        dead_letters,
        DeadLetter {
            sender_path,
            original_target,
            resolved_target,
            message,
            reason,
        },
    );
}

/// Phase-3a full cascade per spec § Behavior on routing errors:
///   1. reply_to set → the error reply (terminal, reply_to=None) is returned to
///      the outer routing loop; it runs through the regular route().
///   2. otherwise → /colony/dead_letters.
///
/// "Terminal" means: the error reply itself has no reply_to set; its own routing
/// miss goes straight to step 2 (DLQ), no further cascade
/// (spec § "cascade is one-shot"). Maximum cascade depth = 2 hops.
fn handle_unresolved(
    dead_letters: &mut VecDeque<DeadLetter>,
    sender_path: Path,
    original_target: Path,
    resolved_target: Path,
    message: Message,
) -> Option<(Path, Message)> {
    tracing::warn!(
        sender = %sender_path.as_str(),
        original = %original_target.as_str(),
        resolved = %resolved_target.as_str(),
        trace_id = %message.trace_id,
        reason = "UnresolvedPath",
        "routing dead-letter / cascade"
    );

    if let Some(reply_target) = message.reply_to.clone() {
        // Step 1: the error reply is terminal — through the outer loop via route().
        let error_reply = MessageBuilder::new(reply_target)
            .trace_id(message.trace_id)
            .parent_message_id(message.id)
            .ttl(message.ttl) // already decremented by route() → decrement further on the next hop
            .build();
        // error_reply.reply_to is None by default → terminal.
        // The sender_path for the next route() call is "/colony" — the cascade has
        // no emitting cell; a virtual colony address as sender is consistent with
        // spec § routing symmetry.
        return Some((Path::new("/colony"), error_reply));
    }

    // Step 2: reply_to == None → straight to the DLQ.
    push_dead_letter(
        dead_letters,
        DeadLetter {
            sender_path,
            original_target,
            resolved_target,
            message,
            reason: crate::dead_letter::DeadLetterReason::UnresolvedPath,
        },
    );
    None
}

/// Push a dead letter into the transient in-memory DLQ buffer.
///
/// **Phase-16 W6d (A6): no eviction.** The DLQ is now persisted in `colony.db`
/// (the single source of truth); the `VecDeque` is a transient hand-off buffer
/// that the single-owner `colony_task` flushes to the `dead_letters` table after
/// every handled event (`persist_dead_letters`). drop-oldest is gone — the
/// diagnostic truth is kept, not evicted (ruling, W6d). `push_dead_letter`
/// stays pure (no DB I/O, no `.await`) so it remains callable from inside the
/// byte-frozen `route()` corridor.
///
/// Phase-13.5-A6-T3: `pub(crate)` so `colony_dispatch::dispatch_colony_endpoint`
/// can DLQ-push for `ColonyEndpointUnimplemented` on unknown `/colony/<x>`.
pub(crate) fn push_dead_letter(queue: &mut VecDeque<DeadLetter>, dl: DeadLetter) {
    queue.push_back(dl);
}

/// W6d (A6): drain the transient in-memory DLQ buffer into the durable
/// `dead_letters` table. Called by the single-owner `colony_task` after each
/// handled event (and inside `route_with_log` after the `route()` return), so
/// the DB is the sole DLQ truth — `/colony/dead_letters` Read/Drain query the DB,
/// never the `VecDeque`, which is left empty and never accumulates. Fire-and-
/// forget per the message-log precedent (a diagnostic write must not backpressure
/// routing); FIFO `pop_front` preserves insertion order in the table.
async fn persist_dead_letters(
    dead_letters: &mut VecDeque<DeadLetter>,
    writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
) {
    while let Some(dl) = dead_letters.pop_front() {
        let message_json = message_to_json(&dl.message);
        let _ = writer_tx
            .send(crate::persist::writer::ColonyWriteOp::InsertDeadLetter {
                sender_path: dl.sender_path.as_str().to_string(),
                original_target: dl.original_target.as_str().to_string(),
                resolved_target: dl.resolved_target.as_str().to_string(),
                error_code: dl.reason.as_code().to_string(),
                trace_id: dl.message.trace_id.to_string(),
                created_at: dl.message.created_at,
                message_json,
            })
            .await;
    }
}

/// W6d (A6): write-fence for deterministic read-after-write on the DLQ. Sends a
/// no-op `Fence` op and awaits its post-commit ack — by FIFO this guarantees every
/// prior fire-and-forget `InsertDeadLetter` is durable before a subsequent
/// `read_dead_letters` on the read-only connection runs. Takes `&writer_tx` (Send +
/// Sync) — NOT `&ColonyDb` (`!Sync`) — so the holding `colony_task` future stays
/// `Send` across this await (the read itself is a separate, await-free borrow).
async fn fence(writer_tx: &tokio::sync::mpsc::Sender<crate::persist::writer::ColonyWriteOp>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if writer_tx
        .send(crate::persist::writer::ColonyWriteOp::Fence { ack: tx })
        .await
        .is_ok()
    {
        let _ = rx.await;
    }
}

/// W6d (A6): serialize a `Message` envelope into the JSON stored in the
/// `dead_letters.message_json` column. Reuses the same primitives as
/// `build_message_log_row_from_msg` (serde for `headers`, `serde_json` for an
/// inline body, `to_string` for uuids/paths) but captures the VERBATIM envelope
/// (pre-resolution `target`, un-decremented `ttl`) so the DLQ-drain reconstructs
/// the dead-lettered message exactly. Counterpart: [`message_from_json`].
fn message_to_json(m: &Message) -> String {
    let (body_kind, body_payload) = match &m.body {
        meclaw_core::Body::Inline(v) => ("inline", v.clone()),
        meclaw_core::Body::Blob(u) => (
            "blob",
            meclaw_core::serde_json::Value::String(u.to_string()),
        ),
    };
    let obj = meclaw_core::serde_json::json!({
        "id": m.id.to_string(),
        "trace_id": m.trace_id.to_string(),
        "parent_message_id": m.parent_message_id.map(|u| u.to_string()),
        "correlation_id": m.correlation_id.map(|u| u.to_string()),
        "target": m.target.as_str(),
        "reply_to": m.reply_to.as_ref().map(|p| p.as_str()),
        "ttl": m.ttl,
        "headers": m.headers,
        "body_kind": body_kind,
        "body_payload": body_payload,
        "created_at": m.created_at,
    });
    meclaw_core::serde_json::to_string(&obj).unwrap_or_default()
}

/// W6d (A6): inverse of [`message_to_json`] — reconstruct a `Message` from the
/// `dead_letters.message_json` column. The data was written by this substrate, so
/// each field parses defensively (nil-uuid / empty-path / null-body fallback)
/// rather than hard-failing the whole drain on a single malformed row.
fn message_from_json(s: &str) -> Message {
    let v: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(s).unwrap_or(meclaw_core::serde_json::Value::Null);
    let opt_uuid = |key: &str| -> Option<meclaw_core::Uuid> {
        v.get(key)
            .and_then(|x| x.as_str())
            .and_then(|s| meclaw_core::Uuid::parse_str(s).ok())
    };
    let body = match v.get("body_kind").and_then(|x| x.as_str()) {
        Some("blob") => meclaw_core::Body::Blob(
            v.get("body_payload")
                .and_then(|x| x.as_str())
                .and_then(|s| meclaw_core::Uuid::parse_str(s).ok())
                .unwrap_or_else(meclaw_core::Uuid::nil),
        ),
        _ => meclaw_core::Body::Inline(
            v.get("body_payload")
                .cloned()
                .unwrap_or(meclaw_core::serde_json::Value::Null),
        ),
    };
    let headers = v
        .get("headers")
        .cloned()
        .and_then(|h| meclaw_core::serde_json::from_value(h).ok())
        .unwrap_or_default();
    Message {
        id: opt_uuid("id").unwrap_or_else(meclaw_core::Uuid::nil),
        trace_id: opt_uuid("trace_id").unwrap_or_else(meclaw_core::Uuid::nil),
        parent_message_id: opt_uuid("parent_message_id"),
        correlation_id: opt_uuid("correlation_id"),
        target: Path::new(v.get("target").and_then(|x| x.as_str()).unwrap_or("")),
        reply_to: v.get("reply_to").and_then(|x| x.as_str()).map(Path::new),
        ttl: v.get("ttl").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        headers,
        body,
        created_at: v.get("created_at").and_then(|x| x.as_i64()).unwrap_or(0),
    }
}

/// W6d (A6): reconstruct a full `DeadLetter` from a persisted `dead_letters` row.
/// The 6 projection fields come from the columns; the `Message` envelope from
/// `message_json`; the `reason` enum maps back from the canonical `error_code`
/// (unknown code → `NoRoute` rather than dropping the row). Used by the DLQ-drain
/// to return `Vec<DeadLetter>` from the DB unchanged for callers.
pub(crate) fn dead_letter_from_row(row: crate::persist::colony_db::DeadLetterRow) -> DeadLetter {
    let reason = crate::dead_letter::DeadLetterReason::from_code(&row.error_code)
        .unwrap_or(crate::dead_letter::DeadLetterReason::NoRoute);
    DeadLetter {
        sender_path: Path::new(&row.sender_path),
        original_target: Path::new(&row.original_target),
        resolved_target: Path::new(&row.resolved_target),
        message: message_from_json(&row.message_json),
        reason,
    }
}

/// Normalise a raw `guard_scope` string into a canonical prefix used for
/// parent-path comparisons in the registry-name scope filter.
///
/// Trims any trailing `/` and treats an empty result as `"/"` (root scope).
/// Examples: `""` → `"/"`, `"/"` → `"/"`, `"/main"` → `"/main"`,
/// `"/main/"` → `"/main"`.
fn canonical_scope_prefix(guard_scope: &str) -> String {
    let s = guard_scope.trim_end_matches('/');
    if s.is_empty() {
        "/".to_string()
    } else {
        s.to_string()
    }
}

/// Split a cell's `content` JSON into its `header`-block (if present, must
/// be an Object) and the remaining body candidate. Spec § "Headers vs.
/// Body — Schreibmodell".
fn split_content_header(mut content: Value) -> (Map<String, Value>, Value) {
    let header_value = match &mut content {
        Value::Object(map) => map.remove("header"),
        _ => None,
    };
    let cell_headers = match header_value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    (cell_headers, content)
}

/// Build the follow-up Message from a CellEmission. Header-Merge per
/// Spec § 822: input-headers first, then cell-emitted content.header
/// overlays (last-write-wins). The body is the content with the
/// `header`-block stripped.
///
/// Retained as a test helper; production code uses `build_follow_up_with`.
#[cfg(test)]
fn build_follow_up_message(em: CellEmission) -> Message {
    // Two-compartment decay: context travels through, hop = isolated cell output
    // (input.hop is dropped, structural freshness, ADR-0001).
    let (cell_hop, body_value) = split_content_header(em.content);
    let merged = em.input_headers.carry_context_with_hop(cell_hop);
    MessageBuilder::new(em.target)
        .trace_id(em.trace_id)
        .parent_message_id_opt(em.parent_message_id)
        .reply_to(em.sender_path)
        .ttl(em.input_ttl)
        .headers(merged)
        .body(Body::Inline(body_value))
        .build()
}

/// Build a follow-up Message with an explicit target and headers override.
///
/// Used by the outputs-arm to support edge overlays (Phase 4) — both the
/// no-edge case (target = em.target) and the edge-overlay case (target =
/// edge.to) go through this single path. The body is extracted from
/// em.content with the `header`-block stripped.
fn build_follow_up_with(em: CellEmission, target: Path, headers_out: Headers) -> Message {
    let (_cell_hop, body_value) = split_content_header(em.content);
    MessageBuilder::new(target)
        .trace_id(em.trace_id)
        .parent_message_id_opt(em.parent_message_id)
        .reply_to(em.sender_path)
        .ttl(em.input_ttl)
        .headers(headers_out)
        .body(Body::Inline(body_value))
        .build()
}

/// GH #82 (ruling 2026-08-13): apply a restoring edge's `modifier.restore_ttl`
/// to a follow-up message that has already been built.
///
/// `ttl` is envelope, and the envelope setter is the colony alone — so the edge
/// only DECLARES the restore (`EdgeDecision::restore_ttl`) and this is where the
/// colony carries it out, outside the frozen `route()` corridor, exactly like
/// the loud TTL-death pre-check above.
///
/// Two properties make this a reset rather than a hole in the loop guard:
///
/// - **never accumulates**: the result is `budget`, not `ttl + budget`. N
///   restores and one restore leave the same ceiling, so a restoring cycle can
///   never grow its own budget.
/// - **never lowers**: a message that entered with MORE than the colony budget
///   (an ingress that asked for a bigger one, `ttl` field of `POST /messages`)
///   keeps what it has. So no restore ever lifts a message above the larger of
///   its ingress budget and the colony default — "restore", not "grant".
///
/// What the restore does remove is TTL as the bound of that cycle. That is the
/// point of the ruling: a restoring edge declares its loop legitimate, and the
/// runaway guard for it is the iteration bound the same edge carries — which is
/// why an unconditional restoring edge is rejected at config load.
fn restore_edge_ttl(current: u32, budget: u32) -> u32 {
    current.max(budget)
}

/// Build a transit follow-up for a hive out-edge match.
///
/// A hive is a **transparent router**: the message is *forwarded*, not
/// *consumed*. Unlike `build_follow_up_with` (the outputs-arm helper, which sets
/// `reply_to = em.sender_path` because the cell answers), this helper passes
/// `reply_to` and `correlation_id` through UNCHANGED (transparency mandate,
/// Auflage 2) — re-pointing them at the hive/sender would break the req/resp
/// pairing the hive is supposed to be invisible to. `parent_message_id` is
/// chained to the incoming message (`Some(src.id)`) so the transit hop is
/// visible in the message-log trace tree (Brainstorm F2). `ttl` is carried
/// verbatim; `route()` decrements it on re-entry, yielding TTL-per-hop. The
/// source is a `Message` (flat `headers`), so there is no content-header split.
fn build_transit_follow_up(src: &Message, target: Path, headers_out: Headers) -> Message {
    MessageBuilder::new(target)
        .trace_id(src.trace_id)
        .parent_message_id_opt(Some(src.id))
        .reply_to_opt(src.reply_to.clone())
        .correlation_id_opt(src.correlation_id)
        .ttl(src.ttl)
        .headers(headers_out)
        .body(src.body.clone())
        .build()
}

/// Push the follow-up of a `ColonyDispatch` step into the routing work-queue.
///
/// `dispatch_colony_endpoint` returns a `RouteAction`; in practice it yields
/// `Done` (terminal) or `Cascade` (e.g. an EDA reply that must route on). It
/// never re-emits `ColonyDispatch`/`HiveTransit` — those arms are defensive:
/// an unexpected re-dispatch is dead-lettered rather than looped, so a buggy
/// dispatcher can never spin the work-queue.
fn enqueue_dispatch_follow(
    work: &mut VecDeque<(Path, Message)>,
    dead_letters: &mut VecDeque<DeadLetter>,
    follow: RouteAction,
) {
    match follow {
        RouteAction::Done => {}
        RouteAction::Cascade { sender, msg } => work.push_back((sender, msg)),
        RouteAction::ColonyDispatch {
            endpoint,
            msg,
            sender,
        } => {
            tracing::warn!(
                endpoint = %endpoint.as_str(),
                sender = %sender.as_str(),
                "dispatch_colony_endpoint re-emitted ColonyDispatch — unexpected; dead-lettering"
            );
            push_dead_letter(
                dead_letters,
                DeadLetter {
                    sender_path: sender,
                    original_target: endpoint.clone(),
                    resolved_target: endpoint,
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::ColonyEndpointUnimplemented,
                },
            );
        }
        RouteAction::HiveTransit { hive_path, msg } => {
            tracing::warn!(
                hive = %hive_path.as_str(),
                "dispatch_colony_endpoint re-emitted HiveTransit — unexpected; dead-lettering"
            );
            push_dead_letter(
                dead_letters,
                DeadLetter {
                    sender_path: hive_path.clone(),
                    original_target: msg.target.clone(),
                    resolved_target: hive_path,
                    message: msg,
                    reason: crate::dead_letter::DeadLetterReason::HiveNoRoute,
                },
            );
        }
    }
}

/// Evaluate a hive's out-edges (the single edge evaluator `apply_edges`) and
/// enqueue one transit follow-up per match. No match (edge-list empty or all
/// CEL conditions false) → DLQ `hive_no_route` (Spec Z.553, trennscharf zu
/// `unresolved_path`: the hive WAS reachable, the graph just did not forward).
fn enqueue_hive_transit(
    work: &mut VecDeque<(Path, Message)>,
    dead_letters: &mut VecDeque<DeadLetter>,
    edges: &EdgeTable,
    hive_path: Path,
    msg: Message,
    egress_tx: Option<&mpsc::Sender<Message>>,
    ttl_budget: u32,
) {
    let decisions = apply_edges(edges, &hive_path, &msg.headers);
    if decisions.is_empty() {
        // stdio-Bridge (Direct-Mode): at the ROOT hive `/`, an unroutable message
        // is the egress edge to the outside (stdout) rather than a dead end. With
        // an egress sink set, hand it over instead of dead-lettering. `try_send`
        // keeps this fn sync; a full/closed channel is a warn (no silent drop).
        if hive_path.as_str() == "/"
            && let Some(tx) = egress_tx
        {
            if let Err(e) = tx.try_send(msg) {
                tracing::warn!(
                    reason = "egress_full_or_closed",
                    error = %e,
                    "root-hive egress drop (stdout consumer slow/gone)"
                );
            }
            return;
        }
        // W2 (Ruling A2): de-collapse the diagnosis — the `sender` is the cell
        // that emitted INTO the dead-end hive (its `reply_to`), not the hive
        // itself; and emit the previously-missing ops-log line carrying the
        // `trace_id` (O1/O2 diagnose-armut). The entry is self-locating via the
        // message envelope (trace_id + created_at surfaced by `DeadLetterDto`).
        let origin_sender = msg.reply_to.clone().unwrap_or_else(|| hive_path.clone());
        tracing::warn!(
            sender = %origin_sender.as_str(),
            hive = %hive_path.as_str(),
            trace_id = %msg.trace_id,
            reason = "HiveNoRoute",
            "hive out-edge matched nothing — dead-letter (hive_no_route)"
        );
        push_dead_letter(
            dead_letters,
            DeadLetter {
                sender_path: origin_sender,
                original_target: msg.target.clone(),
                resolved_target: hive_path,
                message: msg,
                reason: crate::dead_letter::DeadLetterReason::HiveNoRoute,
            },
        );
    } else {
        for dec in decisions {
            let restores = dec.restore_ttl;
            let mut transit = build_transit_follow_up(&msg, dec.target, dec.headers_out);
            // GH #82: a hive out-edge may declare `restore_ttl` too — same edge
            // schema, same semantics. Applied here rather than inside the
            // builder so both edge kinds share one restore rule.
            if restores {
                transit.ttl = restore_edge_ttl(transit.ttl, ttl_budget);
            }
            work.push_back((hive_path.clone(), transit));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell_task::cell_task;
    use meclaw_core::{CellEmission, Message, MessageBuilder, Path};
    use meclaw_testing::mocks::EchoMockCell;
    use meclaw_testing::mocks::FailOnDemandMockCell;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    /// Minimal `ColonyTaskConfig` for builder-field unit tests. Mirrors the
    /// `ColonyTaskConfig::new` call form used by the spawn-tests in this module.
    /// The returned config is inspected, not run, so the temp dir backing the
    /// `colony.db` may drop afterwards.
    fn make_test_config() -> ColonyTaskConfig {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let td = tempfile::TempDir::new().unwrap();
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        ColonyTaskConfig::new(
            inbox_tx,
            inbox_rx,
            out_tx,
            out_rx,
            db,
            crate::CellFactoryRegistry::new(),
            td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )
    }

    #[test]
    fn colony_task_config_with_egress_sets_field() {
        let (egress_tx, _egress_rx) = mpsc::channel::<Message>(8);
        let cfg = make_test_config().with_egress(egress_tx);
        assert!(cfg.egress_tx.is_some(), "with_egress must set egress_tx");
    }

    #[tokio::test]
    async fn root_hive_no_route_goes_to_egress_when_set() {
        let (egress_tx, mut egress_rx) = mpsc::channel::<Message>(8);
        let mut work: VecDeque<(Path, Message)> = VecDeque::new();
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let edges = EdgeTable::new(); // no out-edge at "/" → decisions empty
        let msg = MessageBuilder::new(Path::new("/")).build();
        enqueue_hive_transit(
            &mut work,
            &mut dead_letters,
            &edges,
            Path::new("/"),
            msg,
            Some(&egress_tx),
            meclaw_core::MESSAGE_DEFAULT_TTL,
        );
        assert!(
            dead_letters.is_empty(),
            "root-hive egress must NOT dead-letter"
        );
        assert!(
            egress_rx.try_recv().is_ok(),
            "message must land in egress channel"
        );
    }

    #[tokio::test]
    async fn non_root_hive_no_route_still_dead_letters() {
        let (egress_tx, _rx) = mpsc::channel::<Message>(8);
        let mut work: VecDeque<(Path, Message)> = VecDeque::new();
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let edges = EdgeTable::new();
        let msg = MessageBuilder::new(Path::new("/sub")).build();
        enqueue_hive_transit(
            &mut work,
            &mut dead_letters,
            &edges,
            Path::new("/sub"),
            msg,
            Some(&egress_tx),
            meclaw_core::MESSAGE_DEFAULT_TTL,
        );
        assert_eq!(
            dead_letters.len(),
            1,
            "non-root hive HiveNoRoute → DLQ unchanged"
        );
    }

    /// Phase-13-G-3 test helper: stateless/long-running cells stay Awake → wake
    /// must never be invoked. Mirror of the no-op-with-error-log WakeFn used by
    /// `handle_mutation`'s spawn-loop and `bootstrap_apply`.
    /// Paket-3 P3-B-restart test helper: a `backstop_rx` whose sender is dropped
    /// immediately → the watcher never sees a backstop signal (death classifies
    /// `Normal`/`Panic`). Used by Register-test-sites that do not exercise the
    /// B-backstop restart path.
    fn dropped_backstop_rx() -> tokio::sync::oneshot::Receiver<()> {
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
        rx
    }

    /// W6d (A6) step c: `push_dead_letter` no longer evicts — drop-oldest is gone.
    /// The diagnostic truth is kept (the single-owner `colony_task` flushes the
    /// transient buffer into the durable `dead_letters` table after every event).
    /// Pushing well past the former 1000-cap retains every entry.
    #[test]
    fn push_dead_letter_keeps_all_entries_no_drop_oldest() {
        let mut q: VecDeque<DeadLetter> = VecDeque::new();
        for _ in 0..1005 {
            push_dead_letter(
                &mut q,
                DeadLetter {
                    sender_path: Path::new("/a"),
                    original_target: Path::new("/b"),
                    resolved_target: Path::new("/b"),
                    message: MessageBuilder::new(Path::new("/b")).build(),
                    reason: crate::dead_letter::DeadLetterReason::TtlExpired,
                },
            );
        }
        assert_eq!(q.len(), 1005, "no eviction past the former cap");
    }

    /// W6d (A6): the `message_json` envelope round-trips every field the DLQ-drain
    /// reconstructs — including `body` (inline payload), `correlation_id`, `target`
    /// (verbatim, pre-resolution) and `headers`. This is what keeps the ~40 tests
    /// that inspect `dl.message.*` green after the drain switched to DB-backed
    /// reconstruction (Ruling W6d Option 1).
    #[test]
    fn message_json_round_trips_full_envelope() {
        use meclaw_core::{Body, Uuid};
        let src = Message {
            id: Uuid::now_v7(),
            trace_id: Uuid::now_v7(),
            parent_message_id: Some(Uuid::now_v7()),
            correlation_id: Some(Uuid::now_v7()),
            target: Path::new("/dst"),
            reply_to: Some(Path::new("/sender")),
            ttl: 7,
            headers: meclaw_core::Headers::default(),
            body: Body::Inline(meclaw_core::serde_json::json!({"k": "v", "n": 3})),
            created_at: 12345,
        };
        let back = message_from_json(&message_to_json(&src));
        assert_eq!(back.id, src.id);
        assert_eq!(back.trace_id, src.trace_id);
        assert_eq!(back.parent_message_id, src.parent_message_id);
        assert_eq!(back.correlation_id, src.correlation_id);
        assert_eq!(back.target.as_str(), "/dst");
        assert_eq!(back.reply_to.as_ref().map(|p| p.as_str()), Some("/sender"));
        assert_eq!(back.ttl, 7);
        assert_eq!(back.created_at, 12345);
        assert_eq!(back.body, src.body);
        assert_eq!(back.headers, src.headers);
    }

    /// Register a stateless cell with a live join-handle under `path` and wait
    /// for the ack. Defaults all lifecycle wiring (no peace-stop, no backstop,
    /// `active=true`, unreachable respawn) for plain outputs-arm topology tests.
    async fn register_simple_cell(
        inbox_tx: &mpsc::Sender<ColonyMsg>,
        path: Path,
        sender: mpsc::Sender<Message>,
        join: JoinHandle<()>,
    ) {
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path,
                sender,
                join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();
    }

    /// Phase-13-H-1: smoke-test that the `ColonyMsg::Sleep` variant exists and
    /// can be constructed. Full select!-arm semantics arrive in 13-H-3.
    #[test]
    fn colony_msg_sleep_variant_compiles() {
        let (_tx, rx) = mpsc::channel::<meclaw_core::Message>(8);
        let _m = ColonyMsg::Sleep {
            path: meclaw_core::Path::new("/x"),
            receiver: rx,
        };
    }

    /// Phase-13.5 Lifecycle-3b Task 3: `ColonyMsg::Stopped` variant exists and
    /// can be constructed (distinct from `Sleep`).
    #[test]
    fn colony_msg_stopped_variant_compiles() {
        let (_tx, rx) = mpsc::channel::<meclaw_core::Message>(8);
        let _m = ColonyMsg::Stopped {
            path: meclaw_core::Path::new("/x"),
            receiver: rx,
        };
    }

    /// Phase-13.5 Slice 4 T5: a `StopWiringRestored` for a present cell puts a
    /// fresh `(stop_tx, death_ack_rx)` pair back onto the `RegistryEntry` fields
    /// (the persistent home of a cell's stop-wiring). The disconnect path `take()`s
    /// both → `None`; restoration must flip them back to `Some`. An absent cell
    /// drops the pair without panic.
    #[tokio::test]
    async fn stop_wiring_restored_msg_repairs_registry_and_side_map() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let path = Path::new("/x");
        let (sender, _rx) = mpsc::channel::<Message>(8);
        // Post-disconnect state: stop_tx + death_ack_rx already taken → None.
        registry.insert(
            path.clone(),
            RegistryEntry {
                handle: meclaw_core::ActorHandle::new(path.clone(), sender),
                respawn: Box::new(|| unreachable!()),
                wake: None,
                restart_count: 0,
                restart_limit: 5,
                cell_id: uuid::Uuid::now_v7(),
                cell_type: "stub".into(),
                status: CellStatus::Awake,
                eager_on_reconnect: true,
                active: true,
                failed: false,
                stop_tx: None,
                death_ack_rx: None,
            },
        );

        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        handle_stop_wiring_restored(&mut registry, path.clone(), stop_tx, death_ack_rx);

        let e = registry.get(&path).expect("entry stays per No-Delete");
        assert!(e.stop_tx.is_some(), "stop_tx must be restored");
        assert!(e.death_ack_rx.is_some(), "death_ack_rx must be restored");

        // Ghost-path: an absent cell drops the pair, no panic.
        let mut empty = std::collections::HashMap::<Path, RegistryEntry>::new();
        let (g_stop_tx, _g_stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_g_ack_tx, g_death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        handle_stop_wiring_restored(&mut empty, Path::new("/gone"), g_stop_tx, g_death_ack_rx);
        assert!(empty.is_empty());
    }

    /// Phase-13.5 Lifecycle-3b Task 4 (4.6): pin the canonical `term_timeout`
    /// mutation-reject error_code string (stable API contract). Listed in the
    /// spec `error_code` enum (`docs/meclaw-overview.md` § Validation).
    #[test]
    fn term_timeout_error_code_is_pinned() {
        assert_eq!(super::TERM_TIMEOUT_ERROR_CODE, "term_timeout");
    }

    /// Pin the canonical `stop_wiring_unavailable` mutation-reject error_code
    /// string (stable API contract), analog to `term_timeout`. Listed in the
    /// spec `error_code` enum (`docs/meclaw-overview.md` § Validation).
    #[test]
    fn stop_wiring_unavailable_error_code_is_pinned() {
        assert_eq!(
            super::STOP_WIRING_UNAVAILABLE_ERROR_CODE,
            "stop_wiring_unavailable"
        );
    }

    /// Phase-13.5 Lifecycle-3b Task 4 (A3): `handle_stopped` drains the returned
    /// mailbox remainder into the DLQ as `cell_inactive` (order preserved,
    /// sender = each message's `reply_to`) and swaps a FRESH channel pair into
    /// the registry entry so a later reconnect-wake hits a live sender (no "dead
    /// sender"). The drained old receiver is dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_stopped_drains_remainder_to_dlq_and_swaps_channel() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let path = Path::new("/x");
        let (sender, receiver) = mpsc::channel::<Message>(8);
        // Pre-register an entry whose handle wraps the OLD sender — A3 must swap
        // it for a fresh one and set status NotYetSpawned + active=false.
        registry.insert(
            path.clone(),
            RegistryEntry {
                handle: meclaw_core::ActorHandle::new(path.clone(), sender.clone()),
                respawn: Box::new(|| unreachable!()),
                wake: None,
                restart_count: 0,
                restart_limit: 5,
                cell_id: uuid::Uuid::now_v7(),
                cell_type: "stub".into(),
                status: CellStatus::Awake,
                eager_on_reconnect: true,
                active: true,
                failed: false,
                stop_tx: None,
                death_ack_rx: None,
            },
        );
        // Buffer two unread messages — the DLQ remainder, with distinct senders.
        sender
            .send(
                MessageBuilder::new(path.clone())
                    .reply_to(Path::new("/src-a"))
                    .build(),
            )
            .await
            .unwrap();
        sender
            .send(
                MessageBuilder::new(path.clone())
                    .reply_to(Path::new("/src-b"))
                    .build(),
            )
            .await
            .unwrap();
        drop(sender); // only the registry's swapped sender should remain afterwards

        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        handle_stopped(&mut registry, &mut dead_letters, path.clone(), receiver).await;

        // Two DLQ entries, order preserved, reason cell_inactive, sender = reply_to.
        assert_eq!(dead_letters.len(), 2);
        assert_eq!(
            dead_letters[0].reason,
            crate::dead_letter::DeadLetterReason::CellInactive
        );
        assert_eq!(dead_letters[0].sender_path.as_str(), "/src-a");
        assert_eq!(dead_letters[1].sender_path.as_str(), "/src-b");
        assert_eq!(dead_letters[0].resolved_target.as_str(), "/x");

        // Entry still present, deactivated, NotYetSpawned, sender ALIVE.
        let e = registry.get(&path).expect("entry stays per No-Delete");
        assert!(!e.active);
        assert!(matches!(e.status, CellStatus::NotYetSpawned { .. }));
        // The swapped sender must accept a message (proves it is not the dropped one).
        e.handle
            .send(MessageBuilder::new(path.clone()).build())
            .await
            .expect("swapped sender is alive for reconnect-wake");
    }

    /// Phase-13.5 Lifecycle-3b Task 4 (4.7, A4): the mailbox-remainder → DLQ
    /// drain is cell-kind-agnostic — a stateful, a long-running, and a stateless
    /// cell all hand the colony the SAME `mpsc::Receiver<Message>` on stop, so
    /// `handle_stopped` produces exactly N `cell_inactive` dead-letters per cell,
    /// order preserved, `sender` = each message's source (`reply_to`). This pins
    /// that contract over the three kinds via three independent disconnects.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_stopped_drains_exactly_n_per_kind_order_and_sender() {
        // (cell-path, [(source, N messages from that source)]). The three rows
        // stand for the three cell kinds — each disconnects with its own unread
        // remainder; the drain logic is identical, the assertions are per-cell.
        let cases = [
            ("/stateful", &[("/u1", 2usize)][..]),
            ("/long_running", &[("/u2", 3usize)][..]),
            ("/stateless", &[("/u3", 1usize)][..]),
        ];
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        for (cell_path, sources) in cases {
            let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
            let path = Path::new(cell_path);
            let (sender, receiver) = mpsc::channel::<Message>(16);
            registry.insert(
                path.clone(),
                RegistryEntry {
                    handle: meclaw_core::ActorHandle::new(path.clone(), sender.clone()),
                    respawn: Box::new(|| unreachable!()),
                    wake: None,
                    restart_count: 0,
                    restart_limit: 5,
                    cell_id: uuid::Uuid::now_v7(),
                    cell_type: "stub".into(),
                    status: CellStatus::Awake,
                    eager_on_reconnect: true,
                    active: true,
                    failed: false,
                    stop_tx: None,
                    death_ack_rx: None,
                },
            );
            // Fill the mailbox with N messages from each source, in order.
            let mut expected_senders: Vec<String> = Vec::new();
            for (src, n) in sources {
                for _ in 0..*n {
                    sender
                        .send(
                            MessageBuilder::new(path.clone())
                                .reply_to(Path::new(src))
                                .build(),
                        )
                        .await
                        .unwrap();
                    expected_senders.push((*src).to_string());
                }
            }
            drop(sender);

            let before = dead_letters.len();
            handle_stopped(&mut registry, &mut dead_letters, path.clone(), receiver).await;
            let produced: Vec<_> = dead_letters.iter().skip(before).collect();
            // Exactly N, all cell_inactive, resolved to this cell, order/sender preserved.
            assert_eq!(
                produced.len(),
                expected_senders.len(),
                "{cell_path}: N entries"
            );
            for (dl, exp_src) in produced.iter().zip(expected_senders.iter()) {
                assert_eq!(
                    dl.reason,
                    crate::dead_letter::DeadLetterReason::CellInactive,
                    "{cell_path}: reason"
                );
                assert_eq!(
                    dl.resolved_target.as_str(),
                    cell_path,
                    "{cell_path}: target"
                );
                assert_eq!(
                    &dl.sender_path.as_str().to_string(),
                    exp_src,
                    "{cell_path}: sender order"
                );
            }
        }
    }

    /// Phase-13-H-3: ghost-path smoke — Sleep for an unknown path silently
    /// returns (no panic, no insert). Receiver is dropped on early return.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_sleep_unknown_path_silently_returns() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (_sender, receiver) = mpsc::channel::<Message>(8);
        let mut dead_letters = std::collections::VecDeque::new();
        handle_sleep(
            &mut registry,
            &mut dead_letters,
            &inbox_tx,
            Path::new("/ghost"),
            receiver,
        )
        .await;
        assert!(registry.is_empty());
    }

    /// F1-KH2 Schicht 2 (defense-in-depth pin): a delivery to an ACTIVE but
    /// PARKED cell whose registration carries NO wake mechanic (`wake == None`)
    /// must fail LOUDLY — the message goes to the DLQ (`cell_inactive`) and the
    /// status stays parked (NO false `Awake`). Pre-fix, the inert wake closure
    /// dropped the parked receiver: silent loss + lying lifecycle status.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parked_cell_without_wake_mechanic_dead_letters_loudly() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let path = Path::new("/inert");
        let (sender, receiver) = mpsc::channel::<Message>(1);
        registry.insert(
            path.clone(),
            RegistryEntry {
                handle: meclaw_core::ActorHandle::new(path.clone(), sender),
                respawn: Box::new(|| unreachable!()),
                // No wake mechanic installed (eager-kind / fallback shape).
                wake: None,
                restart_count: 0,
                restart_limit: 5,
                cell_id: uuid::Uuid::now_v7(),
                cell_type: "stub".into(),
                status: CellStatus::NotYetSpawned { receiver },
                eager_on_reconnect: false,
                active: true,
                failed: false,
                stop_tx: None,
                death_ack_rx: None,
            },
        );
        let hive_scopes = HiveScopeTable::new();
        let mut dead_letters = std::collections::VecDeque::new();
        let (log_tx, mut log_rx) = mpsc::channel::<crate::persist::writer::ColonyWriteOp>(8);
        let msg = MessageBuilder::new(path.clone())
            .body(meclaw_core::Body::Inline(meclaw_core::serde_json::json!({
                "messages": [{"origin":"user","type":"text","text":"ping"}]
            })))
            .ttl(4)
            .build();

        let action = route_with_log(
            &mut registry,
            &hive_scopes,
            &mut dead_letters,
            &log_tx,
            Path::new("/"),
            msg,
            &None,
            usize::MAX,
        )
        .await;

        assert!(
            matches!(action, RouteAction::Done),
            "delivery must terminate (no cascade)"
        );
        assert_eq!(dead_letters.len(), 1, "message must be dead-lettered");
        let dl = &dead_letters[0];
        assert_eq!(
            dl.reason,
            crate::dead_letter::DeadLetterReason::CellInactive,
            "reason must be cell_inactive"
        );
        assert_eq!(dl.resolved_target, path);
        let entry = registry.get(&path).unwrap();
        assert!(
            matches!(entry.status, CellStatus::NotYetSpawned { .. }),
            "status must stay parked — no false Awake"
        );
        log_rx.close(); // no log-row assertion — mirror of the inactive-check early return
    }

    /// Phase-13-J-1: empty receiver → status parked to `Asleep`, `WakeFn` is
    /// NOT invoked. Deterministic race-coverage on the empty side.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_sleep_empty_receiver_parks_to_asleep() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/probe");
        let (sender, receiver) = mpsc::channel::<Message>(8);
        let wake_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wc = wake_called.clone();
        let entry = RegistryEntry {
            handle: meclaw_core::ActorHandle::new(path.clone(), sender),
            respawn: Box::new(|| unreachable!()),
            wake: Some(Box::new(move |_| {
                wc.store(true, std::sync::atomic::Ordering::SeqCst);
                let (s, _r) = tokio::sync::oneshot::channel::<()>();
                let (_s2, r2) = tokio::sync::oneshot::channel::<()>();
                (s, r2)
            })),
            restart_count: 0,
            restart_limit: 5,
            cell_id: uuid::Uuid::now_v7(),
            cell_type: "stub".into(),
            status: CellStatus::Awake,
            eager_on_reconnect: true,
            active: true,
            failed: false,
            stop_tx: None,
            death_ack_rx: None,
        };
        registry.insert(path.clone(), entry);
        let mut dead_letters = std::collections::VecDeque::new();
        handle_sleep(
            &mut registry,
            &mut dead_letters,
            &inbox_tx,
            path.clone(),
            receiver,
        )
        .await;
        let e = registry.get(&path).unwrap();
        assert!(matches!(e.status, CellStatus::Asleep { .. }));
        assert!(!wake_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Phase-13-J-1: pre-filled receiver → `WakeFn` is invoked synchronously,
    /// status stays `Awake`. Deterministic race-coverage on the non-empty side
    /// (sender lives, `try_send` fills the buffer before `handle_sleep` runs).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_sleep_non_empty_receiver_invokes_wake_fn() {
        let mut registry = std::collections::HashMap::<Path, RegistryEntry>::new();
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/probe");
        let (sender, receiver) = mpsc::channel::<Message>(8);
        // Pre-fill receiver synchronously (sender still alive — `try_send`
        // fills the buffer without awaiting).
        sender
            .try_send(MessageBuilder::new(path.clone()).build())
            .expect("buffer has space");
        assert!(!receiver.is_empty());
        let wake_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wc = wake_called.clone();
        let entry = RegistryEntry {
            handle: meclaw_core::ActorHandle::new(path.clone(), sender),
            respawn: Box::new(|| unreachable!()),
            wake: Some(Box::new(move |_| {
                wc.store(true, std::sync::atomic::Ordering::SeqCst);
                let (s, _r) = tokio::sync::oneshot::channel::<()>();
                let (_s2, r2) = tokio::sync::oneshot::channel::<()>();
                (s, r2)
            })),
            restart_count: 0,
            restart_limit: 5,
            cell_id: uuid::Uuid::now_v7(),
            cell_type: "stub".into(),
            status: CellStatus::Awake,
            eager_on_reconnect: true,
            active: true,
            failed: false,
            stop_tx: None,
            death_ack_rx: None,
        };
        registry.insert(path.clone(), entry);
        let mut dead_letters = std::collections::VecDeque::new();
        handle_sleep(
            &mut registry,
            &mut dead_letters,
            &inbox_tx,
            path.clone(),
            receiver,
        )
        .await;
        let e = registry.get(&path).unwrap();
        assert!(matches!(e.status, CellStatus::Awake));
        assert!(wake_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_shutdown_completes() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: ack_tx })
            .await
            .unwrap();
        ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_registers_cell_under_path() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Spawn an Echo cell externally, then Register it with Colony.
        let (cell_in_tx, cell_in_rx) = mpsc::channel(8);
        let cell = EchoMockCell::new(Path::new("/a"));
        let cell_join = tokio::spawn(cell_task(
            Path::new("/a"),
            cell_in_rx,
            out_tx.clone(),
            cell,
            None,
            None,
        ));
        let respawn: RespawnFn = Box::new(|| {
            // not exercised by this test — Task 15+ tests restart
            unreachable!("respawn not invoked in register-only test");
        });

        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a"),
                sender: cell_in_tx,
                join: cell_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // Shutdown
        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_routes_external_message_to_registered_cell() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let (tap_tx, mut tap_rx) = mpsc::channel(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (cell_in_tx, cell_in_rx) = mpsc::channel(8);
        let cell = EchoMockCell::new(Path::new("/a")).tap_to(tap_tx);
        let cell_join = tokio::spawn(cell_task(
            Path::new("/a"),
            cell_in_rx,
            out_tx,
            cell,
            None,
            None,
        ));
        let respawn: RespawnFn = Box::new(|| unreachable!());

        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a"),
                sender: cell_in_tx,
                join: cell_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/a")).build(),
            })
            .await
            .unwrap();

        let tapped = tap_rx.recv().await.expect("tap signal from /a");
        assert_eq!(tapped.as_str(), "/a");

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_logs_cell_death_via_watcher() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let (tap_tx, mut tap_rx) = mpsc::channel(8);
        let calls = Arc::new(AtomicU32::new(0));
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (cell_in_tx, cell_in_rx) = mpsc::channel(8);
        let cell = FailOnDemandMockCell::new(Path::new("/f"), 1, calls.clone()).tap_to(tap_tx);
        let cell_join = tokio::spawn(cell_task(
            Path::new("/f"),
            cell_in_rx,
            out_tx.clone(),
            cell,
            None,
            None,
        ));
        // After Task 15, panics trigger actual restarts; provide a real respawn that
        // spawns a quiet EchoMockCell so the colony doesn't crash on restart.
        let respawn_out = out_tx.clone();
        let respawn: RespawnFn = Box::new(move || {
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
            // Backstop pair (P3-B-restart): sender dropped → never fired → the
            // restarted cell's death classifies Normal/Panic (this test exercises
            // panic-restart, not backstop).
            let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
            let replacement = EchoMockCell::new(Path::new("/f"));
            let outputs_inner = respawn_out.clone();
            let j = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                cell_task(Path::new("/f"), rx, outputs_inner, replacement, None, None).await;
            });
            (tx, j, peace_rx, backstop_rx)
        });

        let (_peace_tx_initial, peace_rx_initial) = tokio::sync::oneshot::channel::<()>();
        drop(_peace_tx_initial); // Drop now → watcher will emit CellDied on cell panic.
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/f"),
                sender: cell_in_tx,
                join: cell_join,
                peace_rx: peace_rx_initial,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/f")).build(),
            })
            .await
            .unwrap();

        // Cell taps own_path then panics. Watcher must observe death and
        // emit a CellDied event into colony's inbox — colony logs + removes.
        let _ = tap_rx.recv().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_restarts_panicking_cell_with_fresh_mpsc_pair() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
        let (tap_tx, mut tap_rx) = mpsc::channel(64);
        let calls = Arc::new(AtomicU32::new(0));
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Build a respawn closure that creates a fresh mpsc pair + cell instance
        // each invocation. Each new instance shares `calls` for global observation.
        let factory_calls = calls.clone();
        let factory_tap = tap_tx.clone();
        let factory_out = out_tx.clone();
        type SpawnFactory = Arc<
            dyn Fn() -> (
                    mpsc::Sender<Message>,
                    tokio::task::JoinHandle<()>,
                    tokio::sync::oneshot::Receiver<()>,
                    tokio::sync::oneshot::Receiver<()>,
                ) + Send
                + Sync,
        >;
        let factory: SpawnFactory = Arc::new(move || {
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
            // Backstop pair (P3-B-restart): sender dropped → never fired.
            let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
            let cell = FailOnDemandMockCell::new(Path::new("/f"), 1, factory_calls.clone())
                .tap_to(factory_tap.clone());
            let outputs_inner = factory_out.clone();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                cell_task(Path::new("/f"), rx, outputs_inner, cell, None, None).await;
            });
            (tx, join, peace_rx, backstop_rx)
        });

        let (sender, cell_join, peace_rx, _backstop_rx_initial) = (factory)();
        let respawn_arc = factory.clone();
        let respawn: RespawnFn = Box::new(move || (respawn_arc)());

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/f"),
                sender,
                join: cell_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // First route → cell panics (local panic_at_call=1)
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/f")).build(),
            })
            .await
            .unwrap();
        let p1 = tap_rx.recv().await.unwrap();
        assert_eq!(p1.as_str(), "/f");

        // Allow watcher → death event → restart
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Second route → fresh cell instance (local counter reset to 1) panics again
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/f")).build(),
            })
            .await
            .unwrap();
        let p2 = tap_rx.recv().await.unwrap();
        assert_eq!(p2.as_str(), "/f");

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();

        // Global counter: 2 handle() calls observed across 2 cell instances.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_routes_output_envelope_to_target_cell() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let (tap_a_tx, _tap_a_rx) = mpsc::channel(8);
        let (tap_b_tx, mut tap_b_rx) = mpsc::channel(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (a_in_tx, a_in_rx) = mpsc::channel(8);
        let a = EchoMockCell::new(Path::new("/a"))
            .echo_to(Path::new("/b"))
            .tap_to(tap_a_tx);
        let a_join = tokio::spawn(cell_task(
            Path::new("/a"),
            a_in_rx,
            out_tx.clone(),
            a,
            None,
            None,
        ));

        let (b_in_tx, b_in_rx) = mpsc::channel(8);
        let b = EchoMockCell::new(Path::new("/b")).tap_to(tap_b_tx);
        let b_join = tokio::spawn(cell_task(Path::new("/b"), b_in_rx, out_tx, b, None, None));

        let respawn_a: RespawnFn = Box::new(|| unreachable!());
        let respawn_b: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx_a, peace_rx_a) = oneshot::channel::<()>();
        let (_peace_tx_b, peace_rx_b) = oneshot::channel::<()>();

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a"),
                sender: a_in_tx,
                join: a_join,
                peace_rx: peace_rx_a,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn: respawn_a,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/b"),
                sender: b_in_tx,
                join: b_join,
                peace_rx: peace_rx_b,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn: respawn_b,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // W2 (A1): /a→/b now needs a wired edge (identity-fallback gone).
        let (e_ack_tx, e_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from: Path::new("/a"),
                to: Path::new("/b"),
                ack: e_ack_tx,
            })
            .await
            .unwrap();
        e_ack_rx.await.unwrap();

        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/a")).build(),
            })
            .await
            .unwrap();

        let tapped_b = tap_b_rx
            .recv()
            .await
            .expect("/b must have been routed to via output envelope");
        assert_eq!(tapped_b.as_str(), "/b");

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_drain_dead_letters_returns_empty_when_no_failures() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert!(
            drained.is_empty(),
            "no dead letters expected on fresh colony"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_route_unresolved_target_lands_in_dead_letters() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // No cells registered. Route to /missing → dead letter.
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/missing")).build(),
            })
            .await
            .unwrap();

        // Drain and assert.
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].resolved_target.as_str(), "/missing");
        assert_eq!(drained[0].original_target.as_str(), "/missing");
        assert_eq!(drained[0].sender_path.as_str(), "/");
        assert_eq!(
            drained[0].reason,
            crate::dead_letter::DeadLetterReason::UnresolvedPath
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// Phase-13.5-A6-T3: `/colony/templates` and `/colony/dead_letters` are now
    /// **implemented** — they no longer produce a DLQ push but a reply (resp.
    /// `RouteAction::Done` when `reply_to=None`). The test pins the remaining
    /// DLQ behaviour:
    /// - `/colony` bare → `ColonyEndpointInvalid` (in `route()` itself, not in
    ///   the dispatcher — full-body fixture pinned).
    /// - `/colonial/x` → `UnresolvedPath` (boundary check: a NON-`/colony` prefix
    ///   must run through the normal registry-lookup cascade).
    /// - Unknown `/colony/<x>` → `ColonyEndpointUnimplemented` with sender
    ///   pass-through (additional test in `colony_dispatch::tests`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_route_bare_and_non_colony_prefix_land_in_dead_letters() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // /colony bare → ColonyEndpointInvalid (handled in route() itself,
        // see Vollbody-Fixture plans/phase-13.5-hive-transit-fixtures/expected_route_body.txt).
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/colony")).build(),
            })
            .await
            .unwrap();

        // Boundary case: /colonial is NOT a colony endpoint — must fall through
        // to the regular registry lookup. With no cell at /colonial/x it dead-letters
        // with UnresolvedPath (not ColonyEndpoint*). This catches the plain-prefix
        // bug where `starts_with("/colony")` would wrongly trap /colonial.
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/colonial/x")).build(),
            })
            .await
            .unwrap();

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert_eq!(drained.len(), 2);

        use crate::dead_letter::DeadLetterReason::*;
        let by_target: std::collections::HashMap<_, _> = drained
            .iter()
            .map(|d| (d.resolved_target.as_str().to_string(), d.reason.clone()))
            .collect();
        assert_eq!(by_target.get("/colony"), Some(&ColonyEndpointInvalid));
        assert_eq!(
            by_target.get("/colonial/x"),
            Some(&UnresolvedPath),
            "/colonial must NOT be confused with /colony — boundary check"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_outputs_arm_resolves_relative_target_via_central_route() {
        use crate::cell_task::cell_task;
        use meclaw_core::MessageBuilder;
        use meclaw_testing::mocks::EchoMockCell;

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let (tap_tx, mut tap_rx) = mpsc::channel(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Register /a/b/c as terminal echo (just taps).
        let (c_in_tx, c_in_rx) = mpsc::channel(8);
        let c = EchoMockCell::new(Path::new("/a/b/c")).tap_to(tap_tx);
        let c_join = tokio::spawn(cell_task(
            Path::new("/a/b/c"),
            c_in_rx,
            out_tx.clone(),
            c,
            None,
            None,
        ));
        let respawn_c: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx_c, peace_rx_c) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a/b/c"),
                sender: c_in_tx,
                join: c_join,
                peace_rx: peace_rx_c,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn: respawn_c,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // Register /a/b/d as forwarder (echo_to ../c).
        let (d_in_tx, d_in_rx) = mpsc::channel(8);
        let d = EchoMockCell::new(Path::new("/a/b/d")).echo_to(Path::new("../c"));
        let d_join = tokio::spawn(cell_task(
            Path::new("/a/b/d"),
            d_in_rx,
            out_tx.clone(),
            d,
            None,
            None,
        ));
        let respawn_d: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx_d, peace_rx_d) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a/b/d"),
                sender: d_in_tx,
                join: d_join,
                peace_rx: peace_rx_d,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn: respawn_d,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // W2 (A1): /a/b/d → ../c now needs a wired edge (identity-fallback gone).
        // The edge target stays the RELATIVE `../c` so route() still exercises
        // relative-target resolution (the point of this test).
        let (e_ack_tx, e_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from: Path::new("/a/b/d"),
                to: Path::new("../c"),
                ack: e_ack_tx,
            })
            .await
            .unwrap();
        e_ack_rx.await.unwrap();

        // Fire at /a/b/d → forwards to ../c → resolves to /a/b/c.
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/a/b/d")).build(),
            })
            .await
            .unwrap();

        let p = tap_rx
            .recv()
            .await
            .expect("/a/b/c must receive the routed message");
        assert_eq!(p.as_str(), "/a/b/c");

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// TTL slice (2026-06-11): a SOURCE emission (`parent_message_id == None`,
    /// the OriginSink shape of timer/proxy/mcp) gets its fresh TTL from
    /// `colony.json::message_default_ttl` — NOT from the constant seed the
    /// OriginSink carries in `input_ttl`. Envelope-Setter-Authority: Colony
    /// stamps `ttl` anew on source messages. The capture mailbox receives the
    /// routed message with `ttl == 7 - 1` (one `route()` decrement).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn outputs_arm_source_emission_gets_colony_config_ttl() {
        use meclaw_core::Headers;
        use meclaw_core::serde_json::json;

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let cfg = crate::ColonyConfig {
            message_default_ttl: 7,
            ..crate::ColonyConfig::default()
        };
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            cfg,
            None,
            None,
        )));

        // Raw capture mailbox at /sink (never-ending join task, no cell logic).
        let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
        let sink_join = tokio::spawn(std::future::pending::<()>());
        register_simple_cell(&inbox_tx, Path::new("/sink"), sink_tx, sink_join).await;

        // W2 (A1): /timer→/sink now needs a wired edge (identity-fallback gone).
        let (e_ack_tx, e_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from: Path::new("/timer"),
                to: Path::new("/sink"),
                ack: e_ack_tx,
            })
            .await
            .unwrap();
        e_ack_rx.await.unwrap();

        // Source emission exactly as OriginSink mints it: parent None, the
        // constant seed (64) in input_ttl.
        out_tx
            .send(CellEmission {
                sender_path: Path::new("/timer"),
                parent_message_id: None,
                trace_id: Uuid::now_v7(),
                input_ttl: meclaw_core::MESSAGE_DEFAULT_TTL,
                input_reply_to: None,
                input_headers: Headers::new(),
                target: Path::new("/sink"),
                content: json!({"messages":[{"origin":"user","type":"text","text":"tick"}]}),
                direct_reply: false,
            })
            .await
            .unwrap();

        let msg = sink_rx
            .recv()
            .await
            .expect("/sink must receive the routed source message");
        assert_eq!(
            msg.ttl, 6,
            "source emission must start with colony.json message_default_ttl (7), \
             decremented once by route()"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// Guard for the TTL-slice override scope: a FOLLOW-UP emission
    /// (`parent_message_id == Some`) keeps inheriting the consumed input's TTL
    /// verbatim — the colony.json default must NOT clobber the per-hop budget.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn outputs_arm_follow_up_emission_inherits_input_ttl() {
        use meclaw_core::Headers;
        use meclaw_core::serde_json::json;

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let cfg = crate::ColonyConfig {
            message_default_ttl: 7,
            ..crate::ColonyConfig::default()
        };
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            cfg,
            None,
            None,
        )));

        let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
        let sink_join = tokio::spawn(std::future::pending::<()>());
        register_simple_cell(&inbox_tx, Path::new("/sink"), sink_tx, sink_join).await;

        // W2 (A1): /worker→/sink now needs a wired edge (identity-fallback gone).
        let (e_ack_tx, e_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from: Path::new("/worker"),
                to: Path::new("/sink"),
                ack: e_ack_tx,
            })
            .await
            .unwrap();
        e_ack_rx.await.unwrap();

        out_tx
            .send(CellEmission {
                sender_path: Path::new("/worker"),
                parent_message_id: Some(Uuid::now_v7()),
                trace_id: Uuid::now_v7(),
                input_ttl: 20,
                input_reply_to: None,
                input_headers: Headers::new(),
                target: Path::new("/sink"),
                content: json!({"messages":[{"origin":"user","type":"text","text":"step"}]}),
                direct_reply: false,
            })
            .await
            .unwrap();

        let msg = sink_rx
            .recv()
            .await
            .expect("/sink must receive the routed follow-up message");
        assert_eq!(
            msg.ttl, 19,
            "follow-up emission must inherit input_ttl (20) minus one route() decrement \
             — colony.json default must not apply"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// Phase-13.5-A6-T2: direct `route()`-call returns `RouteAction::Done`
    /// on TTL-expired (no cascade, no colony-dispatch). Failing-test-first
    /// for the route()-return-type-break.
    #[tokio::test]
    async fn route_direct_ttl_zero_returns_done() {
        use meclaw_core::{MessageBuilder, Path};
        let registry: HashMap<Path, RegistryEntry> = HashMap::new();
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let msg = MessageBuilder::new(Path::new("/anywhere")).ttl(0).build();
        let action = route(
            &registry,
            &HiveScopeTable::new(),
            &mut dead_letters,
            Path::new("/"),
            msg,
        )
        .await;
        assert!(matches!(action, RouteAction::Done));
        assert_eq!(dead_letters.len(), 1);
        assert_eq!(
            dead_letters[0].reason,
            crate::dead_letter::DeadLetterReason::TtlExpired
        );
    }

    /// Phase-13.5-A6-T2: direct `route()`-call returns `RouteAction::ColonyDispatch`
    /// on `/colony/<endpoint>`-target (deferred to state-rich callsite).
    #[tokio::test]
    async fn route_direct_colony_endpoint_returns_colony_dispatch() {
        use meclaw_core::{MessageBuilder, Path};
        let registry: HashMap<Path, RegistryEntry> = HashMap::new();
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let msg = MessageBuilder::new(Path::new("/colony/dead_letters"))
            .ttl(8)
            .build();
        let action = route(
            &registry,
            &HiveScopeTable::new(),
            &mut dead_letters,
            Path::new("/sender"),
            msg,
        )
        .await;
        match action {
            RouteAction::ColonyDispatch {
                endpoint, sender, ..
            } => {
                assert_eq!(endpoint.as_str(), "/colony/dead_letters");
                assert_eq!(sender.as_str(), "/sender");
            }
            other => panic!(
                "expected ColonyDispatch, got {:?}",
                match other {
                    RouteAction::Done => "Done",
                    RouteAction::Cascade { .. } => "Cascade",
                    RouteAction::HiveTransit { .. } => "HiveTransit",
                    RouteAction::ColonyDispatch { .. } => unreachable!(),
                }
            ),
        }
        // ColonyDispatch is non-terminal in route() — no DLQ-push happens
        // inside route() itself; the callsite invokes the T2-Stub.
        assert_eq!(dead_letters.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn route_with_ttl_zero_lands_in_dead_letters_as_ttl_expired() {
        use meclaw_core::{MessageBuilder, Path};

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Source message with ttl=0 — must hit ttl_expired before any lookup.
        let msg = MessageBuilder::new(Path::new("/anywhere")).ttl(0).build();
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .unwrap();

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(
            drained[0].reason,
            crate::dead_letter::DeadLetterReason::TtlExpired
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_target_with_reply_to_set_replies_to_sender() {
        use meclaw_core::{Cell, CellEmission, MessageBuilder, OutputSink, Path};
        use std::future::Future;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Receiver-Cell at /listener captures the first Message it sees.
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Option<meclaw_core::Message>>>);
        impl Cell for Capture {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                msg: meclaw_core::Message,
                _sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let slot = self.0.clone();
                async move {
                    *slot.lock().await = Some(msg);
                }
            }
        }

        let store: Arc<Mutex<Option<meclaw_core::Message>>> = Arc::new(Mutex::new(None));

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Register a Capture-Cell at /listener.
        let (lst_in_tx, lst_in_rx) = mpsc::channel(8);
        let cap = Capture(store.clone());
        let lst_join = tokio::spawn(cell_task(
            Path::new("/listener"),
            lst_in_rx,
            out_tx.clone(),
            cap,
            None,
            None,
        ));
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/listener"),
                sender: lst_in_tx,
                join: lst_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // Route a message to /missing with reply_to = /listener.
        let msg = MessageBuilder::new(Path::new("/missing"))
            .reply_to(Path::new("/listener"))
            .build();
        let original_id = msg.id;
        let original_trace = msg.trace_id;
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .unwrap();

        // Allow cascade to deliver the error-reply.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let captured = store
            .lock()
            .await
            .clone()
            .expect("error-reply must reach /listener");
        assert_eq!(captured.target.as_str(), "/listener");
        assert_eq!(
            captured.reply_to, None,
            "error-reply is terminal (reply_to=None)"
        );
        assert_eq!(captured.trace_id, original_trace, "trace_id propagates");
        assert_eq!(
            captured.parent_message_id,
            Some(original_id),
            "parent = original message id"
        );

        // Dead-letter queue stays empty — reply succeeded.
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert!(
            drained.is_empty(),
            "reply_to-Reply must NOT also dead-letter"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// Slice 2 — outputs-arm two-compartment decay: an incoming message with
    /// `context.turn_id="t1"` and `hop.operation="select"` is consumed by a cell
    /// that emits `{"header":{"finish_reason":"tool_calls"}, ...}`.
    /// At the follow-up: `hop.operation` has decayed (the old hop is dropped),
    /// `hop.finish_reason` is the fresh cell output, `context.turn_id` travels
    /// through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn outputs_arm_two_compartment_verfall_drops_hop_carries_context() {
        use meclaw_core::serde_json::json;
        use meclaw_core::{Cell, CellEmission, Headers, MessageBuilder, OutputSink, Path};
        use meclaw_testing::mocks::EchoMockCell;
        use std::future::Future;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Option<meclaw_core::Message>>>);
        impl Cell for Capture {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                msg: meclaw_core::Message,
                _sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let slot = self.0.clone();
                async move {
                    *slot.lock().await = Some(msg);
                }
            }
        }

        let store: Arc<Mutex<Option<meclaw_core::Message>>> = Arc::new(Mutex::new(None));

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // /src: echo-cell that emits a content.header finish_reason=tool_calls
        // and forwards to /listener.
        let (src_in_tx, src_in_rx) = mpsc::channel(8);
        let src_cell = EchoMockCell::new(Path::new("/src"))
            .echo_to(Path::new("/listener"))
            .with_emitted_header("finish_reason", json!("tool_calls"));
        let src_join = tokio::spawn(cell_task(
            Path::new("/src"),
            src_in_rx,
            out_tx.clone(),
            src_cell,
            None,
            None,
        ));
        register_simple_cell(&inbox_tx, Path::new("/src"), src_in_tx, src_join).await;

        // /listener: capture cell.
        let (lst_in_tx, lst_in_rx) = mpsc::channel(8);
        let cap = Capture(store.clone());
        let lst_join = tokio::spawn(cell_task(
            Path::new("/listener"),
            lst_in_rx,
            out_tx.clone(),
            cap,
            None,
            None,
        ));
        register_simple_cell(&inbox_tx, Path::new("/listener"), lst_in_tx, lst_join).await;

        // W2 (Ruling A1): /src→/listener now needs a wired edge — the implicit
        // identity-fallback is gone, an unrouted emission would `no_route`-DLQ.
        let (e_ack_tx, e_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: meclaw_core::Uuid::now_v7(),
                from: Path::new("/src"),
                to: Path::new("/listener"),
                ack: e_ack_tx,
            })
            .await
            .unwrap();
        e_ack_rx.await.unwrap();

        // Route to /src with context.turn_id="t1" + hop.operation="select".
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("turn_id".into(), json!("t1"));
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("operation".into(), json!("select"));
        let msg = MessageBuilder::new(Path::new("/src"))
            .headers(Headers::from_parts(ctx, hop))
            .body(meclaw_core::Body::Inline(json!({"messages": []})))
            .build();
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let captured = store
            .lock()
            .await
            .clone()
            .expect("follow-up must reach /listener");
        // "two-compartment decay" is the two-compartment header model's decay rule
        // (tag: header-model-zwei-faecher-done).
        assert!(
            captured.headers.hop.get("operation").is_none(),
            "old input hop (operation) dropped (two-compartment decay)"
        );
        assert_eq!(
            captured.headers.hop.get("finish_reason"),
            Some(&json!("tool_calls")),
            "cell output is the new hop"
        );
        assert_eq!(
            captured.headers.context.get("turn_id"),
            Some(&json!("t1")),
            "context travels through the hop"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    /// Slice 2 — fan-out context-copy pin: a cell with two matching out-edges
    /// produces two follow-ups to two sinks; each edge sets a different
    /// `hop.route`. The persistent `context` MUST be copied byte-identically
    /// into both branches, while `hop` differs per branch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fan_out_copies_context_byte_identical_while_hop_diverges() {
        use meclaw_core::serde_json::json;
        use meclaw_core::{Cell, CellEmission, Headers, MessageBuilder, OutputSink, Path};
        use meclaw_testing::mocks::EchoMockCell;
        use std::future::Future;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Option<meclaw_core::Message>>>);
        impl Cell for Capture {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                msg: meclaw_core::Message,
                _sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let slot = self.0.clone();
                async move {
                    *slot.lock().await = Some(msg);
                }
            }
        }

        let store_a: Arc<Mutex<Option<meclaw_core::Message>>> = Arc::new(Mutex::new(None));
        let store_b: Arc<Mutex<Option<meclaw_core::Message>>> = Arc::new(Mutex::new(None));

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // /src emits a content (target is overridden by the fan-out edges).
        let (src_in_tx, src_in_rx) = mpsc::channel(8);
        let src_cell = EchoMockCell::new(Path::new("/src")).echo_to(Path::new("/sink_a"));
        let src_join = tokio::spawn(cell_task(
            Path::new("/src"),
            src_in_rx,
            out_tx.clone(),
            src_cell,
            None,
            None,
        ));
        register_simple_cell(&inbox_tx, Path::new("/src"), src_in_tx, src_join).await;

        // Two capture sinks.
        let (a_in_tx, a_in_rx) = mpsc::channel(8);
        let a_join = tokio::spawn(cell_task(
            Path::new("/sink_a"),
            a_in_rx,
            out_tx.clone(),
            Capture(store_a.clone()),
            None,
            None,
        ));
        register_simple_cell(&inbox_tx, Path::new("/sink_a"), a_in_tx, a_join).await;

        let (b_in_tx, b_in_rx) = mpsc::channel(8);
        let b_join = tokio::spawn(cell_task(
            Path::new("/sink_b"),
            b_in_rx,
            out_tx.clone(),
            Capture(store_b.clone()),
            None,
            None,
        ));
        register_simple_cell(&inbox_tx, Path::new("/sink_b"), b_in_tx, b_join).await;

        // Two out-edges from /src, each setting a different hop.route.
        let mut spec_a = crate::config::ModifierSpec::default();
        spec_a.set_hop.insert("route".into(), "'left'".into());
        let mut spec_b = crate::config::ModifierSpec::default();
        spec_b.set_hop.insert("route".into(), "'right'".into());
        let edge_a = crate::bootstrap::PlannedEdge {
            id: Uuid::now_v7(),
            from: Path::new("/src"),
            to: Path::new("/sink_a"),
            condition: None,
            modifier: Some(crate::cel_eval::parse_modifier(&spec_a).unwrap()),
        };
        let edge_b = crate::bootstrap::PlannedEdge {
            id: Uuid::now_v7(),
            from: Path::new("/src"),
            to: Path::new("/sink_b"),
            condition: None,
            modifier: Some(crate::cel_eval::parse_modifier(&spec_b).unwrap()),
        };
        let (ia_ack_tx, ia_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::InitialApply {
                edges: vec![edge_a, edge_b],
                hive_scopes: vec![],
                ack: ia_ack_tx,
            })
            .await
            .unwrap();
        ia_ack_rx.await.unwrap();

        // Route to /src with a persistent context.turn_id="t1".
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("turn_id".into(), json!("t1"));
        let msg = MessageBuilder::new(Path::new("/src"))
            .headers(Headers::from_parts(
                ctx,
                meclaw_core::serde_json::Map::new(),
            ))
            .body(meclaw_core::Body::Inline(json!({"messages": []})))
            .build();
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;

        let a = store_a
            .lock()
            .await
            .clone()
            .expect("fan-out branch must reach /sink_a");
        let b = store_b
            .lock()
            .await
            .clone()
            .expect("fan-out branch must reach /sink_b");

        assert_eq!(
            a.headers.context, b.headers.context,
            "context byte-identical across fan-out branches"
        );
        assert_eq!(
            a.headers.context.get("turn_id"),
            Some(&json!("t1")),
            "context.turn_id survives into both branches"
        );
        assert_ne!(
            a.headers.hop.get("route"),
            b.headers.hop.get("route"),
            "per-branch hop.route diverges"
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_target_with_unreachable_reply_to_falls_back_to_dead_letter() {
        use meclaw_core::{MessageBuilder, Path};

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Message to /missing with reply_to = /also_missing → original target unresolved,
        // error-reply target ALSO unresolved → error-reply itself has reply_to=None
        // (terminal), so it dead-letters with UnresolvedPath. No infinite cascade.
        let msg = MessageBuilder::new(Path::new("/missing"))
            .reply_to(Path::new("/also_missing"))
            .build();
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg,
            })
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        // One DLQ entry expected: the error-reply to /also_missing fails with UnresolvedPath.
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].resolved_target.as_str(), "/also_missing");
        assert_eq!(
            drained[0].reason,
            crate::dead_letter::DeadLetterReason::UnresolvedPath
        );

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[test]
    fn build_follow_up_message_carries_context_and_replaces_hop_with_cell_output() {
        use meclaw_core::serde_json::json;
        use meclaw_core::{Body, CellEmission, Headers, Path, Uuid};

        // context.session_id travels through; input.hop.operation decays; the
        // cell output (content.header) becomes the new hop.
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("session_id".into(), json!("s1"));
        let mut old_hop = meclaw_core::serde_json::Map::new();
        old_hop.insert("operation".into(), json!("select"));
        let input_headers = Headers::from_parts(ctx, old_hop);

        let em = CellEmission {
            input_reply_to: None,
            sender_path: Path::new("/a"),
            parent_message_id: Some(Uuid::now_v7()),
            trace_id: Uuid::now_v7(),
            input_ttl: 10,
            input_headers,
            target: Path::new("/b"),
            content: json!({
                "header": {"forwarded_by": "/a", "finish_reason": "stop"},
                "messages": [{"origin": "assistant", "type": "text", "text": "hi"}]
            }),
            direct_reply: false,
        };

        let msg = build_follow_up_message(em);
        assert_eq!(
            msg.headers.context.get("session_id"),
            Some(&json!("s1")),
            "context survives the hop"
        );
        assert!(
            msg.headers.hop.get("operation").is_none(),
            "old input hop dropped (two-compartment decay)"
        );
        assert_eq!(
            msg.headers.hop.get("forwarded_by"),
            Some(&json!("/a")),
            "cell output becomes the new hop"
        );
        assert_eq!(
            msg.headers.hop.get("finish_reason"),
            Some(&json!("stop")),
            "cell output becomes the new hop"
        );

        let body_value = match msg.body {
            Body::Inline(v) => v,
            _ => panic!("expected Inline body"),
        };
        assert!(
            body_value.get("header").is_none(),
            "content.header must be removed from body"
        );
        assert_eq!(body_value["messages"][0]["text"], json!("hi"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn registry_entry_default_restart_limit_is_5() {
        use crate::ColonyDb;
        let _td = tempfile::TempDir::new().unwrap();
        let db = ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel(8);
        let self_tx = inbox_tx.clone();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            self_tx,
            inbox_rx,
            out_tx.clone(),
            out_rx,
            db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (cell_in_tx, cell_in_rx) = mpsc::channel(8);
        let cell = EchoMockCell::new(meclaw_core::Path::new("/a"));
        let cell_join = tokio::spawn(cell_task(
            meclaw_core::Path::new("/a"),
            cell_in_rx,
            out_tx.clone(),
            cell,
            None,
            None,
        ));
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/a"),
                sender: cell_in_tx,
                join: cell_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None, // None → Default 5
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_msg_add_edge_acks() {
        // Verifies that ColonyMsg::AddEdge reaches the inbox and the handler
        // acks. The edge_table itself is task-local, so we test indirectly:
        // send AddEdge with oneshot, wait for ack, then Shutdown cleanly.
        use meclaw_core::Uuid;
        use tokio::sync::{mpsc, oneshot};

        let (inbox_tx, inbox_rx) = mpsc::channel(64);
        let (outputs_tx, outputs_rx) = mpsc::channel(64);
        let self_tx = inbox_tx.clone();
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            self_tx,
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddEdge {
                id: Uuid::now_v7(),
                from: Path::new("/a"),
                to: Path::new("/b"),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        let (sd_tx, sd_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: sd_tx })
            .await
            .unwrap();
        sd_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_msg_add_hive_scope_acks() {
        use tokio::sync::{mpsc, oneshot};

        let (inbox_tx, inbox_rx) = mpsc::channel(64);
        let (outputs_tx, outputs_rx) = mpsc::channel(64);
        let self_tx = inbox_tx.clone();
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            self_tx,
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::AddHiveScope {
                path: Path::new("/"),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        let (sd_tx, sd_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: sd_tx })
            .await
            .unwrap();
        sd_rx.await.unwrap();
        join.await.unwrap();
    }

    #[cfg(debug_assertions)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_emitting_invalid_ubf_body_lands_in_dead_letters() {
        use meclaw_core::serde_json::json;
        use meclaw_core::{Cell, CellEmission, CellOutput, MessageBuilder, OutputSink, Path};
        use std::future::Future;

        struct BadEmitter;
        impl Cell for BadEmitter {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                _msg: meclaw_core::Message,
                sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let sink = sink.clone();
                async move {
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/anywhere"),
                            content: json!({"messages": [{"origin": "WRONG_ORIGIN", "type": "text"}]}),
                        })
                        .await;
                }
            }
        }

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let _td = tempfile::TempDir::new().unwrap();
        let _db = crate::ColonyDb::open(&_td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            _db,
            crate::CellFactoryRegistry::new(),
            _td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (bad_in_tx, bad_in_rx) = mpsc::channel(8);
        let bad_join = tokio::spawn(cell_task(
            Path::new("/bad"),
            bad_in_rx,
            out_tx.clone(),
            BadEmitter,
            None,
            None,
        ));
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx, peace_rx) = oneshot::channel::<()>();
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/bad"),
                sender: bad_in_tx,
                join: bad_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/bad")).build(),
            })
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .unwrap();
        let drained = ack_rx.await.unwrap();
        assert_eq!(drained.len(), 1, "exactly one InvalidUbfBody dead-letter");
        assert_eq!(
            drained[0].reason,
            crate::dead_letter::DeadLetterReason::InvalidUbfBody
        );
        assert_eq!(drained[0].sender_path.as_str(), "/bad");

        let (s_ack_tx, s_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_ack_tx })
            .await
            .unwrap();
        s_ack_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rescan_templates_message_triggers_apply_scan_result() {
        let td = tempfile::TempDir::new().unwrap();
        let templates_root = td.path().join("templates");
        std::fs::create_dir_all(templates_root.join("new_template")).unwrap();
        std::fs::write(
            templates_root.join("new_template/template.json"),
            r#"{"name":"new_template"}"#,
        )
        .unwrap();

        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            db,
            crate::CellFactoryRegistry::new(),
            td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::RescanTemplates {
                templates_root: templates_root.clone(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();

        // Verifikation via fresh ColonyDb read.
        let probe = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let rows = probe.read_templates().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "new_template");

        // Shutdown.
        let (s_tx, s_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_tx })
            .await
            .unwrap();
        s_rx.await.unwrap();
        join.await.unwrap();
    }

    #[test]
    fn cell_status_awake_is_unit_variant() {
        assert!(matches!(CellStatus::Awake, CellStatus::Awake));
    }

    #[test]
    fn registry_entry_has_status_field() {
        let _: fn(&RegistryEntry) -> &CellStatus = |e| &e.status;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn read_registry_reports_awake_after_register() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
        let td = tempfile::TempDir::new().unwrap();
        let db = crate::ColonyDb::open(&td.path().join("c.db")).unwrap();
        let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            out_tx.clone(),
            out_rx,
            db,
            crate::CellFactoryRegistry::new(),
            td.path().to_path_buf(),
            crate::ColonyConfig::default(),
            None,
            None,
        )));

        // Spawn + Register a stub cell.
        let (cell_in_tx, _cell_in_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let cell_join = tokio::spawn(async { std::future::pending::<()>().await });
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let (_peace_tx, peace_rx) = oneshot::channel::<()>();

        let (reg_ack_tx, reg_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Register {
                path: Path::new("/probe"),
                sender: cell_in_tx,
                join: cell_join,
                peace_rx,
                backstop_rx: dropped_backstop_rx(),
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake: None,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "stub".into(),
                active: true,
                ack: reg_ack_tx,
            })
            .await
            .unwrap();
        reg_ack_rx.await.unwrap();

        // ReadRegistry-Roundtrip.
        let (rr_ack_tx, rr_ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::ReadRegistry {
                path: None,
                path_prefix: None,
                cell_type: None,
                active: None,
                limit: 100,
                ack: rr_ack_tx,
            })
            .await
            .unwrap();
        let reply = rr_ack_rx.await.unwrap();
        let e = reply
            .entries
            .into_iter()
            .find(|d| d.path == "/probe")
            .expect("entry");
        assert_eq!(e.lifecycle_status, "Awake");

        // Shutdown.
        let (s_tx, s_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::Shutdown { ack: s_tx })
            .await
            .unwrap();
        s_rx.await.unwrap();
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_register_initializes_status_awake() {
        let mut registry = std::collections::HashMap::<meclaw_core::Path, RegistryEntry>::new();
        let (inbox_tx, _inbox_rx) = tokio::sync::mpsc::channel::<ColonyMsg>(8);
        let (writer_tx, mut writer_rx) =
            tokio::sync::mpsc::channel::<crate::persist::writer::ColonyWriteOp>(8);
        let qd = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
        let (cell_tx, _cell_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(8);
        let (_peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        handle_register(
            &mut registry,
            &inbox_tx,
            &writer_tx,
            &qd,
            meclaw_core::Path::new("/probe"),
            cell_tx,
            tokio::spawn(async {}),
            peace_rx,
            dropped_backstop_rx(),
            None,
            None,
            Box::new(|| unreachable!()),
            None,
            None,
            uuid::Uuid::now_v7(),
            "stub".into(),
            true,
            ack_tx,
        )
        .await;
        let _ = ack_rx.await;
        let _ = writer_rx.recv().await;
        let e = registry.get(&meclaw_core::Path::new("/probe")).unwrap();
        assert!(matches!(e.status, CellStatus::Awake));
        // Phase-13.5 Lifecycle-3b: a fresh spawn is active (spawn = active).
        assert!(e.active, "fresh spawn via handle_register must be active");
    }

    // ---- Phase-13-G-3: handle_register_dormant ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn register_dormant_sets_status_not_yet_spawned_and_no_lifecycle_in_db() {
        let mut registry = std::collections::HashMap::<meclaw_core::Path, RegistryEntry>::new();
        let (writer_tx, mut writer_rx) =
            tokio::sync::mpsc::channel::<crate::persist::writer::ColonyWriteOp>(8);
        let qd = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
        let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(8);
        let respawn: RespawnFn = Box::new(|| unreachable!());
        let wake: Option<crate::WakeFn> = Some(Box::new(|_| {
            let (s, _r) = tokio::sync::oneshot::channel::<()>();
            let (_s2, r2) = tokio::sync::oneshot::channel::<()>();
            (s, r2)
        }));
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        handle_register_dormant(
            &mut registry,
            &writer_tx,
            &qd,
            meclaw_core::Path::new("/probe"),
            sender,
            receiver,
            respawn,
            wake,
            None,
            uuid::Uuid::now_v7(),
            "stub".into(),
            true,
            false,
            false,
            ack_tx,
        )
        .await;
        let _ = ack_rx.await;
        let op = writer_rx.recv().await.unwrap();
        // Persisted UpsertRegistry MUST be identical to handle_register's
        // (no lifecycle_status column).
        assert!(matches!(
            op,
            crate::persist::writer::ColonyWriteOp::UpsertRegistry { .. }
        ));
        let e = registry.get(&meclaw_core::Path::new("/probe")).unwrap();
        assert!(matches!(e.status, CellStatus::NotYetSpawned { .. }));
    }

    // ---- Paket-3 P3-B-restart: handle_cell_died death_kind corridor pin-tests ----

    /// Build a `RegistryEntry` whose `respawn` bumps a shared counter (and
    /// returns a live dead-pair tuple) so a restart is observable.
    fn entry_with_counting_respawn(
        path: Path,
        restart_count: u32,
        counter: Arc<AtomicU32>,
    ) -> RegistryEntry {
        let (sender, _rx) = mpsc::channel::<Message>(8);
        let respawn: RespawnFn = Box::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (s, _r) = mpsc::channel::<Message>(8);
            let (_pt, pr) = tokio::sync::oneshot::channel::<()>();
            let (_bt, br) = tokio::sync::oneshot::channel::<()>();
            let join = tokio::spawn(async {});
            (s, join, pr, br)
        });
        RegistryEntry {
            handle: ActorHandle::new(path.clone(), sender),
            respawn,
            wake: None,
            restart_count,
            restart_limit: 5,
            cell_id: uuid::Uuid::now_v7(),
            cell_type: "stub".into(),
            status: CellStatus::Awake,
            eager_on_reconnect: true,
            active: true,
            failed: false,
            stop_tx: None,
            death_ack_rx: None,
        }
    }

    /// Pin: `DeathKind::Panic` → restart (entry stays, restart_count bumps,
    /// respawn invoked). The was_panic-equivalent restart path is unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_cell_died_panic_restarts() {
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/p");
        let counter = Arc::new(AtomicU32::new(0));
        let mut registry = HashMap::new();
        registry.insert(
            path.clone(),
            entry_with_counting_respawn(path.clone(), 0, counter.clone()),
        );
        assert_eq!(
            handle_cell_died(&mut registry, &inbox_tx, path.clone(), DeathKind::Panic).await,
            CellDiedOutcome::Restarted
        );
        assert!(
            registry.contains_key(&path),
            "panic → cell stays (restarted)"
        );
        assert_eq!(registry.get(&path).unwrap().restart_count, 1);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Pin (THE sanctioned change): `DeathKind::Backstop` → restart (NOT remove).
    /// Before P3-B-restart this death (clean `return`, no panic) was removed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_cell_died_backstop_restarts() {
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/b");
        let counter = Arc::new(AtomicU32::new(0));
        let mut registry = HashMap::new();
        registry.insert(
            path.clone(),
            entry_with_counting_respawn(path.clone(), 0, counter.clone()),
        );
        assert_eq!(
            handle_cell_died(&mut registry, &inbox_tx, path.clone(), DeathKind::Backstop).await,
            CellDiedOutcome::Restarted
        );
        assert!(
            registry.contains_key(&path),
            "backstop → cell stays (restarted), NOT removed"
        );
        assert_eq!(registry.get(&path).unwrap().restart_count, 1);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Pin: `DeathKind::Normal` → remove (clean mailbox-close end).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_cell_died_normal_removes() {
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/n");
        let counter = Arc::new(AtomicU32::new(0));
        let mut registry = HashMap::new();
        registry.insert(
            path.clone(),
            entry_with_counting_respawn(path.clone(), 0, counter.clone()),
        );
        assert_eq!(
            handle_cell_died(&mut registry, &inbox_tx, path.clone(), DeathKind::Normal).await,
            CellDiedOutcome::Removed
        );
        assert!(
            !registry.contains_key(&path),
            "normal → cell removed from registry"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "respawn must NOT be invoked on Normal death"
        );
    }

    /// Pin (THE Paket-6 corridor break): Backstop over the restart_limit no longer
    /// removes — it marks the entry `failed` in-memory and RETAINS it (No-Delete),
    /// returning `CellDiedOutcome::Failed { path }` so the caller can persist `failed`
    /// AFTER the await-free corridor returns. `active` flips false; no respawn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_cell_died_exhausted_marks_failed_persists() {
        let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let path = Path::new("/x");
        let counter = Arc::new(AtomicU32::new(0));
        let mut registry = HashMap::new();
        // restart_count already at the limit (5) → next bump (6) exceeds it.
        registry.insert(
            path.clone(),
            entry_with_counting_respawn(path.clone(), 5, counter.clone()),
        );
        assert_eq!(
            handle_cell_died(&mut registry, &inbox_tx, path.clone(), DeathKind::Backstop).await,
            CellDiedOutcome::Failed { path: path.clone() }
        );
        assert!(
            registry.contains_key(&path),
            "exhausted → entry RETAINED (No-Delete), not removed"
        );
        let entry = registry.get(&path).unwrap();
        assert!(entry.failed, "exhausted → marked failed in-memory");
        assert!(!entry.active, "failed ⟹ !active");
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "respawn must NOT be invoked on exhaustion"
        );
    }

    // ---- Phase-13-E-1: spawn_watcher peace-aware tests ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_watcher_explicit_peace_no_cell_died() {
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let cell_join = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        spawn_watcher(
            &inbox_tx,
            meclaw_core::Path::new("/probe"),
            cell_join,
            peace_rx,
            backstop_rx,
        );
        let _ = peace_tx.send(());
        let r = tokio::time::timeout(std::time::Duration::from_millis(200), inbox_rx.recv()).await;
        assert!(r.is_err(), "no CellDied expected after explicit peace");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_watcher_no_peace_cell_exit_emits_cell_died_normal() {
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        // Backstop NOT fired (sender dropped) → clean exit classifies Normal.
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let cell_join = tokio::spawn(async {});
        spawn_watcher(
            &inbox_tx,
            meclaw_core::Path::new("/probe"),
            cell_join,
            peace_rx,
            backstop_rx,
        );
        drop(peace_tx);
        let msg = inbox_rx.recv().await.expect("CellDied expected");
        assert!(matches!(
            msg,
            ColonyMsg::CellDied {
                death_kind: DeathKind::Normal,
                ..
            }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_watcher_cell_panic_emits_cell_died_panic() {
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let cell_join = tokio::spawn(async { panic!("boom") });
        spawn_watcher(
            &inbox_tx,
            meclaw_core::Path::new("/probe"),
            cell_join,
            peace_rx,
            backstop_rx,
        );
        drop(peace_tx);
        let msg = inbox_rx.recv().await.expect("CellDied expected");
        assert!(matches!(
            msg,
            ColonyMsg::CellDied {
                death_kind: DeathKind::Panic,
                ..
            }
        ));
    }

    /// Paket-3 P3-B-restart: a clean (non-panic) exit WITH the backstop oneshot
    /// fired classifies `DeathKind::Backstop` → restart (not removal).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_watcher_backstop_fired_emits_cell_died_backstop() {
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (backstop_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        // Cell-task ends cleanly AFTER firing the backstop signal (the
        // `cell_task_stateful` backstop branch order: fire backstop_tx, return).
        let _ = backstop_tx.send(());
        let cell_join = tokio::spawn(async {});
        spawn_watcher(
            &inbox_tx,
            meclaw_core::Path::new("/probe"),
            cell_join,
            peace_rx,
            backstop_rx,
        );
        drop(peace_tx);
        let msg = inbox_rx.recv().await.expect("CellDied expected");
        assert!(matches!(
            msg,
            ColonyMsg::CellDied {
                death_kind: DeathKind::Backstop,
                ..
            }
        ));
    }

    /// AUDIT-PRE14-001 (panic priority): even if the backstop oneshot fired, a
    /// PANICKING join result wins → `DeathKind::Panic` (mutually exclusive).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_watcher_panic_wins_over_backstop() {
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (backstop_tx, backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let _ = backstop_tx.send(()); // backstop fired …
        let cell_join = tokio::spawn(async { panic!("boom") }); // … but it panicked
        spawn_watcher(
            &inbox_tx,
            meclaw_core::Path::new("/probe"),
            cell_join,
            peace_rx,
            backstop_rx,
        );
        drop(peace_tx);
        let msg = inbox_rx.recv().await.expect("CellDied expected");
        assert!(matches!(
            msg,
            ColonyMsg::CellDied {
                death_kind: DeathKind::Panic,
                ..
            }
        ));
    }

    /// Phase-13.5-hive-transit step-3: `build_transit_follow_up` chains the
    /// follow-up to the incoming message (`parent_message_id = Some(src.id)`),
    /// preserves `trace_id`/`ttl`/`body`, sets the new `target`/`headers`, and —
    /// per transparency mandate (Auflage 2) — passes `reply_to`/`correlation_id`
    /// through UNCHANGED (the hive is a transparent router, not a consumer).
    #[test]
    fn build_transit_follow_up_chains_parent_and_passes_reply_and_correlation() {
        use meclaw_core::{Body, MessageBuilder, Path};
        let corr = uuid::Uuid::now_v7();
        let src = MessageBuilder::new(Path::new("/hive"))
            .ttl(7)
            .reply_to(Path::new("/orig"))
            .correlation_id(corr)
            .body(Body::Inline(meclaw_core::serde_json::json!({"k": "v"})))
            .build();
        let mut hop = Map::new();
        hop.insert("msg_type".to_string(), Value::String("text".to_string()));
        let headers = Headers::from_parts(Map::new(), hop);

        let fu = build_transit_follow_up(&src, Path::new("/to"), headers.clone());

        assert_eq!(fu.parent_message_id, Some(src.id));
        assert_eq!(fu.trace_id, src.trace_id);
        assert_eq!(fu.ttl, src.ttl);
        assert_eq!(fu.target.as_str(), "/to");
        assert_eq!(fu.headers, headers);
        assert_eq!(fu.reply_to, src.reply_to);
        assert_eq!(fu.correlation_id, src.correlation_id);
        assert!(matches!(fu.body, Body::Inline(_)));
    }

    /// Phase-13.5-hive-transit step-3: `reply_to`/`correlation_id` that are
    /// `None` on the source stay `None` on the follow-up (no hive-substitution).
    #[test]
    fn build_transit_follow_up_passes_none_reply_and_correlation() {
        use meclaw_core::{MessageBuilder, Path};
        let src = MessageBuilder::new(Path::new("/hive")).ttl(3).build();
        assert!(src.reply_to.is_none());
        assert!(src.correlation_id.is_none());

        let fu = build_transit_follow_up(&src, Path::new("/to"), Headers::new());

        assert_eq!(fu.parent_message_id, Some(src.id));
        assert!(fu.reply_to.is_none());
        assert!(fu.correlation_id.is_none());
    }

    /// Phase-13.5-hive-transit step-2: `route()` returns `RouteAction::HiveTransit`
    /// when the resolved target misses the registry but hits the HiveScopeTable.
    #[tokio::test]
    async fn route_direct_hive_target_returns_hive_transit() {
        use meclaw_core::{MessageBuilder, Path};
        let registry: HashMap<Path, RegistryEntry> = HashMap::new();
        let mut hive_scopes = HiveScopeTable::new();
        hive_scopes.register(HiveScope {
            path: Path::new("/myhive"),
        });
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let msg = MessageBuilder::new(Path::new("/myhive")).ttl(5).build();
        let action = route(
            &registry,
            &hive_scopes,
            &mut dead_letters,
            Path::new("/"),
            msg,
        )
        .await;
        match action {
            RouteAction::HiveTransit { hive_path, .. } => {
                assert_eq!(hive_path.as_str(), "/myhive");
            }
            _ => panic!("expected HiveTransit"),
        }
        assert_eq!(dead_letters.len(), 0);
    }

    /// Phase-13.5-hive-transit step-2: a target that misses BOTH registry and
    /// HiveScopeTable stays an unresolved-path cascade (trennscharf zu HiveTransit).
    #[tokio::test]
    async fn route_direct_unknown_target_is_not_hive_transit() {
        use meclaw_core::{MessageBuilder, Path};
        let registry: HashMap<Path, RegistryEntry> = HashMap::new();
        let hive_scopes = HiveScopeTable::new();
        let mut dead_letters: VecDeque<DeadLetter> = VecDeque::new();
        let msg = MessageBuilder::new(Path::new("/nowhere")).ttl(5).build();
        let action = route(
            &registry,
            &hive_scopes,
            &mut dead_letters,
            Path::new("/sender"),
            msg,
        )
        .await;
        assert!(!matches!(action, RouteAction::HiveTransit { .. }));
    }

    // ── Phase-13.5 A8 T7: offload_oversized (producer hook) ──────────────────

    fn inline_msg(value: meclaw_core::serde_json::Value) -> Message {
        let mut m = MessageBuilder::new(Path::new("/sink")).build();
        m.body = meclaw_core::Body::Inline(value);
        m
    }

    #[tokio::test]
    async fn offload_below_threshold_stays_inline() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let value = meclaw_core::serde_json::json!({"x": "tiny"});
        let msg = offload_oversized(inline_msg(value.clone()), &Some(store), 1_000_000).await;
        match msg.body {
            meclaw_core::Body::Inline(v) => assert_eq!(v, value),
            meclaw_core::Body::Blob(_) => panic!("body < threshold must stay inline"),
        }
    }

    #[tokio::test]
    async fn offload_at_threshold_becomes_blob_a1_edge() {
        // A1 canonical: `>= threshold` → Blob. The `==` boundary case is inclusive.
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let value = meclaw_core::serde_json::json!({"system": "s", "messages": []});
        let exact = meclaw_core::serde_json::to_vec(&value).unwrap().len();
        let msg = offload_oversized(inline_msg(value.clone()), &Some(store.clone()), exact).await;
        match msg.body {
            meclaw_core::Body::Blob(id) => {
                let round = store.read_body(id).await.unwrap();
                assert_eq!(round, value, "offloaded blob round-trips to the same Value");
            }
            meclaw_core::Body::Inline(_) => panic!("body == threshold must offload (>= A1)"),
        }
    }

    #[tokio::test]
    async fn offload_idempotent_skips_existing_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let id = meclaw_core::Uuid::now_v7();
        let mut m = MessageBuilder::new(Path::new("/sink")).build();
        m.body = meclaw_core::Body::Blob(id);
        // threshold 0 would offload any inline body, but an existing blob is skipped.
        let msg = offload_oversized(m, &Some(store), 0).await;
        assert!(
            matches!(msg.body, meclaw_core::Body::Blob(b) if b == id),
            "already-Blob body is left untouched (idempotent re-check per hop)"
        );
    }

    #[tokio::test]
    async fn offload_noop_without_store() {
        let big = meclaw_core::serde_json::json!({"x": "x".repeat(10_000)});
        let msg = offload_oversized(inline_msg(big.clone()), &None, 0).await;
        match msg.body {
            meclaw_core::Body::Inline(v) => assert_eq!(v, big),
            meclaw_core::Body::Blob(_) => panic!("no store wired → never offload"),
        }
    }

    /// `canonical_scope_prefix` normalises guard_scope for parent-path comparisons.
    /// Root variants (empty string, bare "/") must both yield "/" so the root
    /// registry filter includes "/foo" paths (parent="/"). Trailing slashes on
    /// non-root scopes are trimmed to keep the prefix clean.
    #[test]
    fn canonical_scope_prefix_root_and_trim() {
        assert_eq!(canonical_scope_prefix(""), "/");
        assert_eq!(canonical_scope_prefix("/"), "/");
        assert_eq!(canonical_scope_prefix("/main"), "/main");
        assert_eq!(canonical_scope_prefix("/main/"), "/main");
    }
}
