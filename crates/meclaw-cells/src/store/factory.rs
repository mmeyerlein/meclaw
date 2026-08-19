//! Phase-9 StoreCellFactory: open_or_create_cell_db_with_status → DDL
//! → seed-if-fresh → DbConn::wrap → cell_task_stateful.

use crate::store::{StoreCell, StoreParams, ddl, seed};
use meclaw_colony::persist::{OpenStatus, open_or_create_cell_db_with_status};
use meclaw_colony::{
    CellFactory, DbConn, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
    renotify_stop_wiring,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Wrap a `store` connection for the cell task.
///
/// P4: `hamming` is bound to the CONNECTION, so it is registered wherever a
/// store connection is born. Issue #59 added one more birth place that is not
/// a factory path: `DbConn` re-opens a connection lost to a dropped call future
/// on the next access, and it knows nothing about store specifics. So the
/// registration is handed to it as re-open setup — otherwise the re-opened
/// connection would serve every op except `similar`.
///
/// 0.2.0 P3 put the `meclaw_stem` FTS5 tokenizer into the same hook, where it
/// matters more: a connection that does not know the tokenizer cannot open an
/// index declaring it at all, so a re-open without it would take out `search`
/// entirely rather than one op.
fn wrap_store_db(conn: rusqlite::Connection, query_timeout: Option<std::time::Duration>) -> DbConn {
    DbConn::wrap(conn, query_timeout)
        .with_reopen_setup(crate::store::query::install_connection_extensions)
}

/// Phase-9 `store` cell factory. Unit struct (no fields) — all
/// per-instance config lives in `params`.
pub struct StoreCellFactory;

impl CellFactory for StoreCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    /// Pre-spawn validation. Same parse path as `spawn_cell`.
    fn validate_params(&self, raw: &JsonValue) -> Result<(), String> {
        StoreParams::parse(raw).map(|_| ())
    }

    /// Issue #56: the on-disk half of the pre-spawn validation — `seed/<table>
    /// .jsonl` is statically parseable configuration, so the bootstrap plan
    /// phase (and therefore `meclaw --validate [--validate-strict]`) parses it here
    /// instead of letting the error strike at the cell's first wake.
    ///
    /// Same parse path as `spawn_cell` and the `WakeFn` loader
    /// (`seed::check_seed_files` → `parse_seed_file` ← `load_seed_if_present`),
    /// which is what keeps validate-equals-spawn honest.
    fn validate_cell_dir(&self, raw: &JsonValue, cell_dir: &std::path::Path) -> Result<(), String> {
        let params = StoreParams::parse(raw)?;
        seed::check_seed_files(cell_dir, &params.schema)
    }

    /// Spawn a `store` cell instance.
    ///
    /// Sequence (per brainstorm E1+E3+E4):
    /// 1. Parse params (`StoreParams::parse`), then statically parse the seed
    ///    files (`seed::check_seed_files`, issue #56) — a broken seed fails
    ///    THIS CELL here instead of panicking at wake.
    /// 2. `open_or_create_cell_db_with_status` → `(Connection, OpenStatus)`,
    ///    then `hamming::register` (P4: the scalar function is per connection,
    ///    so both the wake and the respawn path install it).
    /// 3. `apply_schema_ddl` (ad-hoc DDL from `params.schema`, sync), then
    ///    `apply_fts_ddl` (P3: FTS5 index + triggers from `params.fts`, idempotent).
    /// 4. If `OpenStatus::Created` → `load_seed_if_present` (sync, fresh-only).
    /// 5. `DbConn::wrap` (with optional `query_timeout` from params).
    /// 6. `tokio::spawn(cell_task_stateful(...))`.
    ///
    /// `RespawnFn` replays steps 2/3/5/6 (NEVER step 4 — fresh-only seed,
    /// brainstorm E4). DDL is idempotent (`CREATE TABLE IF NOT EXISTS`).
    ///
    /// Issue #57 — the `WakeFn` runs steps 2–6 INSIDE the colony task, so no step
    /// there may panic. Step 2's connection is the hard class (no DB → no cell):
    /// the wake starts the cell degraded via [`wake_degraded`]. Everything else is
    /// soft: `hamming::register`, the params-overlay restore, `apply_schema_ddl`,
    /// `apply_fts_ddl` and the seed each log loudly and the cell starts without
    /// that one feature.
    ///
    /// Issue #63 — the `RespawnFn` carries the identical class: it runs at the
    /// `handle_cell_died` call site, likewise inside the colony task. Same split,
    /// mirrored: hard → [`respawn_degraded`], soft → the shared
    /// [`effective_params_or_degrade`] strip.
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
        // Spawn-time validate (parser-invariant). The effective params are
        // rebuilt from the cell.db overlay at each wake/respawn (β restore);
        // the closures capture the BIRTH params Value (config.json snapshot).
        let params = StoreParams::parse(&raw_params)?;

        // Issue #56: the seed files are parsed HERE, on the spawn path, so a
        // syntactically broken seed fails THIS CELL with a named factory error
        // (handled like every other factory error: boot plan reject, mutation
        // reject) instead of panicking later inside the colony task's wake.
        // `schema` is an IMMUTABLE overlay key, so the birth schema checked
        // here is the same schema the WakeFn seeds against.
        seed::check_seed_files(&cell_dir, &params.schema).map_err(|e| format!("store: {e}"))?;

        // GH #132: `write_surface: "internal"` bounds writes to the store's own
        // parent scope. A store directly under the colony root has `/` as its
        // parent, and `/` contains every cell — the declaration is then inert.
        // Not a spawn error (the check needs the PATH, which `validate_params`
        // never sees, and a spawn-only reject would break the validate ≡ spawn
        // invariant), but never silent either: a boundary that quietly bounds
        // nothing is worse than none.
        if params.write_surface == crate::store::WriteSurface::Internal
            && path.as_str().matches('/').count() <= 1
        {
            tracing::warn!(
                path = path.as_str(),
                "store: params.write_surface \"internal\" has no effect here — this store sits \
                 directly under the colony root, so its owning scope is `/` and every cell is \
                 inside it. Move the store into the hive that owns its data."
            );
        }

        // Phase-13-K-2: NO initial cell-task spawn — the mailbox pair goes to
        // Dormant. Seed-on-Created stays correct: WakeFn calls
        // `open_or_create_cell_db_with_status`, the first wake against an empty
        // `cell.db` gives `OpenStatus::Created` → the seed runs; every later wake
        // gives `Resumed` → no re-seed. RespawnFn skips the seed intentionally
        // (a restart is not fresh; doing otherwise would violate the M1 spec).
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — crash-restart path. Connection re-open, DDL reapply
        // (idempotent), Seed-Skip (Resume ≠ fresh).
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn_cell_dir = cell_dir.clone();
        let respawn_path = path.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_birth = raw_params.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        // GH #260: the substrate half of the write boundary travels with them.
        let respawn_write_surface = contract.write_surface;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                // Issue #63: NOTHING on this path may panic either. The closure
                // runs synchronously inside the colony task — inside the
                // await-free restart barrier of `handle_cell_died` at that — so a
                // panic here does not fail one cell, it takes the colony task and
                // with it every cell in the process (the panic-free colony hot
                // path invariant, A1′ class). Same two classes as the WakeFn:
                //   HARD (no `cell.db`) → restart DEGRADED (`respawn_degraded`).
                //   SOFT (one feature could not be installed) → loud log +
                //     continue without that feature (`effective_params_or_degrade`).
                let (conn, _status) =
                    match open_or_create_cell_db_with_status(&respawn_cell_dir.join("cell.db")) {
                        Ok(pair) => pair,
                        Err(e) => {
                            tracing::error!(
                                path = respawn_path.as_str(),
                                error = %e,
                                "store: cell.db could not be re-opened at respawn — the cell \
                                 restarts DEGRADED and answers every message with an error \
                                 (the colony is kept alive; NO in-memory substitute DB is \
                                 installed, so no write is silently lost)"
                            );
                            return respawn_degraded(
                                respawn_path.clone(),
                                respawn_mailbox_capacity,
                                respawn_outputs.clone(),
                                respawn_inbox_tx.clone(),
                                message_timeout,
                                respawn_blob.clone(),
                                format!("cell.db could not be re-opened at respawn: {e}"),
                            );
                        }
                    };
                // P4 hamming registration + β restore + schema/FTS DDL, all
                // soft-failing — shared with the WakeFn so both paths degrade
                // identically. NEVER the seed: a restart is not fresh (E4).
                let effective = match effective_params_or_degrade(
                    &conn,
                    &respawn_birth,
                    &respawn_path,
                    "respawn",
                ) {
                    Ok(effective) => effective,
                    Err(reason) => {
                        return respawn_degraded(
                            respawn_path.clone(),
                            respawn_mailbox_capacity,
                            respawn_outputs.clone(),
                            respawn_inbox_tx.clone(),
                            message_timeout,
                            respawn_blob.clone(),
                            reason,
                        );
                    }
                };
                let to = effective
                    .query_timeout_ms
                    .map(std::time::Duration::from_millis);
                let db = wrap_store_db(conn, to);
                let cell = StoreCell::new(effective);
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
                    respawn_write_surface,
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

        // WakeFn — Open + DDL (idempotent) + Seed-on-Created + Spawn + Watcher.
        let wake_cell_dir = cell_dir.clone();
        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_birth = raw_params.clone();
        let wake_inbox_tx = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let wake_consumes = contract.consumes.clone();
        // GH #260: the substrate half of the write boundary travels with them.
        let wake_write_surface = contract.write_surface;
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            // Issue #57: NOTHING on this path may panic. The closure runs
            // synchronously inside the colony task, so a panic here does not
            // fail one cell — it takes the colony task and with it every cell in
            // the process (the panic-free colony hot path invariant, A1′ class).
            // Two failure classes, two treatments:
            //   HARD (no `cell.db`) → start DEGRADED (`wake_degraded`): the cell
            //     answers every message with a named error. No in-memory
            //     substitute DB — that would mask write loss.
            //   SOFT (one feature could not be installed) → loud log + continue
            //     without that feature, exactly like the #56 seed treatment.
            let (conn, status) =
                match open_or_create_cell_db_with_status(&wake_cell_dir.join("cell.db")) {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::error!(
                            path = wake_path.as_str(),
                            error = %e,
                            "store: cell.db could not be opened at wake — the cell starts \
                             DEGRADED and answers every message with an error (the colony \
                             is kept alive; NO in-memory substitute DB is installed, so no \
                             write is silently lost)"
                        );
                        return wake_degraded(
                            wake_path.clone(),
                            recv,
                            wake_outputs.clone(),
                            wake_inbox_tx.clone(),
                            message_timeout,
                            wake_blob.clone(),
                            format!("cell.db could not be opened at wake: {e}"),
                        );
                    }
                };
            // P4 hamming registration + β restore + schema/FTS DDL, all
            // soft-failing — shared with the RespawnFn (#63) so both paths degrade
            // identically. The FTS DDL runs BEFORE the seed below, so seeded rows
            // are indexed by the triggers on the way in.
            let effective =
                match effective_params_or_degrade(&conn, &wake_birth, &wake_path, "wake") {
                    Ok(effective) => effective,
                    Err(reason) => {
                        return wake_degraded(
                            wake_path.clone(),
                            recv,
                            wake_outputs.clone(),
                            wake_inbox_tx.clone(),
                            message_timeout,
                            wake_blob.clone(),
                            reason,
                        );
                    }
                };
            if status == OpenStatus::Created
                && let Err(e) = seed::load_seed_if_present(&conn, &wake_cell_dir, &effective.schema)
            {
                // Issue #56: this closure runs INSIDE the colony task (the
                // routing/dispatch path wakes a parked cell synchronously), so
                // a panic here does not fail one cell, it takes the whole
                // colony down (the panic-free colony hot path invariant,
                // A1′ class). Every statically detectable seed defect is
                // already rejected by `validate_cell_dir` (bootstrap plan /
                // `--validate`) and by `check_seed_files` on the spawn path
                // above, so reaching this arm means the seed file changed on
                // disk after the cell was spawned, or an INSERT was rejected.
                // Report loudly and continue with an unseeded (but
                // schema-correct) table rather than killing the colony.
                tracing::error!(
                    path = wake_path.as_str(),
                    error = %e,
                    "store: seed load failed at wake — the cell starts WITHOUT its \
                     seed rows (the colony is kept alive; fix the seed file and \
                     re-create the cell.db)"
                );
            }
            let to = effective
                .query_timeout_ms
                .map(std::time::Duration::from_millis);
            let db = wrap_store_db(conn, to);
            let cell = StoreCell::new(effective);
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
                    wake_write_surface,
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
        // Dormant (lazy-wake) variant — these belong to the PRE-wake state
        // (NotYetSpawned). They are inert (their counterparts are dropped here):
        // a disconnect BEFORE wake just flips active=false. After wake, the
        // colony overwrites the registry's stop wiring with the live pair the
        // WakeFn returns (Task 7.5).
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

/// Issues #57/#63: the soft-failing strip both the `WakeFn` and the `RespawnFn`
/// run between "a `cell.db` is open" and "the cell task is built" — the P4
/// `hamming` registration, the β params-overlay restore, the schema DDL and the
/// FTS DDL.
///
/// NOTHING here may panic. The `WakeFn` runs synchronously inside the colony
/// task's routing/dispatch path and the `RespawnFn` inside its await-free restart
/// barrier, so a panic on either path does not fail one cell — it takes the
/// colony task and with it every cell in the process (the panic-free colony hot
/// path invariant, A1′ class). Each step therefore logs loudly and the cell
/// continues WITHOUT that one feature; none of them is worth a process.
///
/// `Err(reason)` is the single unrecoverable outcome: the overlay restore failed
/// AND the birth params no longer parse, so there is no `StoreParams` to build a
/// cell from at all. The reason travels back as a string because the two callers
/// start their degraded cell in different shapes ([`wake_degraded`] returns the
/// wake wiring, [`respawn_degraded`] the `RespawnFn` 4-tuple).
///
/// `phase` (`"wake"` / `"respawn"`) names the trigger strip in the log lines —
/// the two paths are otherwise indistinguishable to an operator reading the
/// journal.
fn effective_params_or_degrade(
    conn: &rusqlite::Connection,
    birth: &JsonValue,
    path: &Path,
    phase: &str,
) -> Result<StoreParams, String> {
    // P4: `hamming` is bound to the CONNECTION, so every wake and every respawn
    // builds a new connection and has to re-register the function.
    if let Err(e) = crate::store::query::hamming::register(conn) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: hamming registration failed at {phase} — the cell starts WITHOUT \
             vector similarity (`similar` ops answer with a SQL error); every other \
             op is unaffected"
        );
    }
    // 0.2.0 P4: same connection-scoped story for `meclaw_norm`, and it has to
    // happen BEFORE the canonical DDL below — the backfill of a normalising
    // binding computes the normal form inside its UPDATE, so without the function
    // the derived column stays NULL and every identity-sensitive read goes blind.
    if let Err(e) = crate::store::query::normalize::register(conn) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: meclaw_norm registration failed at {phase} — a normalising canonical \
             binding cannot derive its column (`canonicalize` and the boot backfill answer \
             with a SQL error); every other op is unaffected"
        );
    }
    // 0.2.0 P3: same connection-scoped story for the stemming FTS5 tokenizer,
    // and it has to happen BEFORE the FTS DDL below — that DDL reads and writes
    // an index which declares the tokenizer by name.
    if let Err(e) = crate::store::query::fts_tokenizer::register(conn) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: FTS tokenizer registration failed at {phase} — the cell starts \
             WITHOUT its full-text index (an index declaring `meclaw_stem` cannot be \
             opened without it, so `search` ops answer with a SQL error); every other \
             op is unaffected"
        );
    }
    // β restore: replay the cell.db params-overlay over birth-params.
    let effective = match crate::params_overlay::restore::<StoreParams>(conn, birth) {
        Ok(effective) => effective,
        Err(e) => {
            tracing::error!(
                path = path.as_str(),
                error = %e,
                "store: the cell.db params overlay could not be replayed at {phase} — \
                 the cell starts on its BIRTH params (config.json) and every runtime \
                 params update is lost"
            );
            // The birth params parsed cleanly at spawn time, so this is an
            // unreachable second failure — handled anyway, because an `.expect`
            // here would be the very panic these issues remove.
            match StoreParams::parse(birth) {
                Ok(birth_params) => birth_params,
                Err(e2) => {
                    tracing::error!(
                        path = path.as_str(),
                        error = %e2,
                        "store: the birth params no longer parse at {phase} — the cell \
                         starts DEGRADED"
                    );
                    return Err(format!("params unusable at {phase}: {e2}"));
                }
            }
        }
    };
    if let Err(e) = ddl::apply_schema_ddl(conn, &effective.schema) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: schema DDL failed at {phase} — declared tables may be MISSING; ops \
             against them answer with `unknown_table` (the colony is kept alive; fix \
             the cell.db and re-wake the cell)"
        );
    }
    // Order is load-bearing: the alias table and the backfill run BEFORE the FTS
    // declaration, so the index is built over an already-derived canonical column
    // rather than over NULLs.
    if let Err(e) = ddl::apply_canonical_ddl(conn, &effective.canonical) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: canonical DDL failed at {phase} — the alias table or the derived column \
             may be MISSING; identity-sensitive reads fall back to whatever the column \
             carries (the colony is kept alive; fix the cell.db and re-wake the cell)"
        );
    }
    if let Err(e) = ddl::apply_fts_ddl(conn, &effective.fts, &effective.canonical) {
        tracing::error!(
            path = path.as_str(),
            error = %e,
            "store: FTS DDL failed at {phase} — the cell starts WITHOUT its full-text \
             index (`search` ops answer with `unknown_table`); every other op is \
             unaffected"
        );
    }
    Ok(effective)
}

/// Issue #63: build the DEGRADED replacement for a `store` cell whose crash-restart
/// could not produce a usable `cell.db`, in the shape the `RespawnFn` must return.
///
/// The respawn counterpart of [`wake_degraded`], and it differs in exactly two
/// mechanical points, both dictated by the call site (`handle_cell_died`):
/// - it mints the fresh mailbox pair itself (the colony hands the respawn no
///   receiver, unlike the wake) and returns the sender inside the frozen 4-tuple
///   `(sender, join, peace_rx, backstop_rx)`;
/// - it does NOT call `spawn_watcher` — `handle_cell_died` wires the watcher from
///   the returned `join`/`peace_rx`/`backstop_rx` itself. The `(stop_tx,
///   death_ack_rx)` pair cannot ride that tuple, so it goes back through
///   [`renotify_stop_wiring`], exactly like the healthy respawn path.
///
/// Everything else is the #57 rationale verbatim: a degraded cell answers every
/// message with a named error rather than installing an in-memory substitute
/// database (which would make writes look accepted and lose them at the next
/// restart), and `consumes` is deliberately NOT enforced so the database defect is
/// what comes back, not a contract verdict hiding it.
fn respawn_degraded(
    path: Path,
    mailbox_capacity: usize,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    message_timeout: Option<std::time::Duration>,
    blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    reason: String,
) -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);
    let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = meclaw_colony::build_stateless_task(
        path.clone(),
        receiver,
        outputs_tx,
        Arc::new(crate::store::DegradedStoreCell::new(reason)),
        1, // one worker: the answer is a single push, ordering costs nothing
        message_timeout,
        Some(colony_inbox_tx.clone()),
        blob_store,
        None,
    );
    // Same re-notify as the healthy respawn: sync `try_send`, never await — this
    // runs inside the await-free respawn corridor.
    renotify_stop_wiring(&colony_inbox_tx, path, stop_tx, death_ack_rx);
    (sender, join, peace_rx, backstop_rx)
}

/// Issue #57: build the DEGRADED replacement for a `store` cell whose wake could
/// not produce a usable `cell.db`, and return the wake wiring the colony expects.
///
/// `WakeFn` must hand back a live `(stop_tx, death_ack_rx)` pair — the colony has
/// already flipped the registry entry to `Awake` when it calls the closure — so
/// "give up" is not an available answer. What IS available: spawn a cell that
/// owns the mailbox and answers every message with a named error
/// ([`crate::store::DegradedStoreCell`]). The stateless dispatcher is the right
/// carrier because it needs no `DbConn` — the very thing that is missing.
///
/// The watcher is wired exactly as on the healthy path. A degraded cell can only
/// ever die `Normal` (it never panics, and the stateless backstop is per-worker),
/// so this never drives `handle_cell_died` into a respawn.
///
/// `consumes` is deliberately NOT enforced for the degraded cell: every message
/// must come back with the database defect, not with a contract verdict that
/// hides it.
fn wake_degraded(
    path: Path,
    receiver: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
    message_timeout: Option<std::time::Duration>,
    blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    reason: String,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = meclaw_colony::build_stateless_task(
        path.clone(),
        receiver,
        outputs_tx,
        Arc::new(crate::store::DegradedStoreCell::new(reason)),
        1, // one worker: the answer is a single push, ordering costs nothing
        message_timeout,
        Some(colony_inbox_tx.clone()),
        blob_store,
        None,
    );
    meclaw_colony::spawn_watcher(&colony_inbox_tx, path, join, peace_rx, backstop_rx);
    (stop_tx, death_ack_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::CellFactory;
    use meclaw_core::Path;
    use std::sync::Arc;

    /// Issue #59 companion of the P4 registration: `hamming` is bound to the
    /// CONNECTION, and `DbConn` reopens a connection lost to a dropped call
    /// future. The reopened connection has to carry the function again —
    /// otherwise the cell keeps serving every op except `similar`, which starts
    /// answering with a plain SQL error until the next respawn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_db_reopen_reregisters_hamming() {
        let td = tempfile::TempDir::new().unwrap();
        let (conn, _status) =
            open_or_create_cell_db_with_status(&td.path().join("cell.db")).unwrap();
        crate::store::query::hamming::register(&conn).unwrap();
        let mut db = wrap_store_db(conn, None);
        // Drop the call future while its closure is parked: the connection goes
        // down with the orphaned closure.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            db.call(move |_c| {
                let _ = release_rx.recv();
            }),
        )
        .await;
        assert!(
            elapsed.is_err(),
            "the call has to be still in flight when the timeout drops its future"
        );
        release_tx.send(()).unwrap();
        // 30s is a generous failure marker, not a semantic discriminator.
        let bits = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            db.call(|c| {
                c.query_row::<i64, _, _>("SELECT hamming('/w==','AA==')", [], |r| r.get(0))
            }),
        )
        .await
        .expect("the reopened connection must be usable");
        assert_eq!(
            bits.unwrap(),
            8,
            "`hamming` must be re-registered on the reopened connection"
        );
    }

    /// 0.2.0 P3: the same #59 property for the FTS5 tokenizer, and it bites
    /// harder than `hamming` does. A connection without `meclaw_stem` cannot
    /// even OPEN an index that declares it — FTS5 resolves the tokenizer name
    /// when the virtual table is opened — so a re-opened connection that lost
    /// the registration would answer every `search` with a tokenizer error
    /// instead of merely losing one op.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_db_reopen_reregisters_the_stemming_tokenizer() {
        let td = tempfile::TempDir::new().unwrap();
        let (conn, _status) =
            open_or_create_cell_db_with_status(&td.path().join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(body, tokenize='meclaw_stem');
             INSERT INTO t(body) VALUES ('hat lieblingseditor helix');",
        )
        .unwrap();
        let mut db = wrap_store_db(conn, None);
        // Drop the call future while its closure is parked: the connection goes
        // down with the orphaned closure.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            db.call(move |_c| {
                let _ = release_rx.recv();
            }),
        )
        .await;
        assert!(
            elapsed.is_err(),
            "the call has to be still in flight when the timeout drops its future"
        );
        release_tx.send(()).unwrap();
        // 30s is a generous failure marker, not a semantic discriminator.
        let hits = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            db.call(|c| {
                c.query_row::<i64, _, _>(
                    "SELECT count(*) FROM t WHERE t MATCH '\"lieblingseditoren\"*'",
                    [],
                    |r| r.get(0),
                )
            }),
        )
        .await
        .expect("the reopened connection must be usable");
        assert_eq!(
            hits.unwrap(),
            1,
            "the reopened connection must know `meclaw_stem` — and fold the plural query \
             onto the singular row through it"
        );
    }

    /// Phase-13-K-2: factory returns `Dormant` — invoke the WakeFn directly to
    /// drive the open/DDL/seed pipeline, then verify the fresh `items` count.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_applies_ddl_and_seed_on_fresh() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
        std::fs::write(
            cell_dir.join("seed/items.jsonl"),
            r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
"#,
        )
        .unwrap();
        let raw = meclaw_core::serde_json::json!({
            "schema":{"items":{"id":"int","name":"text"}}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => {
                unreachable!("Phase-13-K-2: stateful Factory liefert Dormant")
            }
        };
        // Wake spawns cell_task_stateful with the supplied receiver. Dropping
        // the sender then closes the mailbox → task ends cleanly.
        wake(receiver);
        drop(sender);
        // Give the spawned task time to flush + close. Poll cell.db until the
        // seed row is visible (DDL + seed run inside the Wake closure synchronously
        // before the task is spawned, so the row is visible immediately).
        for _ in 0..50 {
            if let Ok(conn) = rusqlite::Connection::open(cell_dir.join("cell.db"))
                && let Ok(n) =
                    conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                && n == 1
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("seed not loaded on fresh within 500 ms");
    }

    /// 0.2.0 P2: a `cell.db` written by the PREVIOUS release must migrate itself on
    /// the next wake — no migration tool, no lost row, no loud failure.
    ///
    /// The starting state is the shipped P15 shape: `facts` without the derived
    /// column, an FTS index over `claim, predicate`, no alias table. Three things
    /// have to happen in one wake, and each of them fails on its own without the
    /// other two: the column is added (`ALTER TABLE`), it is backfilled from the
    /// originals, and the index is swapped onto it (canonical drift class — a
    /// same-length swap, which the additive rule would have refused).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_migrates_itself_onto_the_canonical_column() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            crate::store::query::install_connection_extensions(&conn).unwrap();
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, predicate TEXT, claim TEXT);
                 INSERT INTO facts VALUES ('f1','user:m','Lieblingseditor','Helix');
                 INSERT INTO facts VALUES ('f2','user:m','favorite_editor','vscode');",
            )
            .unwrap();
            crate::store::ddl::apply_fts_ddl(
                &conn,
                &std::collections::BTreeMap::from([(
                    "facts".to_string(),
                    vec!["claim".to_string(), "predicate".to_string()],
                )]),
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","predicate":"text",
                                 "canonical_predicate":"text","claim":"text"}},
            "canonical": {"facts": {"source":"predicate","target":"canonical_predicate",
                                    "aliases":"predicate_aliases"}},
            "fts": {"facts": ["claim", "canonical_predicate"]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        // Reading the FTS index needs the tokenizer, exactly as the running cell
        // does — an index declaring `meclaw_stem` cannot be opened without it.
        crate::store::query::install_connection_extensions(&conn).unwrap();
        let rows: Vec<(String, Option<String>)> = conn
            .prepare("SELECT predicate, canonical_predicate FROM facts ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    "Lieblingseditor".to_string(),
                    Some("Lieblingseditor".into())
                ),
                (
                    "favorite_editor".to_string(),
                    Some("favorite_editor".into())
                ),
            ],
            "every row keeps its original and gains a derived value"
        );
        let aliases: i64 = conn
            .query_row("SELECT count(*) FROM predicate_aliases", [], |r| r.get(0))
            .expect("the alias table must exist after the wake");
        assert_eq!(aliases, 0, "a migration writes no judgements");
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"favorite\"*'",
                [],
                |r| r.get(0),
            )
            .expect("the index must have been swapped onto the canonical column");
        assert_eq!(indexed, 1, "the keyword leg reads the canonical column");
    }

    /// Statement identity W2 (GitHub #13, ruling Q1 + ruling Q4): a `cell.db`
    /// written by the 0.2.0 release gains the THIRD identity dimension on the
    /// next wake, mechanically and without a migration tool.
    ///
    /// The starting state is the shipped 0.2.0 shape: `facts` carries the two
    /// entity dimensions and the written claim, and nothing else. One wake has to
    /// produce three effects, and the package is worthless without any one of
    /// them: `canonical_claim` and `closure_source` are added (`ALTER TABLE`),
    /// the claim dimension is backfilled from the written claim (byte identity is
    /// the day-one canonical value), and the alias plus refusal tables of the new
    /// dimension exist so the judged aliases have somewhere to land.
    ///
    /// Ruling Q4 calls the existing store contents worthless and the migration is
    /// built anyway: meclaw is a public framework and the pattern is a free
    /// generic declaration, so the path has to exist and be proven.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_migrates_itself_onto_the_claim_dimension() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            crate::store::query::install_connection_extensions(&conn).unwrap();
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, canonical_subject TEXT,
                                     predicate TEXT, canonical_predicate TEXT, claim TEXT,
                                     expired_at TEXT, superseded_by TEXT);
                 INSERT INTO facts VALUES
                   ('f1','user','user','practices','practices','yoga twice a week',NULL,NULL);
                 INSERT INTO facts VALUES
                   ('f2','user','user','practices','practices','yoga three times a week',
                    '2026-06-05T00:00:00Z','f1');",
            )
            .unwrap();
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","canonical_subject":"text",
                                 "predicate":"text","canonical_predicate":"text",
                                 "claim":"text","canonical_claim":"text",
                                 "expired_at":"text","superseded_by":"text",
                                 "closure_source":"text"}},
            "canonical": {"facts": [
                {"source":"predicate","target":"canonical_predicate",
                 "aliases":"predicate_aliases"},
                {"source":"subject","target":"canonical_subject",
                 "aliases":"subject_aliases","normalize":true},
                {"source":"claim","target":"canonical_claim",
                 "aliases":"claim_aliases","rejected":"claim_rejected_pairs"}
            ]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        let rows: Vec<(String, Option<String>, Option<String>)> = conn
            .prepare("SELECT claim, canonical_claim, closure_source FROM facts ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    "yoga twice a week".to_string(),
                    Some("yoga twice a week".into()),
                    None
                ),
                (
                    "yoga three times a week".to_string(),
                    Some("yoga three times a week".into()),
                    None
                ),
            ],
            "every row keeps its written claim and gains the byte-identical canonical one, \
             and the migration attributes no closure to anybody"
        );
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' \
                 AND name IN ('claim_aliases','claim_rejected_pairs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            tables, 2,
            "the judged claim aliases and their refusal log need somewhere to land"
        );
        let judged: i64 = conn
            .query_row("SELECT count(*) FROM claim_aliases", [], |r| r.get(0))
            .unwrap();
        assert_eq!(judged, 0, "a migration writes no judgements");
        let kept: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts WHERE expired_at = '2026-06-05T00:00:00Z' \
                 AND superseded_by = 'f1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            kept, 1,
            "the wake is MECHANICAL: it adds columns and derives identities, and it \
             touches no closure at all. Withdrawing the ones the new rule rejects is the \
             re-derive of the nightly round (ruling Q4), never a boot effect"
        );
    }

    /// Statement identity W5 (GitHub #13, ruling Q3 option C): a `cell.db`
    /// written before the judged cardinality table gains it on the next wake,
    /// with no migration tool and without a single fact being touched.
    ///
    /// The table is an ordinary declared table, so the additive path that adds a
    /// COLUMN adds it as a whole — that is the point of the receipt rather than a
    /// shortcut: the pattern is generic and W5 buys it by declaring, not by
    /// writing Rust. What has to be proven is that it lands EMPTY (a migration
    /// states no verdict about any predicate), that the four columns the ruling
    /// names exist, and that the facts of the old store come through unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_migrates_itself_onto_the_cardinality_table() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            crate::store::query::install_connection_extensions(&conn).unwrap();
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, canonical_subject TEXT,
                                     predicate TEXT, canonical_predicate TEXT, claim TEXT,
                                     canonical_claim TEXT);
                 INSERT INTO facts VALUES
                   ('f1','user','user','collects','collects','collects vinyl','collects vinyl');",
            )
            .unwrap();
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {
                "facts": {"id":"text","subject":"text","canonical_subject":"text",
                          "predicate":"text","canonical_predicate":"text",
                          "claim":"text","canonical_claim":"text"},
                "predicate_cardinality": {"canonical_predicate":"text","verdict":"text",
                                          "source":"text","decided_at":"text"}
            },
            "canonical": {"facts": [
                {"source":"predicate","target":"canonical_predicate",
                 "aliases":"predicate_aliases"},
                {"source":"claim","target":"canonical_claim",
                 "aliases":"claim_aliases","rejected":"claim_rejected_pairs"}
            ]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('predicate_cardinality') ORDER BY name")
            .expect("the cardinality table must exist after the wake")
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            columns,
            vec![
                "canonical_predicate".to_string(),
                "decided_at".into(),
                "source".into(),
                "verdict".into()
            ],
            "ruling Q3 names four columns and `source` is the mandatory one: \
             'why does this axis enumerate' has to be answerable from the data"
        );
        let verdicts: i64 = conn
            .query_row("SELECT count(*) FROM predicate_cardinality", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(verdicts, 0, "a migration writes no judgements");
        let facts: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts WHERE claim = 'collects vinyl'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(facts, 1, "the wake adds a table and touches no fact");
    }

    /// 0.2.0 P3 (issue #14, ruling Q6): a `cell.db` whose FTS index was built
    /// WITHOUT the stemming tokenizer migrates itself on the next wake — no
    /// migration tool, no lost row, no loud failure.
    ///
    /// The starting state is the shipped P2 shape: the columns are already the
    /// declared ones, only the tokenizer is missing. That is precisely the case
    /// neither existing drift class sees — the additive rule compares lists and
    /// finds them equal, the canonical rule finds nothing to substitute — so
    /// without the tokenizer class this store would keep an unstemmed index
    /// while every query arrived folded, which is worse than no package at all.
    ///
    /// The receipt is the issue's own sentence: before the wake the plural
    /// question scores zero against the singular fact, after it, one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_migrates_its_index_onto_the_stemming_tokenizer() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        let plural = "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH \
                      '\"lieblingseditoren\"*'";
        {
            // The pre-P3 index, written the way `apply_fts_ddl` wrote it before
            // this package — spelled out rather than produced by that function,
            // because that function now declares the tokenizer.
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, predicate TEXT,
                                     canonical_predicate TEXT, claim TEXT);
                 INSERT INTO facts VALUES ('f1','user:m','hat Lieblingseditor',
                                           'hat Lieblingseditor','Helix');
                 CREATE VIRTUAL TABLE facts_fts USING fts5(claim, canonical_predicate,
                     content='facts', content_rowid='rowid');
                 CREATE TRIGGER facts_fts_ai AFTER INSERT ON facts BEGIN
                   INSERT INTO facts_fts(rowid, claim, canonical_predicate)
                     VALUES (new.rowid, new.claim, new.canonical_predicate);
                 END;
                 INSERT INTO facts_fts(facts_fts) VALUES ('rebuild');",
            )
            .unwrap();
            let before: i64 = conn.query_row(plural, [], |r| r.get(0)).unwrap();
            assert_eq!(
                before, 0,
                "this is the live defect of issue #14: the fact is keyword-invisible \
                 for exactly the question it answers"
            );
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","predicate":"text",
                                 "canonical_predicate":"text","claim":"text"}},
            "canonical": {"facts": {"source":"predicate","target":"canonical_predicate",
                                    "aliases":"predicate_aliases"}},
            "fts": {"facts": ["claim", "canonical_predicate"]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        let after: i64 = conn
            .query_row(plural, [], |r| r.get(0))
            .expect("the index must be readable through the migrated tokenizer");
        assert_eq!(
            after, 1,
            "after the wake the plural question reaches the singular fact"
        );
        // The row itself was never touched — the fold lives in the index, the
        // truth in the table. This is where ruling Q2's byte-faithful entity
        // promise actually holds.
        let (predicate, claim): (String, String) = conn
            .query_row(
                "SELECT predicate, claim FROM facts WHERE id='f1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(predicate, "hat Lieblingseditor");
        assert_eq!(claim, "Helix");
        // The triggers were re-declared with the index: a row written AFTER the
        // migration is folded too. `DROP TABLE` leaves triggers behind, so the
        // rebuild alone would not prove this.
        conn.execute(
            "INSERT INTO facts VALUES ('f2','user:m','hat Katzen','hat Katzen','Minka')",
            [],
        )
        .unwrap();
        let fresh: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"katze\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fresh, 1, "the insert trigger indexes through the tokenizer");
    }

    /// 0.2.0 P4 (issue #23, ruling Q5): a `cell.db` that never saw P2 gains BOTH
    /// identity dimensions in one wake — no migration tool, no intermediate
    /// release, no lost row.
    ///
    /// The starting state is the shipped P15 shape, which is what a store in the
    /// field actually carries: no derived column at all, an index over `claim,
    /// predicate`, no alias table. The declared index is `claim,
    /// canonical_predicate, canonical_subject`, so the column list has to migrate
    /// through BOTH drift classes at once — first the canonical substitution
    /// (`predicate` → `canonical_predicate`), then the additive extension (the
    /// entity column appended). Either class alone refuses this list, which is a
    /// spawn failure rather than a migration.
    ///
    /// The second half is the automatic merge of ruling Q5: the two rows were
    /// written under two spellings of one subject, and the backfill puts them on
    /// ONE canonical subject without an alias, a judgement or a model.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_gains_the_entity_dimension_in_one_wake() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            crate::store::query::install_connection_extensions(&conn).unwrap();
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, predicate TEXT, claim TEXT);
                 INSERT INTO facts VALUES ('f1','User:U1','lives_in','Sonnenhof');
                 INSERT INTO facts VALUES ('f2','  user:u1 ','favorite_editor','Helix');",
            )
            .unwrap();
            crate::store::ddl::apply_fts_ddl(
                &conn,
                &std::collections::BTreeMap::from([(
                    "facts".to_string(),
                    vec!["claim".to_string(), "predicate".to_string()],
                )]),
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","canonical_subject":"text",
                                 "predicate":"text","canonical_predicate":"text","claim":"text"}},
            "canonical": {"facts": [
                {"source":"predicate","target":"canonical_predicate",
                 "aliases":"predicate_aliases"},
                {"source":"subject","target":"canonical_subject",
                 "aliases":"subject_aliases","normalize":true}]},
            "fts": {"facts": ["claim", "canonical_predicate", "canonical_subject"]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        let rows: Vec<(String, Option<String>)> = conn
            .prepare("SELECT subject, canonical_subject FROM facts ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("User:U1".to_string(), Some("user:u1".into())),
                ("  user:u1 ".to_string(), Some("user:u1".into())),
            ],
            "two spellings, one identity — and both originals byte-identical"
        );
        let aliases: i64 = conn
            .query_row("SELECT count(*) FROM subject_aliases", [], |r| r.get(0))
            .expect("the entity alias table must exist after the wake");
        assert_eq!(
            aliases, 0,
            "the merge came from normalisation, not a judgement"
        );
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM facts_fts WHERE facts_fts MATCH '\"user\"*'",
                [],
                |r| r.get(0),
            )
            .expect("the index must carry the entity column after the rebuild");
        assert_eq!(indexed, 2, "the keyword leg now sees the canonical subject");
    }

    /// 0.2.0 P5: a `cell.db` that already ran through P4 — derived columns, alias
    /// tables, judged aliases on disk — gains the REFUSAL log in one wake.
    ///
    /// This is the migration the nightly canonicalisation round needs before it
    /// runs for the first time: without the table `reject_pair` has nowhere to
    /// write and the GC re-judges the same pairs every night. Additive like every
    /// other step of this wave — no existing row is read, moved or rewritten, and
    /// the aliases the store already carried keep working untouched.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_existing_cell_db_gains_the_refusal_log_in_one_wake() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
            crate::store::query::install_connection_extensions(&conn).unwrap();
            // the P4 shape: both dimensions derived, both alias tables present,
            // one judgement already written, and no refusal log anywhere
            conn.execute_batch(
                "CREATE TABLE facts (id TEXT, subject TEXT, canonical_subject TEXT,
                                     predicate TEXT, canonical_predicate TEXT, claim TEXT);
                 CREATE TABLE predicate_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                                 recorded_at TEXT);
                 CREATE TABLE subject_aliases (alias TEXT PRIMARY KEY, canonical TEXT NOT NULL,
                                               recorded_at TEXT);
                 INSERT INTO subject_aliases VALUES ('user','user:u1','2026-08-11T03:00:00Z');
                 INSERT INTO facts VALUES ('f1','user','user:u1','lives_in','lives_in','Sonnenhof');
                 INSERT INTO facts VALUES ('f2','elvse','elvse','lives_in','lives_in','Elvse');",
            )
            .unwrap();
        }
        let raw = meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","canonical_subject":"text",
                                 "predicate":"text","canonical_predicate":"text","claim":"text"}},
            "canonical": {"facts": [
                {"source":"predicate","target":"canonical_predicate",
                 "aliases":"predicate_aliases","rejected":"predicate_rejected_pairs"},
                {"source":"subject","target":"canonical_subject","aliases":"subject_aliases",
                 "normalize":true,"rejected":"subject_rejected_pairs"}]}
        });
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let spawned = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/store"),
                raw,
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("stateful factory yields Dormant"),
        };
        wake(receiver);
        drop(sender);

        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        crate::store::query::install_connection_extensions(&conn).unwrap();
        for table in ["predicate_rejected_pairs", "subject_rejected_pairs"] {
            let n: i64 = conn
                .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
                    r.get(0)
                })
                .unwrap_or_else(|e| panic!("{table} must exist after the wake: {e}"));
            assert_eq!(n, 0, "{table} starts empty — the migration judges nothing");
        }
        // the judgement the store already carried is untouched by the migration
        let alias: String = conn
            .query_row(
                "SELECT canonical FROM subject_aliases WHERE alias='user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alias, "user:u1");
        // and the new table is immediately usable through the op, on the real
        // bindings the factory parsed
        let canonical = crate::store::params::StoreParams::parse(&meclaw_core::serde_json::json!({
            "schema": {"facts": {"id":"text","subject":"text","canonical_subject":"text",
                                 "predicate":"text","canonical_predicate":"text",
                                 "claim":"text"}},
            "canonical": {"facts": [
                {"source":"subject","target":"canonical_subject",
                 "aliases":"subject_aliases","normalize":true,
                 "rejected":"subject_rejected_pairs"}]}
        }))
        .unwrap()
        .canonical;
        let out = crate::store::ops::dispatch_with(
            &conn,
            &meclaw_core::serde_json::json!({"operation":"reject_pair","table":"facts",
                "column":"subject","left":"elvse","right":"user:u1",
                "recorded_at":"2026-08-12T03:00:00Z"}),
            &canonical,
        )
        .unwrap();
        assert_eq!(out.rows_affected, 1);
        assert!(out.error_code.is_none(), "{:?}", out.error_text);
    }

    /// Phase-13-K-2: two consecutive Dormant-Spawn-+-Wake cycles against the same
    /// cell_dir. The second Wake opens the DB with `OpenStatus::Resumed` →
    /// `load_seed_if_present` is skipped. Row count stays at 1.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_skips_seed_on_resume() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
        std::fs::write(
            cell_dir.join("seed/items.jsonl"),
            r#"{"schema":{"id":"int"}}
{"id":1}
"#,
        )
        .unwrap();
        let raw = meclaw_core::serde_json::json!({
            "schema":{"items":{"id":"int"}}
        });
        // ---- Cycle 1 (fresh): Wake → DDL + seed. ----
        let (otx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let s1 = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/s"),
                raw.clone(),
                otx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender1, receiver1, wake1) = match s1 {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("Dormant expected"),
        };
        wake1(receiver1);
        drop(sender1);
        // Wait until the seed has hit cell.db.
        for _ in 0..50 {
            if let Ok(conn) = rusqlite::Connection::open(cell_dir.join("cell.db"))
                && let Ok(n) =
                    conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                && n == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // ---- Cycle 2 (resume): Wake → OpenStatus::Resumed → no re-seed. ----
        let (otx2, _orx2) = tokio::sync::mpsc::channel(8);
        let (itx2, _irx2) = tokio::sync::mpsc::channel(8);
        let s2 = Arc::new(StoreCellFactory)
            .spawn_cell(
                Path::new("/s"),
                raw,
                otx2,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                itx2,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        let (sender2, receiver2, wake2) = match s2 {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => unreachable!("Dormant expected"),
        };
        wake2(receiver2);
        drop(sender2);
        // Allow some time for the second task to start + close. No new seed row
        // should appear.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "seed runs only on fresh");
    }
}
