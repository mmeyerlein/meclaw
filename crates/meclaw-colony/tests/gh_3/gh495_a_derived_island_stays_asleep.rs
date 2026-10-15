//! GH #495: connectivity-derived inactivity is written down, so an island is
//! still an island on the second boot.
//!
//! Activity in this substrate is DERIVED — a node is active iff it is connected
//! and its parent hive is active (`docs/meclaw-overview.md` § Connectivity and
//! activity). The derivation ran correctly and was then thrown away: only a
//! DECLARED sleep (`birth: "inactive"`, GH #437/#491) reached the persisted
//! `registry.status` column. `UpsertRegistry` seeds `'active'` on INSERT and
//! never touches the column on conflict, and the boot-time recompute in
//! `bootstrap.rs` runs only for nodes WITHOUT a registry row — so the first boot
//! of an island left the seeded `'active'` standing, the second boot read it
//! back as the truth, and every island came up awake. For an eager long-running
//! kind that is a real subprocess, a real connection, a real poller that nothing
//! is wired to.
//!
//! The rule this file pins (ADR-0018): **a registration writes down the activity
//! it registers, and a recompute writes down every flip.** Both halves are
//! needed — the recompute persists TRANSITIONS, and a node registered inactive
//! and recomputed inactive has none.
//!
//! Unchanged by design, and guarded here:
//! - the Instanziierungs-Grace (GH #89) — an edge-less cell keeps its
//!   instantiation activity and boots ACTIVE, on every boot;
//! - the durable dormancy marker of ADR-0017 — a DERIVED sleep gets no marker
//!   and stays wakeable by any recompute that finds it connected, which is what
//!   the third boot below measures.
//!
//! Proof discipline: `spawn_count` is bumped ONLY inside `spawn_cell`, the real
//! build-and-spawn path, and never inside `build_boot_inactive_respawn`. So the
//! counter is a POSITIVE receipt for tasks actually built — the island's cells
//! are counted iff they really run.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime, DbConn,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task_long_running, colony_task,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

struct EagerLrFactory {
    spawn_count: Arc<AtomicU32>,
}

fn make_build(
    spawn_count: Arc<AtomicU32>,
    path: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<ColonyMsg>,
) -> impl Fn() -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    move || {
        spawn_count.fetch_add(1, Ordering::SeqCst);
        let (cell, inject_tx) = meclaw_testing::mocks::ReceiptMockLongRunningCell::new();
        let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
        let db = DbConn::wrap(conn, None);
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let (peace_tx, peace_rx) = oneshot::channel();
        let (_backstop_tx, backstop_rx) = oneshot::channel();
        let p = path.clone();
        let o = outputs_tx.clone();
        let cit = colony_inbox_tx.clone();
        let join = tokio::spawn(async move {
            let _keep_inject = inject_tx;
            cell_task_long_running(
                p,
                rx,
                o,
                64,
                cell,
                db,
                Some(peace_tx),
                Some(cit),
                None,
                None,
                None,
                None,
                Default::default(),
            )
            .await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for EagerLrFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let build = make_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let _ = &death_ack_tx;
        let respawn: RespawnFn = Box::new(build);
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let build = make_build(self.spawn_count.clone(), path, outputs_tx, colony_inbox_tx);
        Some(Box::new(build))
    }
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const CONTRACT: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

/// `/a -> /b` on the mainland; `/isle` is a hive whose only edge is internal
/// (`/isle/x -> /isle/y`), so it has no external edge and its subtree derives
/// INACTIVE at first boot.
fn write_island_topology(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
    );
    write(
        root,
        "main/a/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
    write(
        root,
        "main/b/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
    write(
        root,
        "main/isle/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./x","to":"./y"}]}}}"#,
    );
    write(
        root,
        "main/isle/x/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
    write(
        root,
        "main/isle/y/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
    // One template, so a mutation can grow a cell INTO the sleeping island.
    write(root, "templates/leaf/template.json", r#"{"name":"leaf"}"#);
    write(
        root,
        "templates/leaf/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
}

fn factories(spawn_count: Arc<AtomicU32>) -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "lr".into(),
        Arc::new(EagerLrFactory { spawn_count }) as Arc<dyn CellFactory>,
    );
    f
}

fn boot(
    root: &std::path::Path,
    spawn_count: Arc<AtomicU32>,
) -> (
    mpsc::Sender<ColonyMsg>,
    JoinHandle<()>,
    JoinHandle<meclaw_colony::BootstrapReport>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let f = factories(spawn_count);
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

async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx).await;
    tokio::time::timeout(std::time::Duration::from_secs(30), colony_join)
        .await
        .expect("colony shutdown hung")
        .expect("colony task must not panic");
}

async fn ram_entry(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    inbox_tx
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
    tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("ReadRegistry ack timed out")
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

fn db_status(db_path: &std::path::Path, path: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get(0)
    })
    .ok()
}

async fn send_mutation(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    payload: JsonValue,
) -> meclaw_colony::MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack timed out")
        .unwrap()
}

/// The island shape, booted twice. Boot 1 derives `/isle/*` inactive and spawns
/// only the mainland; boot 2 must reach exactly the same state — the derivation
/// has to survive in `colony.db`, because boot 2 does not re-derive it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_island_derived_asleep_is_still_asleep_on_the_second_boot() {
    let td = tempfile::TempDir::new().unwrap();
    write_island_topology(td.path());
    let db_path = td.path().join("colony.db");

    // --- Boot 1: the derivation happens here, for the only time. ---
    let sc1 = Arc::new(AtomicU32::new(0));
    let (inbox1, colony1, apply1) = boot(td.path(), sc1.clone());
    apply1.await.expect("boot-1 apply must not panic");
    for isle in ["/isle/x", "/isle/y"] {
        let e = ram_entry(&inbox1, isle)
            .await
            .expect("island cell registered");
        assert!(
            !e.active,
            "boot 1 derives {isle} inactive (no external edge)"
        );
        assert_eq!(
            e.lifecycle_status, "NotYetSpawned",
            "{isle} must not have a running task at boot"
        );
    }
    assert!(
        ram_entry(&inbox1, "/a").await.expect("/a").active,
        "the mainland is active — the island is an island, not a broken boot"
    );
    assert_eq!(
        sc1.load(Ordering::SeqCst),
        2,
        "exactly the two mainland cells were built"
    );
    shutdown(inbox1, colony1).await;

    // --- The receipt the defect was about: the row, not the RAM. ---
    assert_eq!(
        db_status(&db_path, "/isle/x").as_deref(),
        Some("inactive"),
        "GH #495: the DERIVED inactivity must reach the registry row — before \
         the fix `UpsertRegistry`'s seeded `'active'` stood here"
    );
    assert_eq!(
        db_status(&db_path, "/isle/y").as_deref(),
        Some("inactive"),
        "the whole island, not just the edge endpoint"
    );
    assert_eq!(
        db_status(&db_path, "/a").as_deref(),
        Some("active"),
        "the mainland row is untouched"
    );

    // --- Boot 2: reads the row. It must read the truth. ---
    let sc2 = Arc::new(AtomicU32::new(0));
    let (inbox2, colony2, apply2) = boot(td.path(), sc2.clone());
    apply2.await.expect("boot-2 apply must not panic");
    for isle in ["/isle/x", "/isle/y"] {
        let e = ram_entry(&inbox2, isle)
            .await
            .expect("island cell registered");
        assert!(!e.active, "the second boot must not wake {isle}");
        assert_eq!(
            e.lifecycle_status, "NotYetSpawned",
            "and it must not build a task for {isle} either"
        );
    }
    assert_eq!(
        sc2.load(Ordering::SeqCst),
        2,
        "GH #495: the second boot builds the two mainland tasks and no more — \
         before the fix it built four and the island was live"
    );
    shutdown(inbox2, colony2).await;
    assert_eq!(
        db_status(&db_path, "/isle/x").as_deref(),
        Some("inactive"),
        "state stable across the reboot"
    );
}

/// The other half of the promise: a derived sleep carries no durable marker, so
/// the ordinary wake still works — an edge that reaches the island activates it,
/// and the third boot rehydrates the woken colony.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_into_the_island_wakes_it_and_the_third_boot_agrees() {
    let td = tempfile::TempDir::new().unwrap();
    write_island_topology(td.path());
    let db_path = td.path().join("colony.db");

    // --- Boot 1 + boot 2: asleep, as the test above pins in detail. ---
    let sc1 = Arc::new(AtomicU32::new(0));
    let (inbox1, colony1, apply1) = boot(td.path(), sc1.clone());
    apply1.await.expect("boot-1 apply");
    shutdown(inbox1, colony1).await;

    let sc2 = Arc::new(AtomicU32::new(0));
    let (inbox2, colony2, apply2) = boot(td.path(), sc2.clone());
    apply2.await.expect("boot-2 apply");
    assert!(
        !ram_entry(&inbox2, "/isle/x").await.expect("/isle/x").active,
        "precondition: the island is asleep on the second boot"
    );

    // --- The wake: one edge from the mainland into the island. It crosses the
    //     `/isle` unit boundary, so the hive has an external edge and its
    //     subtree derives active again. No marker stands in the way — a derived
    //     sleep never gets one (ADR-0017 § 4 / ADR-0018).
    let outcome = send_mutation(
        &inbox2,
        meclaw_core::serde_json::json!({"scope":"/","ctx":{},"diff":{
            "add_edges":[{"from":"./a","to":"./isle/x"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the wake mutation must commit: {outcome:?}"
    );
    for isle in ["/isle/x", "/isle/y"] {
        assert!(
            ram_entry(&inbox2, isle).await.expect(isle).active,
            "the crossing edge activates the whole island subtree ({isle})"
        );
    }
    assert_eq!(
        sc2.load(Ordering::SeqCst),
        4,
        "the reconnect eager-spawns the two island tasks on top of the mainland"
    );
    shutdown(inbox2, colony2).await;
    assert_eq!(
        db_status(&db_path, "/isle/x").as_deref(),
        Some("active"),
        "the reconnect flip is persisted (step 10b writes transitions)"
    );

    // --- Boot 3: the woken colony comes back woken. ---
    let sc3 = Arc::new(AtomicU32::new(0));
    let (inbox3, colony3, apply3) = boot(td.path(), sc3.clone());
    apply3.await.expect("boot-3 apply");
    for isle in ["/isle/x", "/isle/y"] {
        let e = ram_entry(&inbox3, isle).await.expect(isle);
        assert!(e.active, "{isle} was woken deliberately and stays awake");
    }
    assert_eq!(
        sc3.load(Ordering::SeqCst),
        4,
        "all four cells run after the third boot"
    );
    shutdown(inbox3, colony3).await;
}

/// Gegenprobe (GH #89, unchanged): an edge-less cell is NOT an island. It keeps
/// its instantiation activity, boots ACTIVE, and boots active again — the widened
/// persistence must not reach it, because it never registers inactive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edgeless_cell_keeps_its_grace_across_a_reboot() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/solo/config.json",
        &format!(r#"{{"cell":{{"type":"lr"}},"params":{{}},{CONTRACT}}}"#),
    );
    let db_path = td.path().join("colony.db");

    let sc1 = Arc::new(AtomicU32::new(0));
    let (inbox1, colony1, apply1) = boot(td.path(), sc1.clone());
    apply1.await.expect("boot-1 apply");
    assert!(
        ram_entry(&inbox1, "/solo").await.expect("/solo").active,
        "Grace: an edge-less cell keeps its instantiation activity"
    );
    assert_eq!(sc1.load(Ordering::SeqCst), 1, "and its task really runs");
    shutdown(inbox1, colony1).await;
    assert_eq!(
        db_status(&db_path, "/solo").as_deref(),
        Some("active"),
        "the Grace row stays `active` — nothing here registered inactive"
    );

    let sc2 = Arc::new(AtomicU32::new(0));
    let (inbox2, colony2, apply2) = boot(td.path(), sc2.clone());
    apply2.await.expect("boot-2 apply");
    assert!(
        ram_entry(&inbox2, "/solo").await.expect("/solo").active,
        "and the second boot grants it the same Grace"
    );
    assert_eq!(sc2.load(Ordering::SeqCst), 1, "task runs on boot 2 as well");
    shutdown(inbox2, colony2).await;
}

/// The mutation half of the same defect. A cell grown INTO the sleeping island
/// is connected (the diff wires it) but derives inactive, because its parent
/// hive is. The apply's activity gate registers it `active: false` and does not
/// build its task — and step 10b, which persists TRANSITIONS, sees none: the
/// entry is already `false` and the recompute agrees. So the row has to be
/// written at registration, or the next boot starts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_grown_into_a_sleeping_island_is_persisted_asleep() {
    let td = tempfile::TempDir::new().unwrap();
    write_island_topology(td.path());
    let db_path = td.path().join("colony.db");

    let sc1 = Arc::new(AtomicU32::new(0));
    let (inbox1, colony1, apply1) = boot(td.path(), sc1.clone());
    apply1.await.expect("boot-1 apply");
    let (rs_tx, rs_rx) = oneshot::channel();
    inbox1
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: rs_tx,
        })
        .await
        .unwrap();
    rs_rx.await.unwrap().expect("template rescan");

    let outcome = send_mutation(
        &inbox1,
        meclaw_core::serde_json::json!({"scope":"/isle","ctx":{},"diff":{
            "add_nodes":[{"name":"late","template":"leaf"}],
            "add_edges":[{"from":"./x","to":"./late"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "growing into a sleeping unit is legal and commits: {outcome:?}"
    );
    let e = ram_entry(&inbox1, "/isle/late").await.expect("/isle/late");
    assert!(!e.active, "the new cell inherits the island's sleep");
    assert_eq!(
        sc1.load(Ordering::SeqCst),
        2,
        "the activity gate built no task for it (still just the mainland)"
    );
    shutdown(inbox1, colony1).await;
    assert_eq!(
        db_status(&db_path, "/isle/late").as_deref(),
        Some("inactive"),
        "GH #495: the apply writes down what it registered — before the fix only \
         a DECLARED `birth: \"inactive\"` reached the row"
    );

    let sc2 = Arc::new(AtomicU32::new(0));
    let (inbox2, colony2, apply2) = boot(td.path(), sc2.clone());
    apply2.await.expect("boot-2 apply");
    assert!(
        !ram_entry(&inbox2, "/isle/late")
            .await
            .expect("/isle/late")
            .active,
        "and the next boot leaves it asleep"
    );
    assert_eq!(
        sc2.load(Ordering::SeqCst),
        2,
        "no task for the grown cell on the second boot either"
    );
    shutdown(inbox2, colony2).await;
}
