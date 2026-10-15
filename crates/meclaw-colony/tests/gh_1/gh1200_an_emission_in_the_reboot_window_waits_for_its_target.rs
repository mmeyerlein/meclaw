//! GH #1200 (third recurrence) — an emission raised while a REBOOTING colony
//! is still registering its cells waits for its target, and only the boot
//! apply window makes it wait.
//!
//! The lost frame of `gh1043` came from this window: the receiving colony's
//! edges were already hydrated from `colony.db`, its peer proxy re-handed an
//! open inbox row the moment it was registered, the gate behind it answered
//! with a write to its book — and the book was not registered yet. The test's
//! child colony booted without `BeginInitialApply`, which the shipped
//! bootstrap (`apply_bootstrap_plan`) always sends first.
//!
//! What the spec says about the two halves, pinned here at the receiver:
//!
//! * outside the window, a target that is not in the registry is a routing
//!   error (overview § Routing errors and the dead-letter queue): a message
//!   with `reply_to` sends an error message back to `reply_to`, and only one
//!   without it is dead-lettered. A cell's emission carries its sender as
//!   `reply_to`, so the sender gets the reply, the dead-letter queue stays
//!   empty, and the target never sees the message — even if it registers a
//!   moment later;
//! * inside the window (startup step 7, GH #389, "on the reboot path too"),
//!   the colony holds the emission until the boot apply has landed, and by
//!   then the target is registered: it arrives there and nowhere else.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, DeadLetter,
    PlannedEdge, RespawnFn, colony_task,
};
use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, Message, Path, Uuid};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Failure marker (30 s convention).
const MARKER: Duration = Duration::from_secs(30);

struct Life {
    inbox: mpsc::Sender<ColonyMsg>,
    outputs: mpsc::Sender<CellEmission>,
    join: tokio::task::JoinHandle<()>,
}

fn boot(db_file: &std::path::Path) -> Life {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(db_file).expect("open colony.db");
    let root = db_file.parent().expect("db dir").to_path_buf();
    let mut cfg = ColonyConfig::parse_str("{}").expect("colony.json parses");
    cfg.shutdown_drain_timeout_ms = 0;
    let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        CellFactoryRegistry::new(),
        root,
        cfg,
        None,
        None,
    )));
    Life {
        inbox: inbox_tx,
        outputs: outputs_tx,
        join,
    }
}

async fn shutdown(life: Life) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = life.inbox.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, life.join).await;
}

fn edges() -> Vec<PlannedEdge> {
    vec![PlannedEdge {
        id: Uuid::now_v7(),
        from: Path::new("/gate"),
        to: Path::new("/book"),
        condition: None,
        modifier: None,
        is_default: false,
        lane: None,
        tap: false,
    }]
}

async fn begin_initial_apply(inbox: &mpsc::Sender<ColonyMsg>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox
        .send(ColonyMsg::BeginInitialApply { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("BeginInitialApply answers")
        .expect("ack");
}

async fn initial_apply(inbox: &mpsc::Sender<ColonyMsg>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox
        .send(ColonyMsg::InitialApply {
            edges: edges(),
            hive_scopes: Vec::new(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("InitialApply answers")
        .expect("ack");
}

/// A registered stand-in for an awake cell: the colony holds its mailbox
/// sender, the test holds the receiver.
struct Stub {
    rx: mpsc::Receiver<Message>,
    _peace_tx: oneshot::Sender<()>,
    _backstop_tx: oneshot::Sender<()>,
}

async fn register(inbox: &mpsc::Sender<ColonyMsg>, path: &str) -> Stub {
    let (tx, rx) = mpsc::channel::<Message>(8);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("the stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox
        .send(ColonyMsg::Register {
            path: Path::new(path),
            sender: tx,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "test-stub".into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
    Stub {
        rx,
        _peace_tx: peace_tx,
        _backstop_tx: backstop_tx,
    }
}

/// The gate's answer to a handed-on frame: a write to its book, emitted while
/// handling a delivery (it has a parent).
fn lay(parent: Uuid) -> CellEmission {
    CellEmission {
        sender_path: Path::new("/gate"),
        parent_message_id: Some(parent),
        trace_id: Uuid::now_v7(),
        input_ttl: meclaw_core::MESSAGE_DEFAULT_TTL,
        input_headers: meclaw_core::Headers::default(),
        input_reply_to: None,
        target: Path::new("/book"),
        content: json!({"messages": [{"origin": "assistant", "type": "text", "text": "lay"}]}),
        direct_reply: false,
    }
}

async fn drain_dead_letters(inbox: &mpsc::Sender<ColonyMsg>) -> Vec<DeadLetter> {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox
        .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the DLQ drain answers")
        .expect("ack")
}

/// A first life that lands its boot apply, so the second boots as a Reboot
/// with the edge `/gate → /book` already hydrated from `colony.db`.
async fn first_life(db_file: &std::path::Path) {
    let life = boot(db_file);
    begin_initial_apply(&life.inbox).await;
    initial_apply(&life.inbox).await;
    shutdown(life).await;
}

fn log_rows(db_file: &std::path::Path, sql: &str) -> i64 {
    rusqlite::Connection::open_with_flags(db_file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open colony.db read-only")
        .query_row(sql, [], |r| r.get(0))
        .expect("count")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outside_the_apply_window_the_emission_answers_its_sender_and_never_reaches_the_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_file = dir.path().join("colony.db");
    first_life(&db_file).await;

    // The harness shape: no `BeginInitialApply`, the gate registered, the
    // book not yet.
    let life = boot(&db_file);
    let mut gate = register(&life.inbox, "/gate").await;
    let parent = Uuid::now_v7();
    life.outputs.send(lay(parent)).await.expect("outputs open");

    // Positive signal: the routing error's reply reaches the sender.
    let reply = tokio::time::timeout(MARKER, gate.rx.recv())
        .await
        .expect("the gate hears back within the marker")
        .expect("gate mailbox open");
    assert_eq!(reply.target.as_str(), "/gate");
    assert_eq!(reply.reply_to, None, "the error reply is terminal");
    let lay_id = reply
        .parent_message_id
        .expect("the reply names the emission it answers");

    // The book comes up a moment later — too late: the emission was routed.
    let mut book = register(&life.inbox, "/book").await;
    initial_apply(&life.inbox).await;
    assert!(
        drain_dead_letters(&life.inbox).await.is_empty(),
        "with a reply_to the routing error is a reply, never a dead letter"
    );
    assert!(
        book.rx.try_recv().is_err(),
        "the book never sees the emission"
    );
    shutdown(life).await;

    // The log has the reply (from `/colony`, parent = the emission) and no
    // row of the emission itself: it was never routable.
    let id = lay_id.to_string();
    assert_eq!(
        log_rows(
            &db_file,
            &format!("SELECT count(*) FROM message_log WHERE id = '{id}'")
        ),
        0
    );
    assert_eq!(
        log_rows(
            &db_file,
            &format!(
                "SELECT count(*) FROM message_log WHERE parent_message_id = '{id}' \
                 AND from_path = '/colony' AND to_path = '/gate'"
            )
        ),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inside_the_apply_window_the_emission_waits_for_its_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_file = dir.path().join("colony.db");
    first_life(&db_file).await;

    // The shipped bootstrap's shape: `BeginInitialApply` first.
    let life = boot(&db_file);
    begin_initial_apply(&life.inbox).await;
    let mut gate = register(&life.inbox, "/gate").await;
    let parent = Uuid::now_v7();
    life.outputs.send(lay(parent)).await.expect("outputs open");
    let mut book = register(&life.inbox, "/book").await;
    initial_apply(&life.inbox).await;

    let got = tokio::time::timeout(MARKER, book.rx.recv())
        .await
        .expect("the book receives the emission within the marker")
        .expect("book mailbox open");
    assert_eq!(got.target.as_str(), "/book");
    assert_eq!(got.parent_message_id, Some(parent));
    assert!(
        drain_dead_letters(&life.inbox).await.is_empty(),
        "nothing dead-letters"
    );
    assert!(gate.rx.try_recv().is_err(), "no error reply to the gate");
    shutdown(life).await;
}
