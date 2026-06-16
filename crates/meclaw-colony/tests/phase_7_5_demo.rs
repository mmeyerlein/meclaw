//! Phase-7.5 T4 Demo: Bootstrap-Spawn mit Restart-Restore via Production-Path.
//!
//! Beweis-Kette: bootstrap_from_filesystem → apply_bootstrap_plan →
//! factory.spawn_cell(cell_dir = c.fs_path) → cell_task_stateful mit
//! open_or_create_cell_db. Nach panic_after-Trigger restartet der Supervisor
//! via RespawnFn (build_cell_with_open_db neu → overlay_from_db lädt counter
//! aus cell.db.system). Bestätigt, dass cell_dir den richtigen Wert durch
//! den Production-Path trägt UND Restart-Restore auf der Production-Spawn-
//! Path funktioniert (nicht nur direct-spawn-via-test-factory).
//!
//! Topologie:
//!   td.path()/main/config.json            (hive, root)
//!   td.path()/main/persist/config.json    (persist_mock, panic_after=3,
//!                                          echo_to=/sink)
//!   /sink                                 (CaptureCell, NICHT im FS-tree —
//!                                          hat keine Factory; direkt via
//!                                          h.spawn registriert VOR bootstrap,
//!                                          damit /persist→/sink resolved)

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

    // FS-Tree: root-hive + persist_mock-Cell mit panic_after=3 + echo_to=/sink.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/sink","panic_after":3},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // Factory + Registry: Arc-share — eine Kopie in ColonyHandle (für
    // Respawn-Pfad), eine in CellFactoryRegistry (für bootstrap_from_filesystem).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // /sink VOR bootstrap registrieren (anti-cascade: /persist's echo_to
    // muss bei der ersten Emission resolved sein).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap registriert /persist via PersistCellFactory über Production-Pfad.
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

    // Post-Wake-Assert (Kern-Beweis cell_dir-Substrat): cell.db lebt jetzt am
    // erwarteten Pfad — WakeFn hat den `cell_dir`-Param korrekt durchgereicht.
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
    // Sink bekommt KEIN Output 3. Supervisor restartet via RespawnFn →
    // build_cell_with_open_db läuft erneut → spawn_count: 1→2.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    wait_for_cell_db_value(&persist_dir, "counter", "3", Duration::from_secs(5)).await;
    wait_for_spawn_count(&spawn_count, 2, Duration::from_secs(5)).await;

    // Probe 4 (post-restart): overlay_from_db lädt counter=3, handle inkrementiert
    // auf 4. Sink sieht header.counter=4 (Kern-Beweis von Phase 7.5).
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

    // Total: 3 Sink-Outputs (1, 2, 4 — Probe 3 wurde durch panic-before-emit
    // unterdrückt). Drain remaining → muss 0 sein.
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

/// Phase-7.5 T5 Demo: Mutation-Spawn-Site mit State an `final_path`.
///
/// Beweis-Kette: ColonyMsg::Mutation { add_nodes } → handle_mutation →
/// stage + atomic_rename (cell-dir + config.json erscheinen on-disk) →
/// factory.spawn_cell(cell_dir = sd.final_path.clone()) →
/// cell_task_stateful mit open_or_create_cell_db. Beweist, dass die ZWEITE
/// Spawn-Site (Mutation, neben Bootstrap aus T4) den `cell_dir`-Param durch
/// den Production-Path trägt.
///
/// KEIN Restart in diesem Test — RespawnFn-Maschinerie ist spawn-site-
/// independent (von T4 bereits via Bootstrap-Pfad bewiesen).
///
/// Topologie:
///   td.path()/main/config.json   (hive, root — einziges initiales FS-Item;
///                                 spec overview Z.331: root-cell-dir `main`
///                                 maps to logical `/`, its name stripped)
///   /sink                        (CaptureCell, direkt via h.spawn VOR Mutation
///                                 registriert — Anti-Cascade)
///   /persist                     (wird via Mutation `add_nodes` unter scope `/`
///                                 erzeugt; logischer Pfad `/persist`, on-disk
///                                 final_path = td.path()/main/persist — der
///                                 root-cell-dir `main` ist das FS-Anchor)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_production_mutation_spawn_with_state() {
    let td = tempfile::TempDir::new().unwrap();

    // FS-Tree: NUR root-hive. KEIN /persist — der entsteht erst durch die
    // Mutation via apply_mutation-staging + atomic_rename.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // Factory + Registry: Arc-share — eine Kopie in ColonyHandle (für
    // Mutation-Spawn-Site), eine in CellFactoryRegistry (für bootstrap der
    // Root-Hive, die für sich genommen keine Cell-Spawn auslöst, weil
    // type=hive — aber bootstrap_from_filesystem läuft trotzdem zur
    // Registry-Validation).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // /sink VOR Mutation registrieren (Anti-Cascade: /main/persist's echo_to
    // muss bei der ersten Emission resolved sein).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap nur die root-hive (registriert keine Cell — type=hive ist
    // Scope-Marker; Factory wird via Registry nicht aufgerufen).
    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap of root-hive must succeed");

    // Pre-Mutation-Assert: cell.db existiert NOCH NICHT — der on-disk
    // persist-Pfad ({root}/main/persist) ist noch nicht angelegt.
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

    // Phase-11 T16: Template-Verzeichnis für persist_mock anlegen und laden.
    {
        let templates_root = td.path().join("templates");
        let tpl = templates_root.join("persist_mock");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"persist_mock"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
    // override_params: {echo_to: "/sink"}}] unter scope `/` (spec overview
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
                    "override_params": {"echo_to": "/sink"}
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

    // Post-Wake-Assert (Kern-Beweis T5): cell.db lebt an
    // td.path()/main/persist/cell.db. Das ist nur möglich, wenn
    // handle_mutation seinen `sd.final_path` korrekt als `cell_dir` an
    // factory.spawn_cell durchgereicht hat (Phase-7.5 T1-Substrat) UND der
    // WakeFn-Closure den Pfad korrekt captured hat (Phase-13-K-2).
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
