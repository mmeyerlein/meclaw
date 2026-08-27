//! GH #437 — "grow it, but do not start it".
//!
//! The case that produced the issue: a long-poll consumer whose upstream
//! permits exactly one consumer. Grown into a live colony it used to start
//! polling at birth and steal the upstream from the running one; the workaround
//! was pointing `base_url` at an unroutable address and restoring the template
//! default after shutdown.
//!
//! The proof is POSITIVE on both halves, and the discriminator is a counter
//! that is bumped ONLY inside the real `spawn_cell` — never inside
//! `build_boot_inactive_respawn` — so `spawn_count == baseline` means no task
//! was ever built, not merely that no side effect was observed. That is the
//! same discipline `paket_3_c1_activity_aware_spawn` established for the
//! activity gate this declaration now joins.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind,
    cell_task_long_running,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ───────────────────────────────────────────────────────────────────────────
// An EAGER long-running factory. `spawn_count` is bumped ONLY in the real
// build path, so `0` is a positive proof that no task was ever constructed.
// ───────────────────────────────────────────────────────────────────────────

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
        Some(Box::new(make_build(
            self.spawn_count.clone(),
            path,
            outputs_tx,
            colony_inbox_tx,
        )))
    }
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
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

async fn rescan(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 500,
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

/// What `colony.db` says — the answer the NEXT boot will read.
fn registry_status_in_db(root: &std::path::Path, path: &str) -> String {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("open colony.db");
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_else(|e| panic!("no registry row for {path}: {e}"))
}

/// A root hive plus a single-cell `poller` template (long-running, persistent)
/// and a `unit` subtree template that contains one.
fn setup(dir: &std::path::Path) {
    write(
        dir,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        dir,
        "templates/poller/template.json",
        r#"{"name":"poller"}"#,
    );
    write(
        dir,
        "templates/poller/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(dir, "templates/unit/template.json", r#"{"name":"unit"}"#);
    write(
        dir,
        "templates/unit/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    for leaf in ["poll", "work"] {
        write(
            dir,
            &format!("templates/unit/{leaf}/config.json"),
            r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
    }
}

fn world(td: &tempfile::TempDir, counter: &Arc<AtomicU32>) -> ColonyHandle {
    let factory: Arc<dyn CellFactory> = Arc::new(EagerLrFactory {
        spawn_count: counter.clone(),
    });
    ColonyHandle::new_with_factories_at(td, vec![("lr".to_string(), factory)])
}

// ───────────────────────────── Task 8: one cell ────────────────────────────

/// A long-running cell declared `birth: "inactive"` is registered, addressable
/// and persisted inactive — and its task is never built.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_running_cell_born_inactive_is_registered_and_never_spawns() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = world(&td, &spawn_count);
    rescan(&h, td.path().join("templates")).await;
    let baseline = spawn_count.load(Ordering::SeqCst);

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_nodes": [
                {"name": "poller", "template": "poller", "birth": "inactive"}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a born-inactive add_nodes commits: {outcome:?}"
    );

    // Positive half 1: it EXISTS, at its address, inactive.
    let e = registry_entry(&h, "/poller")
        .await
        .expect("the node must be in the registry");
    assert!(!e.active, "it is registered inactive: {e:?}");
    assert_eq!(e.lifecycle_status, "NotYetSpawned");

    // Positive half 2: no task was ever built. The counter is bumped inside the
    // real build path only, so this is a statement about the task, not about a
    // side effect that merely did not show up in time.
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        baseline,
        "a cell born inactive must not have had its task built"
    );

    // And the DB agrees, so a reboot agrees too.
    h.shutdown().await;
    assert_eq!(registry_status_in_db(td.path(), "/poller"), "inactive");
}

/// The shipped default is unchanged: no declaration, no change of behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_without_a_birth_declaration_still_starts() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = world(&td, &spawn_count);
    rescan(&h, td.path().join("templates")).await;
    let baseline = spawn_count.load(Ordering::SeqCst);

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_nodes": [{"name": "poller", "template": "poller"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert!(
        spawn_count.load(Ordering::SeqCst) > baseline,
        "the shipped default must still build the task"
    );
    let e = registry_entry(&h, "/poller").await.expect("registered");
    assert!(e.active, "the default is active");
    h.shutdown().await;
}

// ─────────────────────────── Task 9: a whole subtree ───────────────────────

/// A unit is born whole: a subtree declared inactive brings up no task
/// anywhere inside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subtree_born_inactive_starts_nothing_inside_it() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = world(&td, &spawn_count);
    rescan(&h, td.path().join("templates")).await;
    let baseline = spawn_count.load(Ordering::SeqCst);

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_nodes": [
                {"name": "unit", "template": "unit", "birth": "inactive"}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a born-inactive subtree commits: {outcome:?}"
    );

    for p in ["/unit/poll", "/unit/work"] {
        let e = registry_entry(&h, p)
            .await
            .unwrap_or_else(|| panic!("{p} must be registered"));
        assert!(!e.active, "{p} must be registered inactive: {e:?}");
    }
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        baseline,
        "nothing inside a subtree born inactive may have its task built"
    );

    h.shutdown().await;
    for p in ["/unit/poll", "/unit/work"] {
        assert_eq!(
            registry_status_in_db(td.path(), p),
            "inactive",
            "{p} must survive a reboot inactive"
        );
    }
}

// ──────────────── Task 10: the birth mutation does not wake it ─────────────

/// The declaration is the last word about this node IN THE MUTATION THAT GIVES
/// BIRTH TO IT. Wiring it in the same diff is the normal case — you grow a cell
/// where it belongs — and it must not start it. Otherwise the declaration would
/// only work for cells nobody connected, which is the state the issue already
/// had.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wiring_a_born_inactive_cell_in_the_same_diff_does_not_start_it() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = world(&td, &spawn_count);
    rescan(&h, td.path().join("templates")).await;

    // An ingress that IS started, so the diff wires from something live.
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_nodes": [{"name": "ingress", "template": "poller"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let baseline = spawn_count.load(Ordering::SeqCst);

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {
                "add_nodes": [{"name": "poller", "template": "poller", "birth": "inactive"}],
                "add_edges": [{"from": "./ingress", "to": "./poller"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "wiring and sleeping in one diff commits: {outcome:?}"
    );

    let e = registry_entry(&h, "/poller").await.expect("registered");
    assert!(
        !e.active,
        "the birth declaration wins over the recompute of its own mutation: {e:?}"
    );
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        baseline,
        "the wired-and-sleeping cell must not have had its task built"
    );
    h.shutdown().await;
    assert_eq!(registry_status_in_db(td.path(), "/poller"), "inactive");
}

/// The wake is the EXISTING reconnect — no new op, no new message.
/// `docs/meclaw-overview.md` § Reconnect: a node receives an edge again
/// (typically via `add_edges` or a renewed `add_nodes` at the existing path)
/// → the long-running cells of the reactivated subtree start immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_mutation_that_reaches_it_wakes_it() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = world(&td, &spawn_count);
    rescan(&h, td.path().join("templates")).await;

    let _ = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_nodes": [
                {"name": "ingress", "template": "poller"},
                {"name": "ingress2", "template": "poller"}
            ]}
        }),
    )
    .await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {
                "add_nodes": [{"name": "poller", "template": "poller", "birth": "inactive"}],
                "add_edges": [{"from": "./ingress", "to": "./poller"}]
            }
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let asleep = spawn_count.load(Ordering::SeqCst);

    // A SECOND mutation whose recompute reaches the node: a renewed edge.
    let woken = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {"add_edges": [{"from": "./ingress2", "to": "./poller"}]}
        }),
    )
    .await;
    assert!(
        matches!(woken, MutationOutcome::Committed { .. }),
        "the reconnect commits: {woken:?}"
    );

    // Positive receipt: the task was built by the ordinary reconnect.
    assert!(
        spawn_count.load(Ordering::SeqCst) > asleep,
        "the ordinary reconnect must start it — no new operation needed"
    );
    let e = registry_entry(&h, "/poller").await.expect("registered");
    assert!(e.active, "the woken node is active: {e:?}");
    h.shutdown().await;
    assert_eq!(registry_status_in_db(td.path(), "/poller"), "active");
}
