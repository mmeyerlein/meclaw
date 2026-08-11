//! Test-facing wrapper around a running Colony task.
//!
//! Owns Colony's inbox sender + outputs sender so tests can:
//!   - `send`: inject Route messages into Colony
//!   - `spawn`: create a cell + register it (Task 17)
//!   - `shutdown`: terminate Colony and wait for it
//!
//! The wrapper uses `multi_thread` Tokio runtime in tests — never `current_thread`,
//! per CONTRIBUTING.md rule 11 + the test-infrastructure section.

use meclaw_colony::{
    CellFactoryRegistry, ColonyDb, ColonyMsg, DeadLetter, RespawnFn, SpawnedCellKind, cell_task,
    colony_task,
};
use meclaw_core::{Cell, CellEmission, Message, Path, Uuid};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub struct ColonyHandle {
    /// Sender into the Colony inbox; exposed for constructing a `ColonyRuntime`
    /// until T4 adds the `runtime()` helper method.
    pub inbox_tx: mpsc::Sender<ColonyMsg>,
    /// Sender into the outputs channel; exposed for constructing a `ColonyRuntime`
    /// until T4 adds the `runtime()` helper method.
    pub outputs_tx: mpsc::Sender<CellEmission>,
    join: Option<JoinHandle<()>>,
    /// TempDir lifecycle holder (None when using an externally-managed path).
    /// Must be kept alive for the duration of the handle to prevent the temp
    /// directory from being deleted while the Colony task is running.
    #[allow(dead_code)]
    tempdir: Option<tempfile::TempDir>,
    /// Directory containing `colony.db`.
    db_path: std::path::PathBuf,
    /// Colony config read from the test root's `colony.json` (Phase-13.5 A7).
    /// Shared between the spawned `colony_task` and the `runtime()` helper so the
    /// bootstrap-apply path sees the same defaults as the live task.
    colony_config: meclaw_colony::ColonyConfig,
    /// Per-colony blob store (Phase-13.5 A8). `None` for the default test path;
    /// `Some` when a constructor wires a real `DiskBlobStore` (so `colony_task`'s
    /// auto-offload AND spawned cells' delivery-boundary resolution are live).
    /// Shared into `colony_task`, `spawn()`'s `cell_task`, and `runtime()`.
    blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
}

impl ColonyHandle {
    /// Default constructor for tests: spawns a fresh `ColonyDb` in a `TempDir`.
    ///
    /// The underlying temporary directory is preserved on the filesystem until
    /// the OS reclaims it (or the test calls [`tempdir_path`] and cleans up
    /// manually). This allows callers to re-open the same `colony.db` after
    /// `shutdown().await` via [`with_db_path`].
    pub fn new() -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        // Persist the directory so it survives `shutdown()` / handle-drop.
        // The OS will reclaim it on reboot; tests that need strict cleanup can
        // call `std::fs::remove_dir_all(h.tempdir_path())` after shutdown.
        let dir = td.keep();
        Self::build_with(dir, None, CellFactoryRegistry::new())
    }

    /// Phase-6 test helper: same as [`new`] but pre-registers
    /// [`crate::factories::echo::EchoCellFactory`] under the type-key `"echo"`
    /// in the factories registry. Used by Mutation tests that exercise the
    /// spawn-after-rename path (T18) with an `add_nodes` diff against
    /// `template: "echo"`.
    pub fn new_with_echo() -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let dir = td.keep();
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo".into(),
            Arc::new(crate::factories::echo::EchoCellFactory)
                as Arc<dyn meclaw_colony::CellFactory>,
        );
        Self::build_with(dir, None, factories)
    }

    /// Re-open constructor: uses an existing `colony.db` file.
    ///
    /// The `TempDir` lifecycle lies with the caller (e.g. T24 re-boot tests,
    /// T37 demo). The directory at `db_path.parent()` must already exist.
    pub fn with_db_path(db_path: &std::path::Path) -> Self {
        let dir = db_path.parent().expect("db_path has parent").to_path_buf();
        Self::build_with(dir, None, CellFactoryRegistry::new())
    }

    /// Phase-6 T26 helper: like [`new_with_echo`] but uses the caller's
    /// directory (typically backed by a `TempDir` the test holds itself).
    ///
    /// The handle does NOT take ownership of the temp dir — the caller is
    /// responsible for keeping it alive across `abort()`/`drop(handle)`. This
    /// is required by the crash-recovery demo, where the colony task is
    /// killed but the filesystem state must survive into a second boot.
    pub fn new_with_echo_at(dir: &std::path::Path) -> Self {
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo".into(),
            Arc::new(crate::factories::echo::EchoCellFactory)
                as Arc<dyn meclaw_colony::CellFactory>,
        );
        Self::build_with(dir.to_path_buf(), None, factories)
    }

    /// Phase-6.5: ColonyHandle with caller-owned `TempDir` and an arbitrary
    /// list of `(template_name, factory_arc)` registrations.
    ///
    /// Delegates to the same private [`build_with`] path as the echo variants
    /// — no spawn/DB-open logic is duplicated here; this helper only fills
    /// the `CellFactoryRegistry` from the supplied list. The caller owns the
    /// `TempDir` (like [`new_with_echo_at`]), so filesystem state survives
    /// `shutdown()`/`abort()`/`drop(handle)` for cell.db inspection.
    pub fn new_with_factories_at(
        dir: &tempfile::TempDir,
        factories: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)>,
    ) -> Self {
        let mut registry = CellFactoryRegistry::new();
        for (name, factory) in factories {
            registry.insert(name, factory);
        }
        Self::build_with(dir.path().to_path_buf(), None, registry)
    }

    fn build_with(
        dir: std::path::PathBuf,
        tempdir: Option<tempfile::TempDir>,
        factories: CellFactoryRegistry,
    ) -> Self {
        Self::build_with_blob_store(dir, tempdir, factories, None, None)
    }

    /// U8 (RULED A8): like [`new_with_factories_at`] but pins the colony's env
    /// source to an explicit path (mirrors the production `--env` flag reaching
    /// `colony_task`). All substitution paths — boot, mutation, 2b-adoption —
    /// then read from `env_source` instead of the default `<root>/.env`.
    pub fn new_with_factories_and_env_at(
        dir: &tempfile::TempDir,
        factories: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)>,
        env_source: Option<std::path::PathBuf>,
    ) -> Self {
        let mut registry = CellFactoryRegistry::new();
        for (name, factory) in factories {
            registry.insert(name, factory);
        }
        Self::build_with_blob_store(dir.path().to_path_buf(), None, registry, None, env_source)
    }

    /// Phase-13.5 A8: like [`build_with`] but wires an explicit per-colony blob
    /// store into `colony_task`, `spawn()`'s `cell_task`, and `runtime()`. With
    /// `Some(store)` the auto-offload producer hook + delivery-boundary
    /// resolution are live; `None` is the legacy no-blob test path.
    fn build_with_blob_store(
        dir: std::path::PathBuf,
        tempdir: Option<tempfile::TempDir>,
        factories: CellFactoryRegistry,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        env_source: Option<std::path::PathBuf>,
    ) -> Self {
        Self::build_with_blob_store_and_signal(
            dir, tempdir, factories, blob_store, env_source, None,
        )
    }

    /// Like [`build_with_blob_store`] but optionally wires the colony's
    /// deterministic death-ack-wait sync signal (test-only; `None` in production
    /// and for every existing constructor, so the colony path is unchanged).
    fn build_with_blob_store_and_signal(
        dir: std::path::PathBuf,
        tempdir: Option<tempfile::TempDir>,
        factories: CellFactoryRegistry,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        env_source: Option<std::path::PathBuf>,
        death_ack_wait_tx: Option<mpsc::Sender<()>>,
    ) -> Self {
        let db = ColonyDb::open(&dir.join("colony.db")).expect("open colony.db");
        let (inbox_tx, inbox_rx) = mpsc::channel(1000);
        let (outputs_tx, outputs_rx) = mpsc::channel(1000);
        let self_tx = inbox_tx.clone();
        // Phase-6 T11c: colony_task takes factories + root. Tests pass either
        // an empty registry (default for non-spawn tests) or one with
        // `EchoCellFactory` pre-registered under `"echo"` (T18+). Root is the
        // same colony-db directory; the Mutation arm creates `.staging/<id>/`
        // subdirs inside it.
        let root = dir.clone();
        // Phase-13.5 A7: read the test root's colony.json (if any) so tests can
        // exercise custom defaults (idle threshold, blob_inline_max_bytes) by
        // writing a colony.json into the caller-owned dir before construction.
        // Absent → defaults (no behaviour change for existing tests).
        let colony_config =
            meclaw_colony::colony_config::read_colony_config(&dir).unwrap_or_default();
        let mut cfg = meclaw_colony::ColonyTaskConfig::new(
            self_tx,
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            factories,
            root,
            colony_config.clone(),
            blob_store.clone(),
            env_source,
        );
        if let Some(tx) = death_ack_wait_tx {
            cfg = cfg.with_death_ack_wait_signal(tx);
        }
        let join = tokio::spawn(colony_task(cfg));
        Self {
            inbox_tx,
            outputs_tx,
            colony_config,
            join: Some(join),
            tempdir,
            db_path: dir,
            blob_store,
        }
    }

    /// Test-only: like [`new_with_factories_at`], but also wires the colony's
    /// deterministic death-ack-wait signal. Returns the handle plus a receiver
    /// that fires exactly once when the colony is about to block on the inline
    /// death-ack-wait of a disconnect mutation. A test awaits that tick to
    /// release a wedged cell at the precise point the term-timeout path begins,
    /// replacing a flaky wall-clock sleep.
    pub fn new_with_factories_and_death_ack_signal_at(
        dir: &tempfile::TempDir,
        factories: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)>,
    ) -> (Self, mpsc::Receiver<()>) {
        let mut registry = CellFactoryRegistry::new();
        for (name, factory) in factories {
            registry.insert(name, factory);
        }
        let (tx, rx) = mpsc::channel::<()>(8);
        let handle = Self::build_with_blob_store_and_signal(
            dir.path().to_path_buf(),
            None,
            registry,
            None,
            None,
            Some(tx),
        );
        (handle, rx)
    }

    /// Phase-13.5 A8 demo constructor: caller-owned dir + factories + a real
    /// `DiskBlobStore` rooted at `<dir>/blobs`. Reads the dir's `colony.json`
    /// (for a custom `blob_inline_max_bytes`). Use for end-to-end blob
    /// producer→consumer roundtrip tests.
    pub fn new_with_blobs_at(
        dir: &tempfile::TempDir,
        factories: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)>,
    ) -> Self {
        let mut registry = CellFactoryRegistry::new();
        for (name, factory) in factories {
            registry.insert(name, factory);
        }
        let blob_store = Arc::new(
            meclaw_colony::DiskBlobStore::new(dir.path().join("blobs")).expect("create blob store"),
        );
        Self::build_with_blob_store(
            dir.path().to_path_buf(),
            None,
            registry,
            Some(blob_store),
            None,
        )
    }

    /// Phase-13.5 A8: the colony's parsed `blob_inline_max_bytes` (from the test
    /// root's `colony.json`, or the 64 KiB default). Lets demos assert a custom
    /// threshold was applied.
    pub fn blob_inline_max_bytes(&self) -> usize {
        self.colony_config.blob_inline_max_bytes
    }

    /// Returns the directory path containing `colony.db`.
    ///
    /// For `new()` this is the persisted temp directory; for `with_db_path()` it
    /// is the parent directory of the supplied `db_path`.
    pub fn tempdir_path(&self) -> std::path::PathBuf {
        self.db_path.clone()
    }

    /// Spawn a cell via factory closure, register it under `path`, and wait for the ack.
    ///
    /// `factory` is called once immediately to produce the initial cell instance, and
    /// stored as the respawn closure for supervisor-driven restarts.
    pub async fn spawn<C, F>(&self, path: Path, factory: F)
    where
        C: Cell + Send + 'static,
        F: Fn() -> C + Send + Sync + 'static,
    {
        let factory_arc: Arc<F> = Arc::new(factory);
        let outputs_tx = self.outputs_tx.clone();
        // Phase-13.5 A8: capture the colony's blob store OUTSIDE the RespawnFn
        // closure (await-free, like `outputs_tx`) so spawned cells resolve
        // `Body::Blob` at the delivery boundary when a store is wired.
        let blob_store = self.blob_store.clone();
        let make_pair = {
            let factory = factory_arc.clone();
            let outputs_tx = outputs_tx.clone();
            let path = path.clone();
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (tx, rx) = mpsc::channel::<Message>(1000);
                let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
                // Backstop pair (P3-B-restart): sender dropped → never fired →
                // this test cell-task's death classifies Normal/Panic.
                let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
                let cell = (factory)();
                let path_inner = path.clone();
                let outputs_inner = outputs_tx.clone();
                let blob_inner = blob_store.clone();
                let join = tokio::spawn(async move {
                    let _peace_keep = peace_tx;
                    // Slice 2: test helper without a contract → `consumes: None`.
                    cell_task(path_inner, rx, outputs_inner, cell, blob_inner, None).await;
                });
                (tx, join, peace_rx, backstop_rx)
            }
        };
        let (sender, join, peace_rx, backstop_rx) = make_pair();
        let respawn: RespawnFn = Box::new(make_pair);

        let (ack_tx, ack_rx) = oneshot::channel();
        // Phase-13-G-3: stateless/long-running, status stays Awake → no wake
        // mechanic (F1-KH2: a stray parked delivery dead-letters loudly).
        let wake: Option<meclaw_colony::WakeFn> = None;
        self.inbox_tx
            .send(ColonyMsg::Register {
                path,
                sender,
                join,
                peace_rx,
                backstop_rx,
                // `spawn` builds a non-stop-aware test cell-task → no live stop
                // wiring. Disconnect of such a cell only flips `active=false`.
                stop_tx: None,
                death_ack_rx: None,
                respawn,
                wake,
                restart_limit: None,
                cell_id: Uuid::now_v7(),
                cell_type: "test-mock".into(),
                active: true,
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("register ack");
    }

    /// Insert an edge into the colony's edge table and wait for the ack.
    ///
    /// Phase 4: the outputs-arm uses `apply_edges` to override cell-emitted targets
    /// when at least one edge matches the sender path.
    pub async fn add_edge(&self, id: Uuid, from: Path, to: Path) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.inbox_tx
            .send(ColonyMsg::AddEdge {
                id,
                from,
                to,
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("add_edge ack");
    }

    /// Inject a Route message into Colony's inbox with an explicit sender path.
    /// External callers (tests, HTTP-API later) MUST pass a sender_path because
    /// relative-target resolution (`./x`, `../x`) depends on it.
    pub async fn send_from(&self, sender_path: Path, msg: Message) {
        self.inbox_tx
            .send(ColonyMsg::Route { sender_path, msg })
            .await
            .expect("colony inbox closed");
    }

    /// Convenience wrapper: inject a Route message with implicit sender_path = `/`.
    /// Suitable when the test does not exercise relative-target resolution.
    pub async fn send(&self, msg: Message) {
        self.send_from(Path::new("/"), msg).await;
    }

    /// **Phase-2 test hook** — read+drain the colony's dead-letter queue.
    /// Final design uses `/colony/dead_letters` + `reply_to` (Phase 3 / 12).
    pub async fn drain_dead_letters(&self) -> Vec<DeadLetter> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.inbox_tx
            .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("drain ack")
    }

    /// Register a hive scope marker.
    pub async fn add_hive_scope(&self, path: meclaw_core::Path) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.inbox_tx
            .send(ColonyMsg::AddHiveScope { path, ack: ack_tx })
            .await
            .expect("colony inbox closed");
        ack_rx.await.expect("add_hive_scope ack");
    }

    /// Return a clone of the Colony's outputs sender for use by factories.
    pub fn outputs_sender(&self) -> mpsc::Sender<CellEmission> {
        self.outputs_tx.clone()
    }

    /// Return a `ColonyRuntime` snapshot — the minimal primitives for the apply
    /// path. Clones both senders; every call yields a fresh `ColonyRuntime`.
    pub fn runtime(&self) -> meclaw_colony::ColonyRuntime {
        meclaw_colony::ColonyRuntime {
            inbox_tx: self.inbox_tx.clone(),
            outputs_tx: self.outputs_tx.clone(),
            colony_config: self.colony_config.clone(),
            blob_store: self.blob_store.clone(),
        }
    }

    /// Register a `SpawnedCellKind` under `path` in Colony's registry, exactly
    /// like `spawn` does but for a pre-spawned cell. Phase-13-K-2: both
    /// `Active` and `Dormant` are supported — `Active` sends
    /// `ColonyMsg::Register` (status Awake), `Dormant` sends
    /// `ColonyMsg::RegisterDormant` (status NotYetSpawned, the cell task is
    /// spawned on the first wake-pre-send).
    pub async fn register_spawned(&self, path: meclaw_core::Path, sp: SpawnedCellKind) {
        self.register_spawned_typed(path, sp, "test-mock").await;
    }

    /// Like [`register_spawned`] but with an explicit `cell_type` — needed for
    /// `swap_nodes` tests, whose F2 compatibility check compares the registry
    /// entry's `cell_type` against the with-template's `cell.type`. Production
    /// spawn paths (bootstrap / mutation add) set the real cell-type; this helper
    /// lets a manually-registered test cell match its template's type.
    pub async fn register_spawned_typed(
        &self,
        path: meclaw_core::Path,
        sp: SpawnedCellKind,
        cell_type: &str,
    ) {
        let cell_type = cell_type.to_string();
        let (ack_tx, ack_rx) = oneshot::channel();
        match sp {
            SpawnedCellKind::Active {
                sender,
                join,
                peace_rx,
                // Phase-13.5 Lifecycle-3b Task 4: a pre-spawned cell carries its
                // live stop pair → store it so a disconnect can peace-stop it.
                stop_tx,
                death_ack_rx,
                // Paket-3 P3-B-restart: forwarded to the colony watcher.
                backstop_rx,
                respawn,
            } => {
                // Stateless/long-running: status stays Awake → no wake mechanic
                // (F1-KH2: a stray parked delivery dead-letters loudly).
                let wake: Option<meclaw_colony::WakeFn> = None;
                self.inbox_tx
                    .send(ColonyMsg::Register {
                        path,
                        sender,
                        join,
                        peace_rx,
                        backstop_rx,
                        stop_tx: Some(stop_tx),
                        death_ack_rx: Some(death_ack_rx),
                        respawn,
                        wake,
                        restart_limit: None,
                        cell_id: Uuid::now_v7(),
                        cell_type: cell_type.clone(),
                        active: true,
                        ack: ack_tx,
                    })
                    .await
                    .expect("colony inbox closed");
            }
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                // Phase-13.5 Lifecycle-3b Task 3: dropped here (test handle);
                // Task 4 stores them in the registry.
                stop_tx: _,
                death_ack_rx: _,
                respawn,
            } => {
                self.inbox_tx
                    .send(ColonyMsg::RegisterDormant {
                        path,
                        sender,
                        receiver,
                        // Lazy kind: the factory's REAL wake (wake-on-message).
                        wake: Some(wake),
                        respawn,
                        restart_limit: None,
                        cell_id: Uuid::now_v7(),
                        cell_type: cell_type.clone(),
                        active: true,
                        // Fresh spawn is never failed.
                        failed: false,
                        // Lazy stateful (Dormant kind) → wake-on-message.
                        eager_on_reconnect: false,
                        ack: ack_tx,
                    })
                    .await
                    .expect("colony inbox closed");
            }
        }
        ack_rx.await.expect("register_spawned ack");
    }

    /// Phase-6 T26: simulate a process crash by aborting the colony task.
    ///
    /// NO shutdown, NO graceful drain — pure `JoinHandle::abort()`. The
    /// writer thread inside `ColonyDb` will be dropped without a flush, the
    /// inbox channel closes, any in-flight mutation parked between rename(2)
    /// and the spawn-loop never reaches the committed-update path.
    ///
    /// The caller MUST hold the surrounding `TempDir` independently — see
    /// [`new_with_echo_at`]. Filesystem state (colony.db + `.staging/`)
    /// survives the abort and is the input to
    /// `recover_in_flight_mutations`.
    pub fn abort(mut self) {
        if let Some(j) = self.join.take() {
            j.abort();
        }
    }

    /// Take the colony-task `JoinHandle` and await it, returning the join result.
    /// `Err(e)` with `e.is_panic()` proves the `colony_task` panicked (Deep-Audit
    /// F2 mid-rename strict-fail). Filesystem state (colony.db + live tree)
    /// survives, for post-panic assertions.
    pub async fn join_result(mut self) -> Result<(), tokio::task::JoinError> {
        match self.join.take() {
            Some(j) => j.await,
            None => Ok(()),
        }
    }

    /// Send Shutdown to Colony and wait for it to finish.
    pub async fn shutdown(mut self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self
            .inbox_tx
            .send(ColonyMsg::Shutdown { ack: ack_tx })
            .await;
        let _ = ack_rx.await;
        if let Some(j) = self.join.take() {
            let _ = j.await;
        }
    }
}

impl Default for ColonyHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn a raw `colony_task` at `dir` and return its `JoinHandle<()>` directly,
/// without wrapping it in a [`ColonyHandle`].
///
/// Mirrors the channel/registry/root construction of
/// [`ColonyHandle::new_with_factories_at`] but hands the caller the bare
/// `JoinHandle` instead of storing it. This is the only way to assert on a
/// hard-fail boot panic: the colony task panics during reboot-init (before its
/// `select!` loop) when persisted CEL is corrupt, and that panic surfaces as a
/// `JoinError` on the `JoinHandle`. Awaiting the handle and inspecting
/// `is_panic()` / `into_panic()` is the sanctioned assertion path — no
/// `catch_unwind` over the boot.
///
/// `ColonyDb::open` on an existing `dir/colony.db` detects `BootState::Reboot`
/// and runs the edge-hydration path, so this is the reboot entry point for the
/// hard-fail demo tests.
pub fn spawn_colony_task_at(
    dir: &std::path::Path,
    factories: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)>,
) -> JoinHandle<()> {
    let mut registry = CellFactoryRegistry::new();
    for (name, factory) in factories {
        registry.insert(name, factory);
    }
    let db = ColonyDb::open(&dir.join("colony.db")).expect("open colony.db");
    let (inbox_tx, inbox_rx) = mpsc::channel(1000);
    let (outputs_tx, outputs_rx) = mpsc::channel(1000);
    let self_tx = inbox_tx.clone();
    let root = dir.to_path_buf();
    tokio::spawn(colony_task(meclaw_colony::ColonyTaskConfig::new(
        self_tx,
        inbox_rx,
        outputs_tx,
        outputs_rx,
        db,
        registry,
        root,
        meclaw_colony::ColonyConfig::default(),
        // Phase-13.5 A8: test colony helper uses no blob store.
        None,
        // U8: no explicit env source — defaults to `<root>/.env`.
        None,
    )))
}

impl Drop for ColonyHandle {
    /// Emergency abort of the Colony task.
    ///
    /// Emergency only. Always call `shutdown().await` first to ensure the
    /// `ColonyDb` writer thread is joined cleanly before this destructor runs.
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            j.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageBuilder;
    use crate::mocks::EchoMockCell;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_handle_helpers_work_for_phase_5_tests() {
        let h = ColonyHandle::new();
        let path = h.tempdir_path();
        assert!(path.exists());
        h.shutdown().await;
        // with_db_path opens the existing DB without a new TempDir
        let h2 = ColonyHandle::with_db_path(&path.join("colony.db"));
        h2.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_handle_starts_and_shuts_down() {
        let h = ColonyHandle::new();
        h.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_handle_spawn_registers_cell() {
        let h = ColonyHandle::new();
        let (tap_tx, mut tap_rx) = mpsc::channel(8);
        h.spawn(Path::new("/echo"), {
            let tap = tap_tx.clone();
            move || EchoMockCell::new(Path::new("/echo")).tap_to(tap.clone())
        })
        .await;
        h.send(MessageBuilder::new("/echo").build()).await;
        let p = tap_rx.recv().await.unwrap();
        assert_eq!(p.as_str(), "/echo");
        h.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn colony_handle_drain_dead_letters_returns_unresolved_misses() {
        let h = ColonyHandle::new();
        // No cells registered; route to /missing must dead-letter.
        h.send(MessageBuilder::new("/missing").build()).await;
        let dls = h.drain_dead_letters().await;
        assert_eq!(dls.len(), 1);
        assert_eq!(dls[0].resolved_target.as_str(), "/missing");
        h.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn handle_exposes_runtime() {
        let h = ColonyHandle::new();
        let rt = h.runtime();
        assert!(!rt.inbox_tx.is_closed());
        assert!(!rt.outputs_tx.is_closed());
        h.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn registry_upsert_writes_row_per_register() {
        use meclaw_core::Path;
        let h = ColonyHandle::new();
        h.spawn(Path::new("/x"), || {
            crate::mocks::EchoMockCell::new(Path::new("/x"))
        })
        .await;
        let db_path = h.tempdir_path().join("colony.db");
        h.shutdown().await; // BARRIER — the UPSERT op was enqueued before the ack, now flushed.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM registry WHERE path='/x'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(cnt, 1, "register persists exactly one registry row");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn route_arm_logs_external_injection_with_at_external_sentinel() {
        use crate::MessageBuilder;
        let h = ColonyHandle::new();
        h.spawn(Path::new("/x"), || {
            crate::mocks::EchoMockCell::new(Path::new("/x"))
        })
        .await;
        h.send(MessageBuilder::new("/x").build()).await;
        let db_path = h.tempdir_path().join("colony.db");
        h.shutdown().await; // BARRIER
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let from: String = conn
            .query_row(
                "SELECT from_path FROM message_log WHERE to_path='/x' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            from, "@external",
            "Source-Message (parent_message_id IS NULL) → @external-Sentinel"
        );
    }
}
