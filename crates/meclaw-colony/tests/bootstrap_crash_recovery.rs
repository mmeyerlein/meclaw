//! Bootstrap-apply crash atomicity (data-integrity finding run 5/5b, reproduced
//! 2×): the workspace directory is missing → `--validate` PASS (U11 shape:
//! validate does not check the path), the boot panics MID-apply at the
//! `bootstrap_apply.rs` `.expect("validated in plan-phase")` anchor → colony.db
//! stays in a mixed state (registry>0 / edges=0 / hive_scopes=0) → the following
//! boot ran into the `Inconsistent` panic, without a healing path.
//!
//! Contract under test (spec § Startup algorithm, bootstrap recovery): an
//! interrupted FIRST apply heals itself on the next boot — a deterministic
//! rebuild from the filesystem (the FS is the source), NEVER an `Inconsistent`
//! panic, no operator "delete the DB". Mechanics: a durable
//! `bootstrap_in_flight` marker (colony.db `meta`) before the first spawn,
//! cleared atomically in the `InitialApply` transaction; `probe_boot_state`
//! classifies marker states as `FirstBoot` (idempotent resume).
//! Since GH #89 the marker-less cut is: InitialApply traces (edges OR
//! hive_scopes non-empty) classify `Reboot`; registry-only states classify
//! `FirstBoot` (runtime spawns / apply torso — idempotent re-adoption). Pins:
//! `boot_state_edges_only_without_marker_returns_reboot`,
//! `boot_state_registry_only_without_marker_returns_first_boot` in
//! `bootstrap.rs`.

use meclaw_colony::{
    BootState, CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    ContractView, SpawnedCellKind, bootstrap_from_filesystem, colony_task, probe_boot_state,
};
use meclaw_core::{CellEmission, JsonValue, Path};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// U11-shape factory: `validate_params` passes WITHOUT checking the
/// `params.requires` directory; `spawn_cell` fails when it is missing (the
/// canonicalize-at-spawn asymmetry that produced the Run-5 boot panic). On
/// success it delegates to the echo factory (a real, working cell task).
struct RequiresDirFactory {
    echo: Arc<EchoCellFactory>,
}

impl CellFactory for RequiresDirFactory {
    fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
        // U11 shape: the plan-phase does NOT probe the directory.
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let requires = params
            .get("requires")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !std::path::Path::new(&requires).exists() {
            return Err(format!("workspace dir missing: {requires}"));
        }
        self.echo.clone().spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

fn factories() -> CellFactoryRegistry {
    let echo = Arc::new(EchoCellFactory);
    let mut f = CellFactoryRegistry::new();
    f.insert("echo".into(), echo.clone() as Arc<dyn CellFactory>);
    f.insert(
        "requires-dir".into(),
        Arc::new(RequiresDirFactory { echo }) as Arc<dyn CellFactory>,
    );
    f
}

/// Topology: `main` hive (graph edge → edges/hive_scopes in the plan), an OK
/// echo cell, and a fragile cell NESTED UNDER the OK cell — the walk pushes a
/// directory before descending, so the OK cell registers (registry rows
/// committed) BEFORE the fragile spawn panics. That is the Run-5b mixed state.
fn write_topology(root: &std::path::Path, workspace: &std::path::Path) {
    let contract = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;
    let main = root.join("main");
    std::fs::create_dir_all(main.join("ok/fragile")).unwrap();
    // A8 (Phase-16 W1a): the boot graph edge must resolve — the old `./ok →
    // /sink` dangled (`/sink` is neither an FS cell nor registered) and would
    // now LOUD-fail the boot before the U11 spawn gap is reached. A7 (no boot
    // grace): the fragile cell must be ACTIVE to be spawned at all. Both are
    // satisfied by wiring `./ok → ./ok/fragile` (both endpoints are plan cells):
    // `/ok` stays connected/active and `/ok/fragile` is connected → its spawn is
    // attempted at apply → the requires-dir panic produces the mixed state.
    std::fs::write(
        main.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./ok","to":"./ok/fragile"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        main.join("ok/config.json"),
        format!(r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},{contract}}}"#),
    )
    .unwrap();
    std::fs::write(
        main.join("ok/fragile/config.json"),
        format!(
            r#"{{"cell":{{"type":"requires-dir"}},"params":{{"echo_to":"/sink","requires":"{}"}},{contract}}}"#,
            workspace.display()
        ),
    )
    .unwrap();
}

/// One boot: colony task + filesystem bootstrap, both as join handles so the
/// test can observe a mid-apply panic without dying itself.
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
            .expect("plan-phase must validate (U11 shape: the gap is at spawn)")
    });
    (inbox_tx, colony_join, apply_join)
}

/// Clean shutdown: drains + flushes the writer so the on-disk colony.db shows
/// everything that was committed (the real crash had its rows committed too —
/// Run-5b receipt: registry=3/edges=0/hive_scopes=0).
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

fn bootstrap_marker_present(db_path: &std::path::Path) -> bool {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM meta WHERE key='bootstrap_in_flight'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// The repro anchor + healing path in one run:
/// boot 1 (workspace missing) → apply panic mid-apply → mixed state;
/// boot 2 (workspace repaired) → MUST heal (FirstBoot resume from the FS),
/// NEVER an `Inconsistent` panic; the boot-3 classification is `Reboot`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_first_apply_heals_on_reboot_never_inconsistent() {
    let td = tempfile::TempDir::new().unwrap();
    let workspace = td.path().join("workspace");
    write_topology(td.path(), &workspace);
    let db_path = td.path().join("colony.db");

    // --- Boot 1: workspace missing → spawn fails → apply panics mid-apply. ---
    let (inbox1, colony1, apply1) = boot(td.path());
    let apply_err = apply1.await;
    assert!(
        apply_err.is_err(),
        "crash injection: the apply task must panic at the \
         `.expect(\"validated in plan-phase\")` anchor"
    );
    // The colony task survived the apply panic — flush everything it committed
    // (worst case for atomicity: all Register upserts are on disk).
    shutdown(inbox1, colony1)
        .await
        .expect("boot-1 colony must shut down cleanly");

    // --- Repro anchor: the run-5b mixed state is on disk. ---
    let (reg, edges, scopes) = table_counts(&db_path);
    assert!(
        reg > 0,
        "pre-condition broke: the OK cell must have registered before the \
         fragile spawn panicked (walk pushes parent before child)"
    );
    assert_eq!(edges, 0, "InitialApply never ran — edges must be empty");
    assert_eq!(
        scopes, 0,
        "InitialApply never ran — hive_scopes must be empty"
    );
    assert!(
        bootstrap_marker_present(&db_path),
        "the interrupted first apply must leave the durable \
         bootstrap_in_flight marker in colony.db meta"
    );

    // --- Repair the filesystem (the workspace dir exists now). ---
    std::fs::create_dir_all(&workspace).unwrap();

    // --- Boot 2: MUST heal — FirstBoot resume from the filesystem. ---
    let (inbox2, colony2, apply2) = boot(td.path());
    let report = apply2.await.expect(
        "boot 2 must NOT panic: an interrupted first apply heals via the \
         bootstrap_in_flight marker (deterministic rebuild from the filesystem)",
    );
    assert_eq!(report.cell_count, 2, "ok + fragile cells applied");
    shutdown(inbox2, colony2)
        .await
        .expect("boot-2 colony must NOT have died with an Inconsistent panic");

    // --- Consistency receipts: full state, marker cleared. ---
    let (reg, edges, scopes) = table_counts(&db_path);
    assert_eq!(reg, 2, "both cells registered");
    assert_eq!(edges, 1, "hive graph edge committed via InitialApply");
    assert_eq!(scopes, 1, "hive scope committed via InitialApply");
    assert!(
        !bootstrap_marker_present(&db_path),
        "the InitialApply transaction must clear the marker atomically"
    );

    // --- Boot 3 classification: a completed first apply is a normal Reboot. ---
    assert_eq!(
        probe_boot_state(&db_path).expect("probe"),
        BootState::Reboot,
        "after the healed apply the next boot classifies as Reboot"
    );
}
