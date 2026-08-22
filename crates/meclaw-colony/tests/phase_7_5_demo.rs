//! Phase-7.5 T4 demo: bootstrap spawn with restart restore via the production path.
//!
//! Proof chain: bootstrap_from_filesystem → apply_bootstrap_plan →
//! factory.spawn_cell(cell_dir = c.fs_path) → cell_task_stateful with
//! open_or_create_cell_db. After the panic_after trigger the supervisor restarts
//! via RespawnFn (build_cell_with_open_db again → overlay_from_db loads the
//! counter from cell.db.system). Confirms that cell_dir carries the right value
//! through the production path AND that restart restore works on the production
//! spawn path (not just direct-spawn-via-test-factory).
//!
//! Topology:
//!   td.path()/main/config.json            (hive, root)
//!   td.path()/main/persist/config.json    (persist_mock, panic_after=3,
//!                                          emitted_target=/sink)
//!   /sink                                 (CaptureCell, NOT in the FS tree —
//!                                          it has no factory; registered
//!                                          directly via h.spawn BEFORE
//!                                          bootstrap, so /persist→/sink
//!                                          resolves)

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::wait::{wait_for_cell_db_value, wait_for_spawn_count};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_production_bootstrap_spawn_with_restart() {
    let td = tempfile::TempDir::new().unwrap();

    // FS tree: root hive + a persist_mock cell with panic_after=3 + emitted_target=/sink.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"emitted_target":"/sink","panic_after":3},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // Factory + registry: Arc-shared — one copy in the ColonyHandle (for the
    // respawn path), one in the CellFactoryRegistry (for
    // bootstrap_from_filesystem).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // Register /sink BEFORE bootstrap (anti-cascade: /persist's emitted_target must
    // resolve at the first emission).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap registers /persist via PersistCellFactory over the production path.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // A1: /persist's echo to /sink needs an explicit catch-all out-edge — the
    // implicit identity-fallback is gone, so /persist's emission would otherwise
    // dead-letter as no_route and /sink would never receive.
    h.add_edge(Uuid::now_v7(), Path::new("/persist"), Path::new("/sink"))
        .await;

    // Phase-13-K-2: stateful cells boot Dormant — cell.db is not yet opened
    // immediately after bootstrap. Open happens lazily on first Wake-Pre-Send.
    let persist_dir = td.path().join("main").join("persist");

    // spawn_count == 0 immediately after bootstrap — Dormant-Spawn does not
    // run build_cell_with_open_db; that happens on the first Wake.
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        0,
        "spawn_count == 0 after Dormant bootstrap (no eager Wake yet)"
    );

    // Probe 1: counter 0→1, Sink sieht header.counter=1.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "1", Duration::from_secs(5)).await;

    // Post-wake assert (core proof of the cell_dir substrate): cell.db now lives
    // at the expected path — WakeFn passed the `cell_dir` param through correctly.
    assert!(
        persist_dir.join("cell.db").exists(),
        "cell.db must be opened at <fs_root>/main/persist/cell.db after first Wake"
    );
    // spawn_count == 1 after first Wake (PersistCellFactory.build_cell_with_open_db
    // ran via the Wake closure).
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        1,
        "spawn_count == 1 after first Wake-Pre-Send"
    );
    let m1 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("sink recv 1 timeout")
        .expect("sink channel closed");
    assert_eq!(
        m1.headers.hop["counter"].as_i64().unwrap(),
        1,
        "header.counter=1 after probe 1"
    );

    // Probe 2: counter 1→2, Sink sieht header.counter=2.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "2", Duration::from_secs(5)).await;
    let m2 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("sink recv 2 timeout")
        .expect("sink channel closed");
    assert_eq!(
        m2.headers.hop["counter"].as_i64().unwrap(),
        2,
        "header.counter=2 after probe 2"
    );

    // Probe 3: triggert panic_after=3. E6-Order: counter++ → snapshot
    // (cell.db.system 'counter'='3' persistiert) → panic (VOR Output-Emit).
    // The sink gets NO output 3. The supervisor restarts via RespawnFn →
    // build_cell_with_open_db runs again → spawn_count: 1→2.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "3", Duration::from_secs(5)).await;
    wait_for_spawn_count(&spawn_count, 2, Duration::from_secs(5)).await;

    // Probe 4 (post-restart): overlay_from_db loads counter=3, handle increments
    // to 4. The sink sees header.counter=4 (core proof of phase 7.5).
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "4", Duration::from_secs(5)).await;
    let m4 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("sink recv 4 timeout")
        .expect("sink channel closed");
    assert_eq!(
        m4.headers.hop["counter"].as_i64().unwrap(),
        4,
        "header.counter=4 after restart-restore"
    );

    // Total: 3 sink outputs (1, 2, 4 — probe 3 was suppressed by
    // panic-before-emit). Drain the remainder → must be 0.
    let mut extra = 0;
    while sink_rx.try_recv().is_ok() {
        extra += 1;
    }
    assert_eq!(
        extra, 0,
        "sink received exactly 3 outputs (1, 2, 4 — panic on probe 3 suppressed output)"
    );

    h.shutdown().await;
}

/// Phase-7.5 T5 demo: mutation spawn site with state at `final_path`.
///
/// Proof chain: ColonyMsg::Mutation { add_nodes } → handle_mutation →
/// stage + atomic_rename (cell dir + config.json appear on disk) →
/// factory.spawn_cell(cell_dir = sd.final_path.clone()) →
/// cell_task_stateful with open_or_create_cell_db. Proves that the SECOND
/// spawn site (mutation, besides bootstrap from T4) carries the `cell_dir`
/// param through the production path.
///
/// NO restart in this test — the RespawnFn machinery is spawn-site independent
/// (already proven by T4 via the bootstrap path).
///
/// Topology:
///   td.path()/main/config.json   (hive, root — the only initial FS item;
///                                 spec overview Z.331: root-cell-dir `main`
///                                 maps to logical `/`, its name stripped)
///   /sink                        (CaptureCell, registered directly via h.spawn
///                                 BEFORE the mutation — anti-cascade)
///   /persist                     (created via the `add_nodes` mutation under
///                                 scope `/`; logical path `/persist`, on-disk
///                                 final_path = td.path()/main/persist — the
///                                 root-cell-dir `main` is the FS anchor)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_production_mutation_spawn_with_state() {
    let td = tempfile::TempDir::new().unwrap();

    // FS tree: ONLY the root hive. NO /persist — it only comes into being via
    // the mutation through apply_mutation staging + atomic_rename.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Factory + registry: Arc-shared — one copy in the ColonyHandle (for the
    // mutation spawn site), one in the CellFactoryRegistry (for bootstrapping
    // the root hive, which by itself triggers no cell spawn because type=hive —
    // but bootstrap_from_filesystem still runs for registry validation).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // Register /sink BEFORE the mutation (anti-cascade: /main/persist's emitted_target
    // must resolve at the first emission).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap only the root hive (registers no cell — type=hive is a scope
    // marker; the factory is not called via the registry).
    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap of root-hive must succeed");

    // Pre-mutation assert: cell.db does NOT exist yet — the on-disk persist path
    // ({root}/main/persist) has not been created.
    let persist_dir = td.path().join("main").join("persist");
    assert!(
        !persist_dir.join("cell.db").exists(),
        "cell.db must NOT exist before mutation — only root-hive was bootstrapped"
    );
    assert!(
        !persist_dir.exists(),
        "{{root}}/main/persist dir must NOT exist before mutation"
    );
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        0,
        "spawn_count == 0 before mutation — hive-only bootstrap does not spawn cells"
    );

    // Phase-11 T16: create and load the template directory for persist_mock.
    {
        let templates_root = td.path().join("templates");
        let tpl = templates_root.join("persist_mock");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"persist_mock"},"params":{"emitted_target":"/unset"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(meclaw_colony::ColonyMsg::RescanTemplates {
                templates_root,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap();
    }

    // Mutation: add_nodes [{name: "persist", template: "persist_mock",
    // override_params: {emitted_target: "/sink"}}] under scope `/` (spec overview
    // Z.331: the root-cell-dir `main` IS logical `/`, its name stripped). →
    // logical path resolve_scoped_path(/, persist) = /persist; on-disk
    // final_path = {root}/main/persist (path_truth anchors logical `/` under
    // the root cell dir `main`).
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "persist",
                    "template": "persist_mock",
                    "override_params": {"emitted_target": "/sink"}
                }]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Mutation must commit; got {outcome:?}"
    );

    // A1: /persist's echo to /sink needs an explicit catch-all out-edge (the
    // implicit identity-fallback is gone). Wired after the mutation registered /persist.
    h.add_edge(Uuid::now_v7(), Path::new("/persist"), Path::new("/sink"))
        .await;

    // Phase-13-K-2: Mutation-spawn lands Dormant; cell.db not yet opened.
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        0,
        "spawn_count == 0 directly after Mutation-Spawn (Dormant — no eager build)"
    );

    // Probe an /persist: counter 0→1, Sink sieht header.counter=1.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "1", Duration::from_secs(5)).await;

    // Post-wake assert (core proof of T5): cell.db lives at
    // td.path()/main/persist/cell.db. That is only possible if handle_mutation
    // passed its `sd.final_path` through correctly as `cell_dir` to
    // factory.spawn_cell (phase-7.5 T1 substrate) AND the WakeFn closure
    // captured the path correctly (phase-13-K-2).
    assert!(
        persist_dir.join("cell.db").exists(),
        "cell.db must exist at <fs_root>/main/persist/cell.db after first Wake — \
         proves handle_mutation passed sd.final_path through as cell_dir"
    );
    assert_eq!(
        spawn_count.load(Ordering::Relaxed),
        1,
        "spawn_count == 1 after first Wake-Pre-Send on the mutation-spawned cell"
    );
    let m = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("sink recv timeout")
        .expect("sink channel closed");
    assert_eq!(
        m.headers.hop["counter"].as_i64().unwrap(),
        1,
        "header.counter=1 after first probe to mutation-spawned /persist"
    );

    h.shutdown().await;
}
