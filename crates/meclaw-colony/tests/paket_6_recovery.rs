//! Paket-6 Block D: `failed`-Recovery with a Sticky-Diskriminator.
//!
//! A `failed` cell (`active=false, failed=true`) keeps its edges. It is
//! reactivated ONLY when a mutation DIRECTLY addresses it (its path ∈ the
//! mutation's `involved` set: add_edges endpoint, add_nodes/resume target,
//! swap t2/t3). An INCIDENTAL recompute — a neighbour's mutation or a hive
//! cascade that finds the failed cell now-connected — MUST leave it failed
//! (it keeps its edges, so `now_active` would otherwise wrongly revive it).
//!
//! Every genuine false→true reconnect flip clears `failed` and resets
//! `restart_count = 0` (broad reset, including merely-inactive cells).
//!
//! Demos (all behavioural / positive-receipt where a live cell is claimed):
//!   - d-nachbar: failed cell WITH edges; a mutation touching ANOTHER node via
//!     add_edges (failed cell NOT in `involved`) → failed cell stays `failed`.
//!   - d-hive: failed cell under a hive; a mutation that triggers a hive-scope
//!     cascade recompute over its scope (failed cell not directly addressed) →
//!     stays failed.
//!   - d-route-i: failed cell + `add_edges` WITH it as an endpoint → reactivated
//!     (`failed:false, active:true`, fresh restart budget), positive receipt.
//!   - d-route-ii: failed cell + `add_nodes`-Resume on its path → passes the
//!     Awake-gate (failed parks NotYetSpawned, not Awake) → reactivated, fresh
//!     budget, positive receipt.
//!   - d-loop: failed → direct add_edges reconnect → active, fresh count → cell
//!     crashes again over its limit → `failed` again, persisted (bounded loop,
//!     no operator-free ping-pong).

use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, build_stateful_task_with_peace,
};
use meclaw_core::serde_json::Value;
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ───────────────────────────────────────────────────────────────────────────
// Shared test helpers.
// ───────────────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Read the `RegistryEntryDto` for `path` via `/colony/registry` (`active: None`
/// so failed/inactive entries are included).
async fn read_entry(h: &ColonyHandle, path: &str) -> Option<RegistryEntryDto> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Fresh read-only `colony.db` poll for `registry.status == want` (30s
/// convention). Returns the last observed status.
async fn wait_for_registry_status(td: &std::path::Path, path: &str, want: &str) -> String {
    let db = td.join("colony.db");
    let mut last = String::from("<absent>");
    for _ in 0..600 {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("fresh read-only colony.db connection");
        let status: Option<String> = conn
            .query_row("SELECT status FROM registry WHERE path = ?", [path], |r| {
                r.get(0)
            })
            .ok();
        if let Some(s) = status {
            last = s;
            if last == want {
                return last;
            }
        }
        drop(conn);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    last
}

/// Poll `counter` until it reaches `target` (30s convention). Returns last value.
async fn wait_for_count(counter: &Arc<AtomicU32>, target: u32) -> u32 {
    for _ in 0..600 {
        let n = counter.load(Ordering::Relaxed);
        if n >= target {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    counter.load(Ordering::Relaxed)
}

/// One probe with a real UBF body so a live cell can act on it. Low ttl bounds
/// any incidental self-loop.
fn probe(target: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(4)
        .build()
}

// ───────────────────────────────────────────────────────────────────────────
// FlipMockCell: a stateful cell that either HANGS (drives exhaustion → `failed`)
// or EMITS one UBF body to `echo_to` (a positive `/sink` receipt). The behaviour
// is governed by a SHARED `Arc<AtomicBool> hang` so a single FS cell can first be
// driven into `failed` (hang=true) and, after a deliberate reconnect, deliver a
// positive receipt (hang=false). Structurally mirrors `HangCellFactory` (Dormant
// stateful, lazy wake, `restart_limit` from config) so the real corridors run.
// ───────────────────────────────────────────────────────────────────────────

struct FlipMockCell {
    hang: Arc<AtomicBool>,
    echo_to: Path,
}

impl meclaw_colony::stateful_cell::StatefulCell for FlipMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: meclaw_core::Message,
        sink: &'a meclaw_core::OutputSink,
        _db: &'a mut meclaw_colony::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if self.hang.load(Ordering::SeqCst) {
                // Never returns — only the B-backstop can end this handle().
                std::future::pending::<()>().await;
            }
            let _ = sink
                .push(meclaw_core::CellOutput {
                    target: self.echo_to.clone(),
                    content: json!({
                        "header": {"done": true},
                        "messages": [
                            {"origin": "assistant", "type": "text", "text": "alive after reconnect"}
                        ]
                    }),
                })
                .await;
        }
    }
}

/// Factory for `FlipMockCell`. `hang` + `echo_to` + `spawns` are shared so the
/// test drives behaviour and observes (re-)spawns. `spawns` bumps per successful
/// build (initial Wake + every Respawn).
struct FlipCellFactory {
    hang: Arc<AtomicBool>,
    echo_to: Path,
    spawns: Arc<AtomicU32>,
}

impl FlipCellFactory {
    fn build_cell(&self, cell_dir: &std::path::Path) -> (FlipMockCell, rusqlite::Connection) {
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
        self.spawns.fetch_add(1, Ordering::Relaxed);
        (
            FlipMockCell {
                hang: self.hang.clone(),
                echo_to: self.echo_to.clone(),
            },
            conn,
        )
    }
}

impl CellFactory for FlipCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    /// F1-KH2 kind discriminator: this factory produces a `Dormant` (lazy
    /// stateful) cell, so it MUST declare `is_lazy() = true`. Under A7 a flip
    /// cell that boots INACTIVE (island) is registered via
    /// `register_inactive_non_spawned`, which dispatches on `is_lazy()` — the
    /// default `false` would mis-wire it as eager (wake `None`) and a later
    /// reconnect could never wake it. (Grace masked this: the cell used to boot
    /// active and never took the boot-inactive path.)
    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — crash/backstop-restart path (clones captured OUTSIDE the
        // closure → the `handle_cell_died` corridor stays await-free).
        let respawn_self = self.clone();
        let respawn_path = path.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_cell_dir = cell_dir.clone();
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_blob = blob_store.clone();
        let respawn_cap = mailbox_capacity;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (cell, conn) = respawn_self.build_cell(&respawn_cell_dir);
                let db = meclaw_colony::DbConn::wrap(conn, None);
                let (s, r) = mpsc::channel::<Message>(respawn_cap);
                let (j, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                    build_stateful_task_with_peace(
                        respawn_path.clone(),
                        r,
                        respawn_outputs.clone(),
                        respawn_inbox.clone(),
                        idle_timeout,
                        message_timeout,
                        cell_timeout,
                        cell,
                        db,
                        respawn_blob.clone(),
                     None,);
                meclaw_colony::renotify_stop_wiring(
                    &respawn_inbox,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, j, peace_rx, backstop_rx)
            },
        );

        // WakeFn — NotYetSpawned/Asleep → Awake (lazy stateful).
        let wake_self = self.clone();
        let wake_path = path.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_cell_dir = cell_dir.clone();
        let wake_inbox = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob = blob_store.clone();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (cell, conn) = wake_self.build_cell(&wake_cell_dir);
            let db = meclaw_colony::DbConn::wrap(conn, None);
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                build_stateful_task_with_peace(
                    wake_path.clone(),
                    recv,
                    wake_outputs.clone(),
                    wake_inbox.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    wake_blob.clone(),
                    None,
                );
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            (stop_tx, death_ack_rx)
        });

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

/// Bootstrap wiring bundle returned by [`flip_factories`]: the ordered
/// `(name, factory)` list, the matching [`CellFactoryRegistry`], and the `flip`
/// factory handle returned separately so a test can drive it after bootstrap.
type FlipFactoryWiring = (
    Vec<(String, Arc<dyn CellFactory>)>,
    CellFactoryRegistry,
    Arc<dyn CellFactory>,
);

/// `(name, factory)` list + matching `CellFactoryRegistry` for bootstrap, wiring
/// a `flip` FlipCellFactory + `echo` EchoCellFactory.
fn flip_factories(
    hang: Arc<AtomicBool>,
    echo_to: &str,
    spawns: Arc<AtomicU32>,
) -> FlipFactoryWiring {
    let flip: Arc<dyn CellFactory> = Arc::new(FlipCellFactory {
        hang,
        echo_to: Path::new(echo_to),
        spawns,
    });
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    let list = vec![
        ("flip".to_string(), flip.clone()),
        ("echo".to_string(), echo.clone()),
    ];
    let mut reg = CellFactoryRegistry::new();
    reg.insert("flip".into(), flip.clone());
    reg.insert("echo".into(), echo);
    (list, reg, flip)
}

// ───────────────────────────────────────────────────────────────────────────
// d-nachbar: a failed cell WITH edges; a mutation touching ANOTHER node via
// add_edges (the failed cell is NOT in `involved`) → the failed cell stays
// `failed:true`, NO spawn. Proven positively: the neighbour add_edges COMMITS,
// the failed cell's persisted status stays "failed".
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_nachbar_incidental_neighbour_mutation_leaves_failed() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // /anchor (echo) → /flip activates flip; flip hangs → exhaustion → failed.
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/flip/config.json",
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /other (echo, self-echo) — the UNRELATED neighbour the later mutation edges.
    write(
        td.path(),
        "main/other/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/other"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let hang = Arc::new(AtomicBool::new(true));
    let spawns = Arc::new(AtomicU32::new(0));
    let (list, reg, _flip) = flip_factories(hang, "/sink", spawns.clone());
    let h = ColonyHandle::new_with_factories_at(&td, list);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");

    // Activate /flip (edge anchor→flip), wake it → it hangs → backstop →
    // exhaustion (restart_limit 0) → failed.
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"flip"}]}}),
    )
    .await;
    assert!(matches!(act, MutationOutcome::Committed { .. }), "{act:?}");
    h.send(probe("/flip")).await;
    let status = wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await;
    assert_eq!(status, "failed", "/flip must be driven into failed");
    let spawns_at_failed = spawns.load(Ordering::Relaxed);

    // INCIDENTAL neighbour mutation: edge /anchor→/other. /flip keeps its edge
    // (anchor→flip is untouched) so a recompute would find it `now_active`, but
    // /flip is NOT in `involved` → the Sticky-Diskriminator must leave it failed.
    let m = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"other"}]}}),
    )
    .await;
    assert!(
        matches!(m, MutationOutcome::Committed { .. }),
        "neighbour add_edges must commit, got {m:?}"
    );

    // /flip stays failed (registry + persisted) and no extra spawn fired.
    let e = read_entry(&h, "/flip")
        .await
        .expect("/flip visible (No-Delete)");
    assert!(
        e.failed,
        "incidental neighbour mutation must leave /flip failed; got {e:?}"
    );
    assert!(!e.active, "failed /flip must stay inactive; got {e:?}");
    let status2 = wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await;
    assert_eq!(status2, "failed", "persisted status must stay failed");
    assert_eq!(
        spawns.load(Ordering::Relaxed),
        spawns_at_failed,
        "no respawn of the failed cell on a neighbour mutation"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// d-hive: a failed cell under a hive; a mutation that triggers a hive-scope
// cascade recompute over its scope (the failed cell is not DIRECTLY addressed)
// → it stays failed. The mutation edges a SIBLING under the same hive via the
// hive endpoint, so the recompute scope reaches the failed cell's subtree, yet
// the Sticky-Diskriminator keeps it failed.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_hive_cascade_recompute_leaves_failed() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive + two top-level gating sources /x and /y (echo). The sub-hive /h
    // is INTERNALLY wired (`./src -> ./flip`) and gated at the parent level by an
    // edge `x -> h` (the working hive-cascade pattern, mirrors
    // phase_13_5_lifecycle_3b_hive_disconnect_lr). /h/src self-echo-bootstraps the
    // probe to /h/flip.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/x/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/y/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/y"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Sub-hive /h with internal wiring `./src -> ./flip`, so once /h is gated
    // active /h/flip is connected and wakeable.
    write(
        td.path(),
        "main/h/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./src","to":"./flip"}]}}}"#,
    );
    write(
        td.path(),
        "main/h/src/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/h/flip/config.json",
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let hang = Arc::new(AtomicBool::new(true));
    let spawns = Arc::new(AtomicU32::new(0));
    let (list, reg, _flip) = flip_factories(hang, "/sink", spawns.clone());
    let h = ColonyHandle::new_with_factories_at(&td, list);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");

    // Gate the hive active: parent-level edge x -> h (short-names at scope `/`).
    // The internal `./src -> ./flip` wiring then makes /h/flip active.
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"h"}]}}),
    )
    .await;
    assert!(matches!(act, MutationOutcome::Committed { .. }), "{act:?}");
    // Wake /h/flip → hang → backstop → exhaustion (restart_limit 0) → failed.
    h.send(probe("/h/flip")).await;
    let status = wait_for_registry_status(&h.tempdir_path(), "/h/flip", "failed").await;
    assert_eq!(status, "failed", "/h/flip must be driven into failed");
    let spawns_at_failed = spawns.load(Ordering::Relaxed);

    // Incidental hive-scope CASCADE: add a SECOND parent-level gating edge y -> h.
    // This recomputes the WHOLE /h hive scope (cascade over all members) and
    // reaches /h/flip, but /h/flip is NOT directly in `involved` → the
    // Sticky-Diskriminator must leave it failed (it keeps its internal edge, so
    // `now_active` would otherwise wrongly revive it).
    let m = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"y","to":"h"}]}}),
    )
    .await;
    assert!(
        matches!(m, MutationOutcome::Committed { .. }),
        "hive-sibling add_edges must commit, got {m:?}"
    );

    let e = read_entry(&h, "/h/flip").await.expect("/h/flip visible");
    assert!(
        e.failed,
        "hive cascade recompute must leave /h/flip failed; got {e:?}"
    );
    assert!(!e.active, "failed /h/flip must stay inactive; got {e:?}");
    let status2 = wait_for_registry_status(&h.tempdir_path(), "/h/flip", "failed").await;
    assert_eq!(status2, "failed", "persisted status must stay failed");
    assert_eq!(
        spawns.load(Ordering::Relaxed),
        spawns_at_failed,
        "no respawn of the failed cell on a hive cascade"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// d-route-i: failed cell + `add_edges` WITH it as an endpoint → reactivated
// (`failed:false, active:true`, fresh restart budget), POSITIVE receipt: after
// flipping hang=false the reactivated cell processes a probe and /sink receives.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_route_i_direct_add_edges_reactivates_with_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/flip/config.json",
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let hang = Arc::new(AtomicBool::new(true));
    let spawns = Arc::new(AtomicU32::new(0));
    let (list, reg, _flip) = flip_factories(hang.clone(), "/sink", spawns.clone());
    let h = ColonyHandle::new_with_factories_at(&td, list);

    // /sink (CaptureCell) BEFORE bootstrap (anti-cascade) — the positive receipt.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        meclaw_testing::topologies::phase_3a::CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");

    // Drive /flip into failed (hang=true).
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"flip"}]}}),
    )
    .await;
    assert!(matches!(act, MutationOutcome::Committed { .. }), "{act:?}");
    h.send(probe("/flip")).await;
    assert_eq!(
        wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await,
        "failed"
    );

    // Stop hanging: the reactivated cell will now emit a receipt to /sink.
    hang.store(false, Ordering::SeqCst);

    // DIRECT add_edges with /flip as an endpoint (sink→flip): /flip ∈ involved →
    // Sticky-Diskriminator allows the reconnect. (anchor→flip already exists; this
    // re-adds a distinct edge that directly addresses /flip.)
    // W2b (Ruling A1): also wire /flip -> /sink so the reactivated cell's echo
    // reaches /sink (identity-fallback gone). /flip stays ∈ involved (endpoint in
    // both edges); both endpoints live.
    let m = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"sink","to":"flip"},{"from":"flip","to":"sink"}]}}),
    )
    .await;
    assert!(
        matches!(m, MutationOutcome::Committed { .. }),
        "direct add_edges must commit, got {m:?}"
    );

    // Reactivated: failed cleared, active, fresh restart budget.
    let e = read_entry(&h, "/flip").await.expect("/flip");
    assert!(!e.failed, "direct reconnect must clear failed; got {e:?}");
    assert!(e.active, "direct reconnect must reactivate; got {e:?}");
    // restart_count is reset to 0 by the reconnect branch; the DTO does not
    // surface restart_count, so reactivation is proven behaviourally here (the
    // cell goes live and answers a probe). The reset itself is exercised
    // end-to-end by d-loop (a reset budget is what lets the reconnected cell
    // wake + re-crash again rather than staying exhausted).

    // Positive receipt: probe /flip → it processes (no longer hangs) → /sink hit.
    h.send(probe("/flip")).await;
    let got = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .ok()
        .flatten();
    assert!(
        got.is_some(),
        "reactivated /flip must process a probe → /sink receipt (positive proof of a live cell)"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// d-route-ii: failed cell + `add_nodes`-Resume on its path. A failed cell is
// parked NotYetSpawned (A3), NOT Awake → the Resume passes the
// `resume_requires_stopped_cell` Awake-gate → reactivated (`failed:false`, fresh
// budget), POSITIVE receipt.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_route_ii_add_nodes_resume_reactivates_with_receipt() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/flip/config.json",
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // A `flip` template (type-compatible with the bootstrap /flip cell) so an
    // `add_nodes` Resume can re-address /flip's existing path.
    let tpl_dir = td.path().join("templates").join("flip");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"flip"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let hang = Arc::new(AtomicBool::new(true));
    let spawns = Arc::new(AtomicU32::new(0));
    let (list, reg, _flip) = flip_factories(hang.clone(), "/sink", spawns.clone());
    let h = ColonyHandle::new_with_factories_at(&td, list);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        meclaw_testing::topologies::phase_3a::CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");
    rescan_templates(&h, td.path().join("templates")).await;

    // Drive /flip into failed.
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"flip"}]}}),
    )
    .await;
    assert!(matches!(act, MutationOutcome::Committed { .. }), "{act:?}");
    h.send(probe("/flip")).await;
    assert_eq!(
        wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await,
        "failed"
    );

    hang.store(false, Ordering::SeqCst);

    // add_nodes-Resume on /flip's existing path. /flip is failed → NotYetSpawned,
    // NOT Awake → passes the Awake-gate; the Resume target ∈ `involved` → the
    // Sticky-Diskriminator allows the reconnect.
    let m = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"flip","template":"flip"}]}}),
    )
    .await;
    match &m {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => panic!(
            "add_nodes-Resume on a failed (NotYetSpawned) cell must COMMIT (passes the Awake gate), \
             got Rejected({error_code}): {details}"
        ),
    }

    let e = read_entry(&h, "/flip").await.expect("/flip");
    assert!(!e.failed, "add_nodes-Resume must clear failed; got {e:?}");
    assert!(e.active, "add_nodes-Resume must reactivate; got {e:?}");
    // restart_count is reset to 0 by the reconnect branch (DTO has no field;
    // reactivation is proven behaviourally below via a /sink receipt).

    // W2b (Ruling A1): wire /flip -> /sink so the resumed cell's echo reaches
    // /sink (identity-fallback gone; the add_nodes-Resume carries no edge). Both
    // endpoints live.
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"flip","to":"sink"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /flip -> /sink must commit"
    );

    h.send(probe("/flip")).await;
    let got = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .ok()
        .flatten();
    assert!(
        got.is_some(),
        "resumed /flip must process a probe → /sink receipt (positive proof)"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// d-loop: failed → direct add_edges reconnect → active, fresh count → cell
// crashes again over its limit → `failed` again, persisted. A BOUNDED loop: the
// reconnect is operator-driven; without an operator act there is no ping-pong.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_loop_reconnect_then_recrash_refails_bounded() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/flip/config.json",
        r#"{"cell":{"type":"flip","message_timeout":300,"restart_limit":0},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // A second source cell so the operator reconnect is a DISTINCT edge directly
    // addressing /flip (other→flip), not a dedup of the existing anchor→flip.
    write(
        td.path(),
        "main/other/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/flip"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let hang = Arc::new(AtomicBool::new(true));
    let spawns = Arc::new(AtomicU32::new(0));
    let (list, reg, _flip) = flip_factories(hang.clone(), "/sink", spawns.clone());
    let h = ColonyHandle::new_with_factories_at(&td, list);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");

    // ── Round 1: drive /flip into failed. ────────────────────────────────────
    let act = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"flip"}]}}),
    )
    .await;
    assert!(matches!(act, MutationOutcome::Committed { .. }), "{act:?}");
    h.send(probe("/flip")).await;
    assert_eq!(
        wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await,
        "failed",
        "round 1: /flip must reach failed"
    );

    // ── Operator act: direct add_edges reconnect → active, fresh count. ───────
    let spawns_before_reconnect = spawns.load(Ordering::Relaxed);
    let m = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"other","to":"flip"}]}}),
    )
    .await;
    assert!(
        matches!(m, MutationOutcome::Committed { .. }),
        "reconnect must commit, got {m:?}"
    );
    let e = read_entry(&h, "/flip").await.expect("/flip");
    assert!(
        !e.failed && e.active,
        "reconnect → active, failed cleared; got {e:?}"
    );
    // restart_count is reset to 0 by the reconnect branch (DTO has no field).
    // This is proven behaviourally below: the cell wakes + re-crashes over its
    // limit — only possible if the budget was reset (a stale count == limit+1
    // would leave it exhausted and it would never spawn again).

    // ── Round 2: still hanging → wake it → crash again over the limit → failed. ─
    h.send(probe("/flip")).await;
    // A fresh wake must have spawned the cell again (count advanced).
    let woke = wait_for_count(&spawns, spawns_before_reconnect + 1).await;
    assert!(
        woke > spawns_before_reconnect,
        "round 2: reconnected /flip must spawn again on a probe"
    );
    let status = wait_for_registry_status(&h.tempdir_path(), "/flip", "failed").await;
    assert_eq!(
        status, "failed",
        "round 2: re-crashing over the limit must re-fail + persist (bounded loop)"
    );
    let e = read_entry(&h, "/flip").await.expect("/flip");
    assert!(
        e.failed && !e.active,
        "round 2: /flip failed again; got {e:?}"
    );

    h.shutdown().await;
}
