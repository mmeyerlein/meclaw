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
///
/// GH #285 — `slot_endpoints` is the second known-set, and the only term that
/// resolves an address with no node behind it: a hive that declared a SLOT
/// (`declared_slot_endpoints`) said the address exists and may stand empty. It
/// is a separate set on purpose. The persisted set answers "this colony
/// registered it once"; this one answers "a hive promised it" — and a promise
/// must not turn its subject into a registered node, which is exactly what
/// folding the two together would quietly assert.
pub fn unresolved_boot_endpoints(
    plan: &BootstrapPlan,
    registry_paths: &std::collections::HashSet<String>,
    slot_endpoints: &std::collections::HashSet<String>,
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
            // GH #285: …or a DECLARED slot of a hive in this tree. The only
            // resolvable address that is not a node: a hive said this address
            // exists and may stand empty, so the edge onto it is the edge the
            // declaration invited. Everything else keeps its teeth — a typo
            // cannot inherit the exemption, because nothing declared it.
            || slot_endpoints.contains(s)
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

/// GH #285 — the addresses this plan's hives declared as SLOTS, absolute.
///
/// A slot is a hive saying "here is an address a parent may wire, and it may
/// stand empty". [`unresolved_boot_endpoints`] takes the result as its second
/// known-set: the declaration is what buys the exemption, so an endpoint that
/// merely LOOKS like an unbuilt child — a typo, a plain port whose child is
/// missing — keeps dangling.
///
/// Reads each hive's own `config.json` through
/// [`collect_sealed_hives`](crate::mutation::port_boundary::collect_sealed_hives),
/// the one reader that decides what a port name denotes (GH #196), so a slot
/// findable here under a spelling the boundary seals shut cannot happen. A hive
/// whose config is missing or unparsable contributes nothing — the same silence
/// that reader already keeps, and the boot reports such a tree on its own.
#[must_use]
pub fn declared_slot_endpoints(
    root: &std::path::Path,
    plan: &BootstrapPlan,
) -> std::collections::HashSet<String> {
    let sealed = crate::mutation::port_boundary::collect_sealed_hives(
        root,
        plan.hives.iter().map(|h| &h.path),
    );
    // The address rule itself lives next to the reader, because the mutation
    // edge check (GH #285, second half) asks the SAME question of the hives the
    // colony is running — and two spellings of "hive path plus one segment" is
    // how a slot becomes wireable at boot and unknown at mutation time.
    crate::mutation::port_boundary::slot_endpoint_addresses(&sealed)
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
/// GH #424 — the templates registry a boot-time growth resolves against.
///
/// Read-only, straight out of `{root}/colony.db`'s `templates` table — the same
/// `TemplatesRegistry::from_entries` conversion the colony does at its five
/// other sites.
///
/// ORDER MATTERS, and it holds in production: `meclaw-cli` runs
/// `templates::boot_load_or_scan` BEFORE it calls
/// `bootstrap_from_filesystem_with_env`, so the table is filled by the time a
/// growth asks. A test that calls the bootstrap without a prior scan sees an
/// EMPTY registry — which is not a silent failure but a `template_missing` that
/// names `none`, the right answer to "resolve this against nothing".
fn boot_templates_registry(root: &std::path::Path) -> crate::templates::TemplatesRegistry {
    let Ok(db) = crate::persist::colony_db::ColonyDb::open(&root.join("colony.db")) else {
        return crate::templates::TemplatesRegistry::default();
    };
    let rows = db.read_templates().unwrap_or_default();
    crate::templates::TemplatesRegistry::from_entries(
        rows.into_iter()
            .map(|r| crate::templates::TemplateEntry {
                template_id: r.template_id,
                name: r.name,
                version: r.version,
                filesystem_path: std::path::PathBuf::from(r.filesystem_path),
            })
            .collect(),
    )
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
    let mut plan = crate::bootstrap::plan_bootstrap_with_env(
        root,
        factories,
        &overlay,
        boot_state.clone(),
        env_path,
    )?;
    // GH #424: a FIRST boot fulfils its declarations BEFORE it applies
    // anything.
    //
    // Before the apply, deliberately: if the tree grew afterwards, the colony
    // would have spawned half a tree and the activity derivation (A7) would
    // have reasoned over a topology that is about to stop existing.
    //
    // The plan above described a tree that still had markers in it, so it is
    // thrown away and re-planned over what now stands. Idempotent by
    // construction: a grown marker is no longer a marker, so the next pass
    // plans no growth at all — which is also the loop's termination argument.
    // Each pass consumes at least one marker, so the number of passes is
    // bounded by the number of markers the first pass found, plus one to
    // discover there is nothing left. A pass that consumes none while markers
    // remain is a defect, and it is named rather than spun on.
    if !plan.growths.is_empty() {
        let templates = boot_templates_registry(root);
        let budget = plan.growths.len() + 1;
        for pass in 0..=budget {
            if plan.growths.is_empty() {
                break;
            }
            if pass == budget {
                let mut errors = BootstrapErrors::new();
                for g in &plan.growths {
                    errors.push(crate::bootstrap::BootstrapError::GrowthFailed {
                        path: g.fs_path.clone(),
                        reference: g.reference.clone(),
                        reason: "growth did not converge: the marker survived its own growth"
                            .to_string(),
                    });
                }
                return Err(errors);
            }
            crate::bootstrap_grow::grow_planned_refs(
                root,
                &plan.growths,
                &templates,
                factories,
                env_path,
            )?;
            plan = crate::bootstrap::plan_bootstrap_with_env(
                root,
                factories,
                &overlay,
                boot_state.clone(),
                env_path,
            )?;
        }
    }
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
    // GH #178: header-contract violations found in the topology this reboot will
    // actually run. Committed state, so the boot proceeds — but it says WHAT it
    // found and WHERE, because a finding an operator cannot see is a finding
    // that costs a restart to discover. `--validate --validate-strict` is the
    // pre-flight surface that turns the same list into a non-zero exit.
    for finding in &plan.header_contract_findings {
        tracing::warn!(
            finding = %finding,
            "reboot found a header-contract violation in the persisted topology — the \
             colony starts, the obligation is NOT satisfied; re-wire it or relax the \
             contract (`meclaw --validate --validate-strict` lists these before a boot)"
        );
    }
    // GH #283 (ruling Q1 2026-08-21): the plan's advisories — hints about the
    // topology this boot will run, said out loud and nothing more. Warned
    // beside the two channels above and NOT promoted anywhere: an unguarded
    // default is a legal topology, and the boot that finds one starts.
    for advisory in &plan.advisories {
        tracing::warn!(
            advisory = %advisory,
            "topology advisory — the colony starts; this is a hint about a shape that is \
             usually not what the author meant, not a defect"
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
    // GH #168: a hive has no registry row, so the overlay cannot vouch for one
    // whose directory is gone while its edges live on in the table. The
    // `hive_scopes` table is the hive half of the same "this colony registered
    // it" answer, and a reboot now plans the persisted edges — so it belongs in
    // the resolvable universe for exactly the reason the overlay does.
    let mut registry_paths = snapshot_registry_paths(runtime).await;
    registry_paths.extend(overlay.keys().map(|p| p.as_str().to_string()));
    registry_paths.extend(crate::bootstrap::registered_hive_paths(root));
    // GH #285: the third universe, and the only one that is not a node — a hive
    // that declared a slot invited exactly this edge, and an address that may
    // stand empty is the one thing the check above cannot infer from a tree.
    let slot_endpoints = declared_slot_endpoints(root, &plan);
    let unresolved = unresolved_boot_endpoints(&plan, &registry_paths, &slot_endpoints);
    if !unresolved.is_empty() {
        let mut errors = BootstrapErrors::new();
        for (edge_id, endpoint) in unresolved {
            errors.push(crate::bootstrap::BootstrapError::DanglingEndpoint { edge_id, endpoint });
        }
        return Err(errors);
    }
    let report = apply_bootstrap_plan(plan, factories, runtime).await;
    warn_on_declared_hive_rules_after_boot(root, runtime).await;
    Ok(report)
}

/// GH #147 and GH #173, the boot half: say it out loud when a hive's own
/// declarations no longer match the topology it woke up with — a paired drain
/// that nobody consumes, a contract lane with no door behind it.
///
/// A warning, never a refusal — the mutation path is where this rule bites,
/// because that is somebody changing a colony they did not necessarily build.
/// The birth topology is authorship, the same reason the port boundary leaves
/// the bootstrap alone (GH #133).
///
/// Runs after apply, so it sees the topology the colony actually woke up with —
/// including the edges rehydrated from `colony.db`, which the plan does not
/// carry.
async fn warn_on_declared_hive_rules_after_boot(root: &std::path::Path, runtime: &ColonyRuntime) {
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
    // GH #173: the same graph answers the second question — does every lane a
    // hive promises still have a door? One ReadGraph, two declarations checked.
    //
    // GH #176: the contract half needs the edge's `modifier`. A hive's failure
    // exit recognises something interior and STAMPS the lane, and a check that
    // only sees conditions cannot tell that door from a missing one. GH #237
    // gave the drain half the same need — the lane form's trigger IS a
    // caller's `set_hop.route` — so both halves read one and the same edge
    // view.
    let contracts = crate::mutation::hive_contract::collect_hive_contracts(root, hive_paths.iter());
    let contract_edges = boot_edges_from_graph(&graph);
    crate::mutation::required_drains::warn_on_missing_drains(&reqs, &contract_edges);
    crate::mutation::hive_contract::warn_on_broken_contracts(&contracts, &contract_edges);
}

/// Project a `/colony/graph` answer into the edge view both boot probes read.
///
/// GH #283 widened the tuple by a fifth term, the routing phase, and GH #367
/// filled it: `GraphEdgeDto` now names the phase, so the table these probes
/// rebuild is the table the colony routes on. Before that the term was a
/// literal `false` for every edge, which put a default edge into phase one —
/// where it fires BESIDE the regular arms instead of after them — and both
/// checks judged a topology that does not exist.
pub fn boot_edges_from_graph(
    graph: &crate::api_dto::ReadGraphReply,
) -> Vec<crate::mutation::hive_contract::BootEdge> {
    graph
        .edges
        .iter()
        .map(|e| {
            (
                e.from.clone(),
                e.to.clone(),
                e.condition.clone(),
                e.modifier.clone(),
                e.is_default,
            )
        })
        .collect()
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
            is_default: false,
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
            growths: vec![],
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/a"), make_cell("/b")],
            edges: vec![make_edge("/a", "/b")],
            unregistered_nodes: vec![],
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let d = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert!(d.is_empty(), "no dangling endpoints expected, got {d:?}");
    }

    /// An edge whose `to` is not in cells or hives is reported as dangling.
    #[test]
    fn dangling_edge_endpoints_reports_unknown_to() {
        let plan = BootstrapPlan {
            growths: vec![],
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/a")],
            // /sink is a registry-only cell (h.spawn), not in the FS plan.
            edges: vec![make_edge("/a", "/sink")],
            unregistered_nodes: vec![],
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let d = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert_eq!(d.len(), 1, "one dangling endpoint expected, got {d:?}");
        assert_eq!(d[0].1.as_str(), "/sink");
    }

    /// An edge whose `from` is not known is also reported.
    #[test]
    fn dangling_edge_endpoints_reports_unknown_from() {
        let plan = BootstrapPlan {
            growths: vec![],
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![make_cell("/b")],
            edges: vec![make_edge("/ghost", "/b")],
            unregistered_nodes: vec![],
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let d = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert_eq!(d.len(), 1, "one dangling endpoint expected, got {d:?}");
        assert_eq!(d[0].1.as_str(), "/ghost");
    }

    /// Both endpoints unknown → two entries.
    #[test]
    fn dangling_edge_endpoints_reports_both_unknown() {
        let plan = BootstrapPlan {
            growths: vec![],
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![],
            edges: vec![make_edge("/x", "/y")],
            unregistered_nodes: vec![],
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let d = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert_eq!(d.len(), 2, "two dangling endpoints expected, got {d:?}");
    }

    /// A hive endpoint (known in `plan.hives`) is NOT dangling.
    #[test]
    fn dangling_edge_endpoints_hive_endpoint_not_dangling() {
        let plan = BootstrapPlan {
            growths: vec![],
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
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let d = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
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
            growths: vec![],
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
            header_contract_findings: vec![],
            advisories: vec![],
        };
        let mut registry: HashSet<String> = HashSet::new();
        registry.insert("/sink".to_string());
        let u = super::unresolved_boot_endpoints(&plan, &registry, &HashSet::new());
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
        let dangling = super::unresolved_boot_endpoints(
            &plan,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
        );
        assert!(
            dangling.iter().any(|(_, ep)| ep.as_str() == "/sink"),
            "/sink must be identified as dangling, got {dangling:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn apply_via_runtime_registers_hive_count() {
        let plan = BootstrapPlan {
            growths: vec![],
            hives: vec![PlannedHive {
                path: Path::new("/"),
            }],
            cells: vec![],
            edges: vec![],
            unregistered_nodes: vec![],
            header_contract_findings: vec![],
            advisories: vec![],
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
