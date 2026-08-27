//! GH #437 — the drift lock for the promise `templates/README.md` makes about
//! `birth` (`docs/development-rules.md` § 2d).
//!
//! § 2d: a countable or behaviour-describing promise on a public TEMPLATE
//! surface gets a Cargo test in the same commit that does BOTH halves — grep
//! the sentence AND assert the mechanism. A test that does only one half is not
//! a drift lock: the grep alone survives a substrate that stopped honouring the
//! sentence, and the mechanism alone survives a README that stopped making the
//! promise.
//!
//! The promise: "a cell born inactive is registered, addressable and persisted
//! inactive, and no task is built for it, not even when the same mutation wires
//! it. The next mutation that reaches it wakes it."

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

fn readme() -> String {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/README.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

// ── half 1: the sentence is there ──────────────────────────────────────────

#[test]
fn the_readme_still_makes_the_promise() {
    let text = readme();
    for needle in [
        r#"declare `"birth": "inactive"` on the `add_nodes` entry"#,
        "registered, addressable and persisted inactive",
        "no task is built for it, not even when",
        "The next mutation that reaches it wakes it.",
    ] {
        assert!(
            text.contains(needle),
            "templates/README.md must still say: {needle}"
        );
    }
}

// ── half 2: the substrate does what the sentence says ──────────────────────

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
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_substrate_keeps_the_readme_promise() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "templates/poller/template.json",
        r#"{"name":"poller"}"#,
    );
    write(
        td.path(),
        "templates/poller/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawn_count = Arc::new(AtomicU32::new(0));
    let factory: Arc<dyn CellFactory> = Arc::new(EagerLrFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), factory)]);
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");

    // An ingress that IS live, so the wiring below comes from something active.
    let _ = send_mutation(
        &h,
        json!({"scope": "/", "ctx": {},
               "diff": {"add_nodes": [{"name": "ingress", "template": "poller"}]}}),
    )
    .await;
    let baseline = spawn_count.load(Ordering::SeqCst);

    // "…not even when the same mutation wires it."
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/", "ctx": {},
            "diff": {
                "add_nodes": [{"name": "sleepy", "template": "poller", "birth": "inactive"}],
                "add_edges": [{"from": "./ingress", "to": "./sleepy"}]
            }
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    assert_eq!(
        spawn_count.load(Ordering::SeqCst),
        baseline,
        "no task may be built for a cell born inactive"
    );

    // "registered, addressable"
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
    let e = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == "/sleepy")
        .expect("the node must be registered");
    assert!(!e.active);

    // "The next mutation that reaches it wakes it."
    let _ = send_mutation(
        &h,
        json!({"scope": "/", "ctx": {},
               "diff": {"add_nodes": [{"name": "ingress2", "template": "poller"}],
                        "add_edges": [{"from": "./ingress2", "to": "./sleepy"}]}}),
    )
    .await;
    assert!(
        spawn_count.load(Ordering::SeqCst) > baseline + 1,
        "the ordinary reconnect must build the task (ingress2 plus the woken cell)"
    );

    h.shutdown().await;
    // "persisted inactive" — the row is the answer the next boot reads; it now
    // says `active`, because the reconnect above woke it. The inactive half of
    // this claim is pinned by `gh437_a_cell_can_be_born_inactive`.
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM registry WHERE path = '/sleepy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "active", "the woken cell's row follows it");
}
