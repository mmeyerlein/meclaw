//! Phase-16 W1b A5b (Ruling 2026-06-12, test a): on a REBOOT an unknown cell
//! directory (a `config.json` node that was never instantiated/mutated, e.g.
//! manually placed) is REPORTED, never adopted. Registration is
//! instantiation/mutation-only — the reboot walk never mints a cell_id for a
//! discovered dir. Receipt: after a clean first boot, a foreign cell dir added
//! to the live tree is NOT in the registry after the reboot, and the reboot
//! still succeeds (the foreign node is reported via a WARN, not a boot fail).
//!
//! Boot harness mirrors `bootstrap_crash_recovery.rs` (colony task + filesystem
//! bootstrap as join handles, clean drain on shutdown).

use meclaw_colony::{
    BootState, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime, bootstrap_from_filesystem,
    colony_task, probe_boot_state,
};
use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    f
}

/// Clean first-boot topology: root hive wiring `./a → ./b`, two connected echo
/// cells (both active). The hive graph edge + scope populate edges/hive_scopes
/// so the next boot classifies as a Reboot.
fn write_clean_tree(root: &std::path::Path) {
    let demo = root.join("demo");
    std::fs::create_dir_all(demo.join("a")).unwrap();
    std::fs::create_dir_all(demo.join("b")).unwrap();
    std::fs::write(
        demo.join("config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        demo.join("a/config.json"),
        format!(r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"./b"}},{CONTRACT}}}"#),
    )
    .unwrap();
    std::fs::write(
        demo.join("b/config.json"),
        format!(r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink"}},{CONTRACT}}}"#),
    )
    .unwrap();
}

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
            .expect("bootstrap must succeed")
    });
    (inbox_tx, colony_join, apply_join)
}

async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), colony_join).await;
}

fn registry_has(db_path: &std::path::Path, path: &str) -> bool {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM registry WHERE path = ?1",
        [path],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// Boot 1 (FirstBoot, clean) → registry populated; add `/foreign`; Boot 2
/// (Reboot) → `/foreign` is NOT registered (reported, not adopted) and the boot
/// still succeeds; the known cells stay registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reboot_reports_unknown_cell_dir_without_registering_it() {
    let td = tempfile::TempDir::new().unwrap();
    write_clean_tree(td.path());
    let db_path = td.path().join("colony.db");

    // --- Boot 1: FirstBoot — the walk is the source, both cells register. ---
    let (inbox1, colony1, apply1) = boot(td.path());
    let report1 = apply1.await.expect("boot 1 apply join");
    assert_eq!(report1.cell_count, 2, "first boot registers /a and /b");
    shutdown(inbox1, colony1).await;
    assert_eq!(
        probe_boot_state(&db_path).expect("probe"),
        BootState::Reboot,
        "after a completed first apply the next boot classifies as Reboot"
    );

    // --- A foreign cell dir is placed into the live tree (never mutated in). ---
    std::fs::create_dir_all(td.path().join("demo/foreign")).unwrap();
    std::fs::write(
        td.path().join("demo/foreign/config.json"),
        format!(r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink"}},{CONTRACT}}}"#),
    )
    .unwrap();

    // --- Boot 2: Reboot — /foreign is reported (WARN), NOT adopted. ---
    let (inbox2, colony2, apply2) = boot(td.path());
    let report2 = apply2
        .await
        .expect("boot 2 must succeed — an unknown cell dir does not fail the boot");
    // Only the two known (rehydrated) cells are applied; /foreign is diverted.
    assert_eq!(
        report2.cell_count, 2,
        "reboot applies only the registered cells; the foreign dir is not adopted"
    );
    shutdown(inbox2, colony2).await;

    // --- Receipt: /foreign never entered the registry; /a and /b did. ---
    assert!(
        !registry_has(&db_path, "/foreign"),
        "unknown cell dir must NOT be registered on reboot (no adoption)"
    );
    assert!(registry_has(&db_path, "/a"), "/a stays registered");
    assert!(registry_has(&db_path, "/b"), "/b stays registered");
}
