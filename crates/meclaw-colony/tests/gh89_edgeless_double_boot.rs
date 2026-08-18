//! GH #89: an edge-less but healthy workspace boots twice.
//!
//! Repro (S3 verification runs, 2026-08-13): a topology with cells but no
//! edges (registry>0, edges=0, hive_scopes>0) boots fine the first time; the
//! second boot of the SAME workspace used to classify that combination as
//! `Inconsistent` and panic ("inconsistent colony.db: table counts"), although
//! the workspace is exactly what the first boot wrote. Same for a hive-only
//! colony (0/0/1).
//!
//! Contract under test (Ruling F2-R1, spec § Startup-Algorithmus): edge-less
//! and cell-less persisted shapes are LEGITIMATE (the spec grants edge-less
//! single cells their instantiation activity; a hive marker alone is a valid
//! colony). The boot-state probe classifies any non-empty, marker-less
//! colony.db as `Reboot` — never `Inconsistent`. Real corruption stays loud at
//! the schema/read layer (edge/hive_scope hydration hard-fails, pinned in
//! `phase_16_hive_scope_hydration_hard_fail.rs` and the durable-edges demo)
//! and probe-level only for unreadable tables (unit pin in `bootstrap.rs`).

use meclaw_colony::{
    BootState, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    bootstrap_from_filesystem, colony_task, probe_boot_state,
};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
    );
    f
}

/// The #89 shape: a hive root plus ONE cell, no edges anywhere.
fn write_edgeless_topology(root: &std::path::Path) {
    let main = root.join("main");
    std::fs::create_dir_all(main.join("solo")).unwrap();
    std::fs::write(main.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        main.join("solo/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/solo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// The hive-only shape: nothing but a hive marker (0/0/1 after boot 1).
fn write_hive_only_topology(root: &std::path::Path) {
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
}

/// One boot: colony task + filesystem bootstrap, both as join handles so the
/// test can observe a boot panic without dying itself (crash-recovery model).
fn boot(
    root: &std::path::Path,
) -> (
    mpsc::Sender<ColonyMsg>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<meclaw_colony::BootstrapReport>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let f = factories();
    let colony_join = tokio::spawn(colony_task(meclaw_colony::ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        f.clone(),
        root.to_path_buf(),
        ColonyConfig::default(),
        None,
        None,
    )));
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx,
        colony_config: ColonyConfig::default(),
        blob_store: None,
    };
    let root_owned = root.to_path_buf();
    let apply_join = tokio::spawn(async move {
        bootstrap_from_filesystem(&root_owned, &f, &runtime)
            .await
            .expect("bootstrap plan/apply must succeed on a healthy workspace")
    });
    (inbox_tx, colony_join, apply_join)
}

/// Clean shutdown: drains + flushes the writer so the on-disk colony.db shows
/// everything that was committed. A JoinError proves the colony task panicked.
async fn shutdown(
    inbox_tx: mpsc::Sender<ColonyMsg>,
    colony_join: tokio::task::JoinHandle<()>,
) -> Result<(), tokio::task::JoinError> {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), colony_join)
        .await
        .expect("colony shutdown hung")
}

fn table_counts(db_path: &std::path::Path) -> (i64, i64, i64) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or(0)
    };
    (count("registry"), count("edges"), count("hive_scopes"))
}

/// Boot 1 writes registry=1/edges=0/hive_scopes=1; boot 2 of the SAME
/// workspace must come up (Reboot), never the old `Inconsistent` panic.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edgeless_workspace_boots_twice() {
    let td = tempfile::TempDir::new().unwrap();
    write_edgeless_topology(td.path());
    let db_path = td.path().join("colony.db");

    // --- Boot 1: fresh workspace, plain FirstBoot. ---
    let (inbox1, colony1, apply1) = boot(td.path());
    apply1.await.expect("boot-1 apply task must not panic");
    shutdown(inbox1, colony1)
        .await
        .expect("boot-1 colony must shut down cleanly");

    // --- Pre-condition receipt: the #89 shape is on disk. ---
    let (reg, edges, scopes) = table_counts(&db_path);
    assert_eq!(reg, 1, "the solo cell registered");
    assert_eq!(edges, 0, "no edges anywhere — that is the point");
    assert_eq!(scopes, 1, "the hive scope persisted");
    assert_eq!(
        probe_boot_state(&db_path).expect("probe"),
        BootState::Reboot,
        "edge-less but healthy state must classify as Reboot"
    );

    // --- Boot 2: MUST come up — no `inconsistent colony.db` panic. ---
    let (inbox2, colony2, apply2) = boot(td.path());
    apply2
        .await
        .expect("boot-2 apply must not panic on the workspace boot 1 wrote");
    shutdown(inbox2, colony2)
        .await
        .expect("boot-2 colony must NOT have died with an Inconsistent panic");

    // --- Stability receipt: the reboot did not distort the persisted state. ---
    let (reg, edges, scopes) = table_counts(&db_path);
    assert_eq!(
        (reg, edges, scopes),
        (1, 0, 1),
        "state stable across reboot"
    );
}

/// A colony of nothing but a hive marker (0/0/1 after boot 1) boots twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_only_workspace_boots_twice() {
    let td = tempfile::TempDir::new().unwrap();
    write_hive_only_topology(td.path());
    let db_path = td.path().join("colony.db");

    // --- Boot 1. ---
    let (inbox1, colony1, apply1) = boot(td.path());
    let report = apply1.await.expect("boot-1 apply task must not panic");
    assert_eq!(report.cell_count, 0, "hive-only: no cells to apply");
    shutdown(inbox1, colony1)
        .await
        .expect("boot-1 colony must shut down cleanly");

    // --- Pre-condition receipt: the hive-only shape is on disk. ---
    let (reg, edges, scopes) = table_counts(&db_path);
    assert_eq!((reg, edges, scopes), (0, 0, 1), "hive-only persisted shape");
    assert_eq!(
        probe_boot_state(&db_path).expect("probe"),
        BootState::Reboot,
        "hive-only state must classify as Reboot"
    );

    // --- Boot 2: MUST come up. ---
    let (inbox2, colony2, apply2) = boot(td.path());
    apply2
        .await
        .expect("boot-2 apply must not panic on the workspace boot 1 wrote");
    shutdown(inbox2, colony2)
        .await
        .expect("boot-2 colony must NOT have died with an Inconsistent panic");

    let (reg, edges, scopes) = table_counts(&db_path);
    assert_eq!(
        (reg, edges, scopes),
        (0, 0, 1),
        "state stable across reboot"
    );
}
