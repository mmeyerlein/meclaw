//! Apply-phase of the filesystem bootstrap.
//!
//! Plan-phase (`plan_bootstrap`) produces a `BootstrapPlan`; this apply-phase
//! performs the side effects: register hive scopes, spawn cells, insert edges.
//!
//! Atomic-strict contract: if `plan_bootstrap` returned `Ok(plan)`, then
//! `apply_bootstrap_plan(plan)` MUST NOT fail. The `.expect("validated in
//! plan-phase")` calls below codify that contract — if they panic, the parser
//! invariant on `CellFactory` was broken.

use crate::{
    BootstrapErrors, BootstrapPlan, CellFactoryRegistry, ColonyMsg, ColonyRuntime, SpawnedCellKind,
};
use tokio::sync::oneshot;

/// Counts of what was applied. Useful for tests and diagnostics.
#[derive(Debug, Default, Clone)]
pub struct BootstrapReport {
    /// Number of hive scopes registered.
    pub hive_count: usize,
    /// Number of cells spawned and registered.
    pub cell_count: usize,
    /// Number of edges inserted.
    pub edge_count: usize,
}

/// Execute a validated `BootstrapPlan` against a running Colony.
///
/// Atomic-strict contract (see module doc): every `.expect` here is safeguarded
/// by the plan-phase validation. If any expect fires, the parser-invariant on
/// `CellFactory` was violated.
pub async fn apply_bootstrap_plan(
    plan: BootstrapPlan,
    factories: &CellFactoryRegistry,
    runtime: &ColonyRuntime,
) -> BootstrapReport {
    // Bootstrap-Recovery (Run-5/5b-Befund): durable `bootstrap_in_flight`
    // marker BEFORE the first spawn. A crash anywhere in this apply (e.g. the
    // U11-shape spawn panic below) leaves the marker behind, and the next boot
    // resumes as a FirstBoot (idempotent re-apply from the filesystem) instead
    // of panicking on the mixed registry/edges/hive_scopes state. The matching
    // clear runs atomically inside the InitialApply transaction at the end.
    {
        let (ack_tx, ack_rx) = oneshot::channel();
        runtime
            .inbox_tx
            .send(ColonyMsg::BeginInitialApply { ack: ack_tx })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("BeginInitialApply ack");
    }
    // Cells: spawn via factory + register via ColonyMsg::Register (Active) or
    // ColonyMsg::RegisterDormant (Dormant, Phase-13-K-2).
    for c in &plan.cells {
        // Phase-13.5 Lifecycle-3b Task 9.1, demo (g): boot-gating of inactive
        // cells (startup step 6, "long-running only for active cells"). A
        // rehydrated cell whose persisted `status == 'inactive'` (overlay →
        // `c.active == false`) must NOT get a running task at boot. We skip the
        // factory `spawn_cell` call entirely (no task built, no polling) and
        // register an inert non-running entry instead: the registry slot EXISTS
        // (with `active == false`), so reads see it and routing dead-letters it
        // as `cell_inactive`, but no cell-task runs. This applies symmetrically
        // to eager (Long-Running / Stateless) and lazy (Stateful) kinds — both
        // register as `NotYetSpawned` with no task; the only difference at boot
        // is that an active eager cell would otherwise have been eager-spawned,
        // which is exactly what this skip prevents.
        //
        // Phase-13.5 Slice 4 T7 (checklist c, demo f): an eager cell that boots
        // inactive now receives a REAL `respawn` closure via the factory's
        // `build_boot_inactive_respawn` hook (the SAME construction a normal
        // eager cell's `RespawnFn` gets — captures `cell_dir` / `params` /
        // `colony_inbox_tx`), WITHOUT spawning the task at boot (boot-gating
        // preserved). It registers `eager_on_reconnect == true`, so a later
        // `add_edges` reconnect's `(entry.respawn)()` spawns the task IMMEDIATELY
        // (and re-notifies the fresh stop pair, T6) — no reboot needed. Lazy
        // stateful cells (no hook, returns `None`) get the factory's REAL
        // wake-on-message wiring via `spawn_cell` (F1-KH2 inventory #3; lazy
        // `spawn_cell` builds no task, so boot-gating is preserved).
        if !c.active {
            register_inactive_non_spawned(runtime, factories, c).await;
            continue;
        }
        let factory = factories
            .get(&c.cell_type)
            .cloned()
            .expect("validated in plan-phase: unknown cell_type cannot reach apply");
        // Phase-13-K-2: idle_timeout-Mapping aktiv. `cell.timeout == 0` →
        // Default-Idle (oder per-cell-Override via `idle_timeout_ms`). Andere
        // values (-1 persistent, >0 one-shot) get no idle timer.
        let idle_timeout = match c.cell_timeout {
            0 => Some(std::time::Duration::from_millis(
                c.idle_timeout_ms
                    .unwrap_or(runtime.colony_config.idle_timeout_default_ms),
            )),
            _ => None,
        };
        // paket-7 B5 (Auflage A3): resolve the effective emits-validation flag
        // BEFORE `spawn_cell` builds the RespawnFn closure that clones this
        // `ContractView`, so a crash-restarted cell carries the resolved flag.
        let mut contract_view = c.contract_view.clone();
        contract_view.validate_emits =
            resolve_validate_emits(runtime.colony_config.strict_validation);
        // Hardening Slice 1: capture the contract pieces for SetNodeContract
        // BEFORE `spawn_cell` consumes the `ContractView`.
        let nc_emits = contract_view.emits.clone();
        let nc_validate_emits = contract_view.validate_emits;
        let spawned = factory
            .spawn_cell(
                c.path.clone(),
                c.params.clone(),
                runtime.outputs_tx.clone(),
                c.fs_path.clone(),
                contract_view,
                runtime.inbox_tx.clone(),
                idle_timeout,
                c.cell_timeout,
                // P3-B-plumb-2: resolve the active B-backstop from the per-cell
                // `cell.message_timeout` against the colony
                // `message_timeout_default_ms`. `>0` → backstop, `0`/`-1` → None.
                crate::resolve_message_timeout(
                    c.message_timeout,
                    runtime.colony_config.message_timeout_default_ms,
                ),
                runtime.blob_store.clone(),
                c.mailbox_size
                    .unwrap_or(runtime.colony_config.mailbox_default_capacity),
            )
            .expect("validated in plan-phase: invalid params cannot reach apply");
        let (ack_tx, ack_rx) = oneshot::channel();
        match spawned {
            SpawnedCellKind::Active {
                sender,
                join,
                peace_rx,
                // Phase-13.5 Lifecycle-3b Task 4 (F2): the colony-initiated
                // peace-stop trigger + death-ack are passed into the registry so
                // the recompute-hook can disconnect this cell.
                stop_tx,
                death_ack_rx,
                // Paket-3 P3-B-restart: forwarded to the colony watcher (via
                // ColonyMsg::Register → handle_register → spawn_watcher) so a
                // backstop death of this cell classifies DeathKind::Backstop.
                backstop_rx,
                respawn,
            } => {
                // Stateless / long-running cells stay Awake forever → no wake
                // mechanic (F1-KH2 Schicht 2: a stray parked delivery
                // dead-letters loudly instead of invoking an inert closure).
                let wake: Option<crate::WakeFn> = None;
                runtime
                    .inbox_tx
                    .send(ColonyMsg::Register {
                        path: c.path.clone(),
                        sender,
                        join,
                        peace_rx,
                        backstop_rx,
                        stop_tx: Some(stop_tx),
                        death_ack_rx: Some(death_ack_rx),
                        respawn,
                        wake,
                        restart_limit: c.restart_limit,
                        cell_id: c.cell_id,
                        cell_type: c.cell_type.clone(),
                        // Phase-13.5 Lifecycle-3b: overlay-derived activity.
                        active: c.active,
                        ack: ack_tx,
                    })
                    .await
                    .expect("colony inbox closed");
            }
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                // Phase-13.5 Lifecycle-3b Task 3: dropped here; Task 4 stores
                // them in the registry to drive the lazy-wake peace-stop.
                stop_tx: _,
                death_ack_rx: _,
                respawn,
            } => {
                // Phase-13-K-2: stateful Cells starten als `NotYetSpawned` —
                // The mailbox receiver parks in the status; the cell task only on the first
                // Wake-Pre-Send (13-I-1) gespawnt.
                runtime
                    .inbox_tx
                    .send(ColonyMsg::RegisterDormant {
                        path: c.path.clone(),
                        sender,
                        receiver,
                        respawn,
                        wake: Some(wake),
                        restart_limit: c.restart_limit,
                        cell_id: c.cell_id,
                        cell_type: c.cell_type.clone(),
                        // Phase-13.5 Lifecycle-3b: overlay-derived activity.
                        active: c.active,
                        // Paket-6 C: overlay-derived failure flag.
                        failed: c.failed,
                        // Lazy stateful (Dormant factory kind) → wake-on-message.
                        eager_on_reconnect: false,
                        ack: ack_tx,
                    })
                    .await
                    .expect("colony inbox closed");
            }
        }
        ack_rx.await.expect("Register ack");
        // Hardening Slice 1: register the per-cell contract data in the
        // colony's `node_contracts` map (14-B header projection + compiled
        // emits + resolved enforcement flag) after the successful
        // Register/RegisterDormant ack.
        let (nc_ack_tx, nc_ack_rx) = oneshot::channel();
        runtime
            .inbox_tx
            .send(ColonyMsg::SetNodeContract {
                path: c.path.clone(),
                contract: crate::NodeContract {
                    header_view: c.header_view.clone(),
                    emits: nc_emits,
                    validate_emits: nc_validate_emits,
                },
                ack: nc_ack_tx,
            })
            .await
            .expect("colony inbox closed");
        nc_ack_rx.await.expect("SetNodeContract ack");
        index_provenance(runtime, c).await;
    }
    // FIX 3 — InitialApply bundle: hive_scopes + edges in ONE transaction (atomic).
    // The handler also enters both in memory (EdgeTable + HiveScopeTable).
    {
        let (ack_tx, ack_rx) = oneshot::channel();
        runtime
            .inbox_tx
            .send(ColonyMsg::InitialApply {
                edges: plan.edges.clone(),
                hive_scopes: plan.hives.iter().map(|h| h.path.clone()).collect(),
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("InitialApply ack");
    }
    // A8 (Phase-16 W1a, Ruling 2026-06-12): the boot endpoint-existence check
    // ran registry-aware in `bootstrap_from_filesystem_with_env` BEFORE apply —
    // a genuinely-unresolved endpoint already failed the boot there. Every edge
    // that reaches apply resolves against the plan, the live registry, or
    // `/colony/*`, so no observability warn is emitted here anymore (the old
    // plan-only warn would have falsely flagged resolved registry-only sinks).
    BootstrapReport {
        hive_count: plan.hives.len(),
        cell_count: plan.cells.len(),
        edge_count: plan.edges.len(),
    }
}

/// GH #62: copy the node's `cell.provenance` into the `colony.db` `registry`
/// index, after its registration ack.
///
/// The file is the source of truth for a node's origin; the registry columns are
/// the query index over it. Running this at every boot (not only at
/// instantiation) is what makes a restored or imported tree re-index itself:
/// its `config.json` files carry the provenance, its freshly minted `colony.db`
/// does not. A node without provenance sends nothing and keeps NULL.
async fn index_provenance(runtime: &ColonyRuntime, c: &crate::bootstrap::PlannedCell) {
    let Some(provenance) = c.provenance.clone() else {
        return;
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    runtime
        .inbox_tx
        .send(ColonyMsg::SetRegistryProvenance {
            path: c.path.clone(),
            provenance,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("SetRegistryProvenance ack");
}

/// Register a rehydrated **inactive** cell as a non-running registry entry
/// (Phase-13.5 Lifecycle-3b Task 9.1, demo (g); Slice 4 T7, demo f).
///
/// No factory `spawn_cell` is called, so no cell-task is built and no polling
/// starts — the boot-gating guarantee. The entry is inserted via
/// `RegisterDormant` (status `NotYetSpawned`, no task, no watcher) with
/// `active == false`. The mailbox sender/receiver pair is a fresh throwaway
/// channel (the cell never runs, so nothing is ever sent through it while
/// inactive — routing dead-letters to an inactive cell short-circuit).
///
/// **Eager kinds (T7)**: the factory's `build_boot_inactive_respawn` hook builds
/// a REAL `respawn` closure WITHOUT spawning the task (boot-gating preserved).
/// We register `eager_on_reconnect == true` with that real closure, so a later
/// `add_edges` reconnect's `(entry.respawn)()` spawns the task IMMEDIATELY (no
/// reboot). The `wake` stays the inert no-op (an eager cell is never woken — it
/// is re-spawned, not woken).
///
/// **Lazy kinds (and any factory that returns `None` from the hook)**: get the
/// factory's REAL `WakeFn` + mailbox pair + `RespawnFn` via `spawn_cell`
/// (F1-KH2 inventory #3 — lazy `spawn_cell` builds NO task, boot-gating
/// preserved) and `eager_on_reconnect == false`: an `add_edges` reconnect
/// leaves the cell parked and the first delivery wakes it (Hot/Cold).
async fn register_inactive_non_spawned(
    runtime: &ColonyRuntime,
    factories: &CellFactoryRegistry,
    c: &crate::bootstrap::PlannedCell,
) {
    use tokio::sync::oneshot;
    // Try the factory's boot-inactive respawn hook (T7). `Some` → a real respawn
    // for an eager kind (eager re-spawn on reconnect); `None` → lazy / opted-out
    // → real wake-on-message wiring via `spawn_cell` (F1-KH2 inventory #3).
    let idle_timeout = match c.cell_timeout {
        0 => Some(std::time::Duration::from_millis(
            c.idle_timeout_ms
                .unwrap_or(runtime.colony_config.idle_timeout_default_ms),
        )),
        _ => None,
    };
    // paket-7 B5 (Auflage A3): resolve the effective emits-validation flag
    // BEFORE the boot-inactive reconnect hook captures this `ContractView`, so a
    // reconnect-respawned eager cell carries the resolved flag.
    let mut contract_view = c.contract_view.clone();
    contract_view.validate_emits = resolve_validate_emits(runtime.colony_config.strict_validation);
    let factory = factories.get(&c.cell_type).cloned();
    // F1-KH2 kind discriminator: declared on the trait, so the kind is known
    // WITHOUT building a task (an eager `spawn_cell` would violate boot-gating,
    // demo (g) — "no task runs at boot for an inactive cell").
    let is_lazy = factory.as_ref().map(|f| f.is_lazy()).unwrap_or(false);
    let real_respawn = factory
        .as_ref()
        .cloned()
        .filter(|_| !is_lazy)
        .and_then(|f| {
            f.build_boot_inactive_respawn(
                c.path.clone(),
                c.params.clone(),
                runtime.outputs_tx.clone(),
                c.fs_path.clone(),
                contract_view.clone(),
                runtime.inbox_tx.clone(),
                idle_timeout,
                c.cell_timeout,
                // P3-B-plumb-1: behavior-neutral — message_timeout resolved later.
                None,
                runtime.blob_store.clone(),
                c.mailbox_size
                    .unwrap_or(runtime.colony_config.mailbox_default_capacity),
            )
        });

    // F1-KH2 kind split (inventory finding #3 — same pre-R12 class as the
    // subtree path): a boot-INACTIVE lazy cell reconnects via `add_edges` as
    // wake-on-message, so its registration MUST carry the factory's REAL
    // WakeFn. The old inert wake dropped the parked receiver on the first
    // post-reconnect delivery (silent loss + false `Awake`). Lazy `spawn_cell`
    // builds NO task (the Dormant pair parks) — boot-gating is preserved.
    // Eager kinds keep the throwaway pair + inert wake (re-spawned, not woken).
    let (sender, receiver, wake, respawn, eager_on_reconnect): (
        tokio::sync::mpsc::Sender<meclaw_core::Message>,
        tokio::sync::mpsc::Receiver<meclaw_core::Message>,
        Option<crate::WakeFn>,
        crate::RespawnFn,
        bool,
    ) = if let Some(real_respawn) = real_respawn {
        // Eager kind (boot-inactive hook): parked throwaway pair, never used
        // while inactive (inactive-routing short-circuits before route()).
        let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        (sender, receiver, None, real_respawn, true)
    } else if is_lazy {
        let spawned = factory.map(|f| {
            f.spawn_cell(
                c.path.clone(),
                c.params.clone(),
                runtime.outputs_tx.clone(),
                c.fs_path.clone(),
                contract_view.clone(),
                runtime.inbox_tx.clone(),
                idle_timeout,
                c.cell_timeout,
                crate::resolve_message_timeout(
                    c.message_timeout,
                    runtime.colony_config.message_timeout_default_ms,
                ),
                runtime.blob_store.clone(),
                c.mailbox_size
                    .unwrap_or(runtime.colony_config.mailbox_default_capacity),
            )
        });
        match spawned {
            Some(Ok(SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                // Dormant placeholder stop wiring belongs to the PRE-wake state —
                // dropped exactly like the active-boot Dormant arm.
                stop_tx: _,
                death_ack_rx: _,
                respawn,
            })) => (sender, receiver, Some(wake), respawn, false),
            Some(Ok(SpawnedCellKind::Active {
                sender: _,
                join: _,
                peace_rx: _,
                stop_tx,
                death_ack_rx: _,
                backstop_rx: _,
                respawn,
            })) => {
                // Unreachable in practice: every eager built-in implements the
                // boot-inactive hook. Best-effort: peace-stop the transient task
                // and keep the eager parked shape with the REAL respawn.
                tracing::error!(
                    path = %c.path.as_str(),
                    "boot-inactive spawn: factory returned Active without a \
                     boot-inactive hook — stopping the transient task"
                );
                let _ = stop_tx.send(());
                let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                (sender, receiver, None, respawn, true)
            }
            Some(Err(e)) => {
                // Atomic-strict contract: params were validated in the
                // plan-phase, a spawn error here is exceptional. Register the
                // inert fallback; deliveries fail LOUDLY (defense layer).
                tracing::error!(
                    error = %e,
                    path = %c.path.as_str(),
                    "boot-inactive spawn_cell failed — registering inert fallback"
                );
                let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                (sender, receiver, None, boot_inactive_inert_respawn(), false)
            }
            None => {
                // `is_lazy == true` implies the factory exists — defensive only.
                tracing::error!(
                    path = %c.path.as_str(),
                    cell_type = %c.cell_type,
                    "boot-inactive registration: no factory — registering inert fallback"
                );
                let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
                (sender, receiver, None, boot_inactive_inert_respawn(), false)
            }
        }
    } else {
        // Eager kind WITHOUT a boot-inactive hook (or factory missing): no
        // task at boot (gating), no wake mechanic. After a reconnect such a
        // cell stays parked and deliveries dead-letter loudly (`cell_inactive`,
        // defense layer) — never silent loss.
        let (sender, receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        (sender, receiver, None, boot_inactive_inert_respawn(), false)
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    runtime
        .inbox_tx
        .send(ColonyMsg::RegisterDormant {
            path: c.path.clone(),
            sender,
            receiver,
            respawn,
            wake,
            restart_limit: c.restart_limit,
            cell_id: c.cell_id,
            cell_type: c.cell_type.clone(),
            active: c.active,
            failed: c.failed,
            eager_on_reconnect,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("RegisterDormant ack");
    // Hardening Slice 1: register the per-cell contract data after the
    // successful RegisterDormant ack — same wiring as the active spawn loop,
    // using the locally resolved `contract_view` (resolve_validate_emits above).
    let (nc_ack_tx, nc_ack_rx) = oneshot::channel();
    runtime
        .inbox_tx
        .send(ColonyMsg::SetNodeContract {
            path: c.path.clone(),
            contract: crate::NodeContract {
                header_view: c.header_view.clone(),
                emits: contract_view.emits.clone(),
                validate_emits: contract_view.validate_emits,
            },
            ack: nc_ack_tx,
        })
        .await
        .expect("colony inbox closed");
    nc_ack_rx.await.expect("SetNodeContract ack");
    index_provenance(runtime, c).await;
}

/// Inert RespawnFn for the exceptional boot-inactive fallbacks (no factory /
/// spawn error). Never invoked for a parked, non-eager cell.
fn boot_inactive_inert_respawn() -> crate::RespawnFn {
    Box::new(|| {
        tracing::error!(
            "inert RespawnFn invoked on a boot-inactive non-eager cell — should \
             never happen (no real task wiring; reconnect is wake-on-message). No-op."
        );
        let (s, _r) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
        let (_pt, pr) = oneshot::channel::<()>();
        let (_bt, br) = oneshot::channel::<()>();
        let join = tokio::spawn(async {});
        (s, join, pr, br)
    })
}

/// Boot endpoint-existence check (Phase-16 W1a A8, Ruling 2026-06-12).
///
/// Returns the `params.graph` edge endpoints the running colony resolves to
/// NOTHING: not a plan cell, not a plan hive, not an already-live registry
/// path (a runtime-spawned sink registered BEFORE bootstrap — the `h.spawn`
/// pattern), and not a `/colony/*` virtual endpoint. Such an endpoint is a
/// typo / dead wiring → the real boot fails LOUD (precise: edge id + missing
/// path). The registry term carries the test-spawn pattern; in production a
/// registry-only-without-FS node cannot occur (A5b registration model), so the
/// check is as sharp as the strict reading there. One entry per unresolved
/// side of every edge: `(edge_id, missing_endpoint)`.
pub fn unresolved_boot_endpoints(
    plan: &BootstrapPlan,
    registry_paths: &std::collections::HashSet<String>,
) -> Vec<(meclaw_core::Uuid, meclaw_core::Path)> {
    let resolvable = |p: &meclaw_core::Path| -> bool {
        let s = p.as_str();
        // `/colony/*` is a virtual (non-cell) routing endpoint family; the root
        // `/` is always a scope. Otherwise the path must be a plan cell, a plan
        // hive, or an already-live registry entry.
        s == "/"
            || s.starts_with("/colony")
            || plan.cells.iter().any(|c| c.path.as_str() == s)
            || plan.hives.iter().any(|h| h.path.as_str() == s)
            || registry_paths.contains(s)
    };
    let mut out = Vec::new();
    for edge in &plan.edges {
        if !resolvable(&edge.from) {
            out.push((edge.id, edge.from.clone()));
        }
        if !resolvable(&edge.to) {
            out.push((edge.id, edge.to.clone()));
        }
    }
    out
}

/// Top-level bootstrap entry point: plan-phase then apply-phase.
///
/// Scans `root` for `config.json` files, validates the resulting plan, and
/// applies it to the running Colony referenced by `runtime`.
pub async fn bootstrap_from_filesystem(
    root: &std::path::Path,
    factories: &CellFactoryRegistry,
    runtime: &ColonyRuntime,
) -> Result<BootstrapReport, BootstrapErrors> {
    bootstrap_from_filesystem_with_env(root, factories, runtime, None).await
}

/// `bootstrap_from_filesystem` with an explicit `.env` location (U7, `--env`
/// CLI flag). `None` keeps the `{root}/.env` default.
pub async fn bootstrap_from_filesystem_with_env(
    root: &std::path::Path,
    factories: &CellFactoryRegistry,
    runtime: &ColonyRuntime,
    env_path: Option<&std::path::Path>,
) -> Result<BootstrapReport, BootstrapErrors> {
    // Phase-13.5 Lifecycle-3a: read the identity overlay from `root/colony.db`'s
    // registry table (read-only probe, co-located with the FS-walk builder so
    // the builder consults persistence for known paths). On first boot the file
    // is absent → empty overlay → every cell gets a fresh cell_id. Corrupt
    // identity data hard-fails the boot rather than silently re-minting cell_ids.
    let overlay = crate::persist::colony_db::read_registry_overlay(&root.join("colony.db"))
        .expect("read colony.db identity overlay (corrupt registry must hard-fail boot)");
    // A5b (Phase-16 W1b): classify the boot so the walk knows whether an
    // overlay-miss cell is the FirstBoot source-of-truth (planned) or a Reboot
    // unknown node (reported, never adopted). A probe failure is non-fatal here
    // — fall back to FirstBoot so the walk keeps adopting (the dedicated boot
    // classifier in `colony_task` strict-fails an inconsistent DB separately).
    let boot_state = crate::bootstrap::probe_boot_state(&root.join("colony.db"))
        .unwrap_or(crate::bootstrap::BootState::FirstBoot);
    let plan =
        crate::bootstrap::plan_bootstrap_with_env(root, factories, &overlay, boot_state, env_path)?;
    // A5b: a Reboot that walked over unknown cell dirs reports them — loudly, so
    // the operator sees the consistency drift — but the boot succeeds (they are
    // simply not registered; instantiation/mutation is the only registration
    // path). Empty on FirstBoot.
    for path in &plan.unregistered_nodes {
        tracing::warn!(
            path = %path.as_str(),
            "reboot found an unregistered cell directory — NOT adopted (registration is \
             instantiation/mutation-only); mutate with `adopt` to register it"
        );
    }
    // A8 (Phase-16 W1a, Ruling 2026-06-12): boot endpoint-existence check
    // against the LIVE colony — plan cells/hives ∪ already-live registry paths
    // (runtime-spawned sinks registered before bootstrap) ∪ `/colony/*`. An
    // endpoint resolving to none of these is a typo / dead edge → LOUD boot
    // fail, BEFORE any spawn (the apply phase stays the side-effecting half).
    // The resolvable registry universe is the LIVE RAM registry (runtime-spawned
    // cells) PLUS the persisted overlay (colony.db `registry` table): an ORPHAN
    // overlay entry — a node whose FS dir was removed but whose `cell_id`/status
    // persist (No-Delete-Policy) — is a legitimate edge endpoint even though it
    // is not in the FS plan and is not re-spawned into the live registry.
    let mut registry_paths = snapshot_registry_paths(runtime).await;
    registry_paths.extend(overlay.keys().map(|p| p.as_str().to_string()));
    let unresolved = unresolved_boot_endpoints(&plan, &registry_paths);
    if !unresolved.is_empty() {
        let mut errors = BootstrapErrors::new();
        for (edge_id, endpoint) in unresolved {
            errors.push(crate::bootstrap::BootstrapError::DanglingEndpoint { edge_id, endpoint });
        }
        return Err(errors);
    }
    let report = apply_bootstrap_plan(plan, factories, runtime).await;
    warn_on_missing_drains_after_boot(root, runtime).await;
    Ok(report)
}

/// GH #147, the boot half: say it out loud when a hive port that declared a
/// paired drain is wired without one.
///
/// A warning, never a refusal — the mutation path is where this rule bites,
/// because that is somebody changing a colony they did not necessarily build.
/// The birth topology is authorship, the same reason the port boundary leaves
/// the bootstrap alone (GH #133).
///
/// Runs after apply, so it sees the topology the colony actually woke up with —
/// including the edges rehydrated from `colony.db`, which the plan does not
/// carry.
async fn warn_on_missing_drains_after_boot(root: &std::path::Path, runtime: &ColonyRuntime) {
    let (ack_tx, ack_rx) = oneshot::channel();
    if runtime
        .inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: meclaw_core::Path::new("/"),
            ack: ack_tx,
        })
        .await
        .is_err()
    {
        return;
    }
    let Ok(graph) = ack_rx.await else {
        return;
    };
    let hive_paths: Vec<meclaw_core::Path> = graph
        .nodes
        .iter()
        .filter(|n| n.cell_type == "hive")
        .map(|n| meclaw_core::Path::new(&n.path))
        .collect();
    let reqs = crate::mutation::required_drains::collect_required_drains(root, hive_paths.iter());
    let edges: Vec<(String, String, Option<String>)> = graph
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone(), e.condition.clone()))
        .collect();
    crate::mutation::required_drains::warn_on_missing_drains(&reqs, &edges);
}

/// Snapshot the set of registered node paths from the running colony (A8).
/// Used by the boot endpoint-existence check to resolve runtime-spawned cells
/// (registered before bootstrap) that are not part of the FS plan.
async fn snapshot_registry_paths(runtime: &ColonyRuntime) -> std::collections::HashSet<String> {
    let (ack_tx, ack_rx) = oneshot::channel();
    runtime
        .inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: usize::MAX,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    let reply = ack_rx.await.expect("ReadRegistry ack");
    reply.entries.into_iter().map(|e| e.path).collect()
}

/// Resolve the effective emits-validation flag at spawn time (F1 Knopf-Kette):
/// `cfg!(debug_assertions) || colony.json strict_validation`.
pub(crate) fn resolve_validate_emits(strict_validation: bool) -> bool {
    cfg!(debug_assertions) || strict_validation
}

#[cfg(test)]
mod tests {
    use super::resolve_validate_emits;
    use crate::{BootstrapPlan, CellFactoryRegistry, ColonyRuntime, PlannedHive};
    use meclaw_core::Path;

    // ── Unit test for the `resolve_validate_emits` helper (paket-7 B5) ────────

    #[test]
    fn effective_validate_emits_resolves_knob_chain() {
        // F1 Knopf-Ketten-Pin: debug_assertions ODER strict_validation.
        assert!(resolve_validate_emits(true)); // strict on → always
        assert_eq!(resolve_validate_emits(false), cfg!(debug_assertions)); // strict off → debug only
    }

    // ── Unit tests for the pure `dangling_edge_endpoints` helper ─────────────

    /// Helper: build a minimal `PlannedEdge` with a given from/to pair.
    fn make_edge(from: &str, to: &str) -> crate::PlannedEdge {
        crate::PlannedEdge {
            id: meclaw_core::Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: None,
            modifier: None,
        }
    }

    /// Helper: build a minimal `PlannedCell` with a given path.
    fn make_cell(path: &str) -> crate::PlannedCell {
        crate::PlannedCell {
            path: Path::new(path),
            fs_path: std::path::PathBuf::from(path),
            cell_type: "echo".into(),
            params: meclaw_core::serde_json::json!({}),
            restart_limit: None,
            cell_id: meclaw_core::Uuid::now_v7(),
            contract_view: crate::factory::ContractView::default(),
            cell_timeout: 0,
            idle_timeout_ms: None,
            message_timeout: None,
            active: true,
            failed: false,
            mailbox_size: None,
            header_view: crate::mutation::validate::HeaderNodeView::default(),
            provenance: None,
        }
    }

    /// No dangling endpoints when all edge endpoints are known cells.
    #[test]
    fn dangling_edge_endpoints_empty_when_all_known() {
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/a"), make_cell("/b")],
            edges: vec![make_edge("/a", "/b")],
            unregistered_nodes: vec![],
        };
        let d = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert!(d.is_empty(), "no dangling endpoints expected, got {d:?}");
    }

    /// An edge whose `to` is not in cells or hives is reported as dangling.
    #[test]
    fn dangling_edge_endpoints_reports_unknown_to() {
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/a")],
            // /sink is a registry-only cell (h.spawn), not in the FS plan.
            edges: vec![make_edge("/a", "/sink")],
            unregistered_nodes: vec![],
        };
        let d = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert_eq!(d.len(), 1, "one dangling endpoint expected, got {d:?}");
        assert_eq!(d[0].1.as_str(), "/sink");
    }

    /// An edge whose `from` is not known is also reported.
    #[test]
    fn dangling_edge_endpoints_reports_unknown_from() {
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/b")],
            edges: vec![make_edge("/ghost", "/b")],
            unregistered_nodes: vec![],
        };
        let d = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert_eq!(d.len(), 1, "one dangling endpoint expected, got {d:?}");
        assert_eq!(d[0].1.as_str(), "/ghost");
    }

    /// Both endpoints unknown → two entries.
    #[test]
    fn dangling_edge_endpoints_reports_both_unknown() {
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![],
            edges: vec![make_edge("/x", "/y")],
            unregistered_nodes: vec![],
        };
        let d = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert_eq!(d.len(), 2, "two dangling endpoints expected, got {d:?}");
    }

    /// A hive endpoint (known in `plan.hives`) is NOT dangling.
    #[test]
    fn dangling_edge_endpoints_hive_endpoint_not_dangling() {
        let plan = BootstrapPlan {
            hives: vec![
                PlannedHive {
                    path: Path::new("/"),
                },
                PlannedHive {
                    path: Path::new("/pool"),
                },
            ],
            cells: vec![make_cell("/a")],
            // /pool is a hive → known → not dangling.
            edges: vec![make_edge("/a", "/pool")],
            unregistered_nodes: vec![],
        };
        let d = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert!(
            d.is_empty(),
            "hive endpoint must not be dangling, got {d:?}"
        );
    }

    // ── A8 (Phase-16 W1a): registry-aware boot endpoint-existence check ───────

    /// The apply-phase check resolves a registry-only endpoint (pre-spawned via
    /// h.spawn) AND a `/colony/*` virtual endpoint, but flags a genuine typo.
    #[test]
    fn unresolved_boot_endpoints_resolves_registry_only_and_colony_but_flags_typo() {
        use std::collections::HashSet;
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/a")],
            edges: vec![
                make_edge("/a", "/sink"),             // registry-only → resolved
                make_edge("/a", "/colony/mutations"), // virtual endpoint → resolved
                make_edge("/a", "/bogus"),            // typo → flagged
            ],
            unregistered_nodes: vec![],
        };
        let mut registry: HashSet<String> = HashSet::new();
        registry.insert("/sink".to_string());
        let u = super::unresolved_boot_endpoints(&plan, &registry);
        assert_eq!(u.len(), 1, "only /bogus must be unresolved, got {u:?}");
        assert_eq!(u[0].1.as_str(), "/bogus");
    }

    // ── Plan-level pin: dangling endpoint stays in the plan (apply decides) ───

    /// PLAN level (A8, Phase-16 W1a): `plan_bootstrap` stays `Ok` with a
    /// `params.graph` edge to a non-FS endpoint and the edge is in the plan —
    /// the plan phase cannot know the live registry, so it never decides
    /// existence. The registry-aware LOUD boot fail (typo) vs. commit
    /// (registry-only sink) lives at the APPLY path
    /// (`phase_16_w1a_boot_endpoint.rs`, the flipped case e). `--validate` uses
    /// `dangling_edge_endpoints` to WARN over the same plan-only view.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bootstrap_commits_with_dangling_endpoint_no_reject() {
        use crate::{persist::colony_db::RegistryOverlay, plan_bootstrap};
        use tempfile::TempDir;

        let td = TempDir::new().unwrap();
        // Root hive declares an edge to "/sink" which is NOT in the FS tree
        // (registry-only cell via h.spawn at runtime).
        std::fs::create_dir_all(td.path().join("main")).unwrap();
        std::fs::write(
            td.path().join("main/config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"/sink"}]}}}"#,
        )
        .unwrap();

        let factories = CellFactoryRegistry::new();
        let overlay = RegistryOverlay::new();
        // plan_bootstrap must succeed (Ok), not reject because /sink is absent.
        let plan = plan_bootstrap(td.path(), &factories, &overlay)
            .expect("bootstrap must succeed even with dangling /sink endpoint");

        // The edge is in the plan (from=/ to=/sink).
        assert_eq!(plan.edges.len(), 1, "the dangling edge must be in the plan");
        assert_eq!(plan.edges[0].to.as_str(), "/sink");

        // dangling_edge_endpoints identifies /sink as dangling.
        let dangling = super::unresolved_boot_endpoints(&plan, &std::collections::HashSet::new());
        assert!(
            dangling.iter().any(|(_, ep)| ep.as_str() == "/sink"),
            "/sink must be identified as dangling, got {dangling:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn apply_via_runtime_registers_hive_count() {
        let plan = BootstrapPlan {
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![],
            edges: vec![],
            unregistered_nodes: vec![],
        };
        let (inbox_tx, mut inbox_rx) = tokio::sync::mpsc::channel(8);
        let (outputs_tx, _) = tokio::sync::mpsc::channel(8);
        let rt = ColonyRuntime {
            inbox_tx,
            outputs_tx,
            colony_config: crate::ColonyConfig::default(),
            blob_store: None,
        };
        // Fake inbox-consumer: acks BeginInitialApply (bootstrap-recovery
        // marker handshake) + InitialApply (replaces AddHiveScope after FIX 3).
        let consumer = tokio::spawn(async move {
            while let Some(msg) = inbox_rx.recv().await {
                match msg {
                    crate::ColonyMsg::BeginInitialApply { ack } => {
                        let _ = ack.send(());
                    }
                    crate::ColonyMsg::InitialApply { ack, .. } => {
                        let _ = ack.send(());
                    }
                    _ => {}
                }
            }
        });
        let factories = CellFactoryRegistry::new();
        let report = crate::apply_bootstrap_plan(plan, &factories, &rt).await;
        assert_eq!(report.hive_count, 1);
        drop(rt); // closes inbox_tx → consumer ends
        consumer.await.unwrap();
    }
}
