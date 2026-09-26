//! GH #850 (R-SN-5, ADR-0045) — an overflow that survived a restart goes to
//! the cell that can take it, and nowhere else.
//!
//! Stage 2 of a cell's overflow lives in `colony.db` (`mailbox_overflow`) and
//! comes back at the next boot. Who gets it depends on what registers at the
//! path:
//!
//! * a cell that is active and not failed gets it, in order — also a parked
//!   stateful cell (`NotYetSpawned`), which the overflow has to WAKE because
//!   nothing else will (the cases below give it a real `WakeFn`);
//! * a disconnected or failed cell does not: the overflow is dead-lettered
//!   `cell_inactive` at once, like the rest of a disconnected mailbox — a
//!   message routed to such a cell today dead-letters the same way;
//! * a path nobody registers by the end of the boot (the directory went away
//!   between two boots) is dead-lettered the same way when the boot apply
//!   lands.
//!
//! Before the fix an overflow for an inactive, failed or missing cell stayed
//! for ever: its drain task waited on a parked mailbox nobody wakes, the rows
//! stayed on disk, and the tickets it held kept the colony from ever being
//! quiescent — every shutdown ran into its drain deadline.
//!
//! Every case measures at the receiver: the mailbox the test holds, the
//! dead-letter queue, the table, the time a shutdown takes.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, DeadLetter,
    RespawnFn, WakeFn, colony_task,
};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Failure marker (30 s convention).
const MARKER: Duration = Duration::from_secs(30);
/// Mailbox of every cell here: four makes "full" a fact of the setup.
const CAPACITY: usize = 4;
/// How many messages a woken cell reads before it goes back to sleep.
const READS_PER_WAKE: usize = 7;

/// A registered stand-in for an awake cell: the colony holds its mailbox
/// sender, the test holds the receiver.
struct Stub {
    /// Held, never read: the first life's cells read nothing.
    _rx: mpsc::Receiver<Message>,
    _peace_tx: oneshot::Sender<()>,
    _backstop_tx: oneshot::Sender<()>,
}

async fn register_stub(inbox_tx: &mpsc::Sender<ColonyMsg>, path: &str) -> Stub {
    let (tx, rx) = mpsc::channel::<Message>(CAPACITY);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("the stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
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
        _rx: rx,
        _peace_tx: peace_tx,
        _backstop_tx: backstop_tx,
    }
}

/// A stateful cell the way the boot registers it: parked `NotYetSpawned`,
/// its mailbox receiver held by the colony. With `wake`, a delivery into the
/// parked mailbox is only ever read once the colony calls it.
async fn register_dormant(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    path: &str,
    wake: Option<WakeFn>,
    active: bool,
    failed: bool,
) {
    let (tx, rx) = mpsc::channel::<Message>(CAPACITY);
    let respawn: RespawnFn = Box::new(|| unreachable!("the dormant stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::RegisterDormant {
            path: Path::new(path),
            sender: tx,
            receiver: rx,
            respawn,
            wake,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "test-dormant".into(),
            active,
            failed,
            dormant: false,
            eager_on_reconnect: false,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
}

/// A wake mechanic that reads [`READS_PER_WAKE`] messages, reports each id,
/// and goes back to sleep by handing its receiver to the colony — the way a
/// stateful cell's idle timeout parks it. The next delivery wakes it again.
fn sleepy_wake(
    path: &str,
    inbox_tx: mpsc::Sender<ColonyMsg>,
    seen: mpsc::UnboundedSender<Uuid>,
) -> WakeFn {
    let path = Path::new(path);
    Box::new(move |mut rx: mpsc::Receiver<Message>| {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let (path, inbox, seen) = (path.clone(), inbox_tx.clone(), seen.clone());
        tokio::spawn(async move {
            let _keep = (stop_rx, death_ack_tx);
            for _ in 0..READS_PER_WAKE {
                match rx.recv().await {
                    Some(m) => {
                        let _ = seen.send(m.id);
                    }
                    None => return,
                }
            }
            let _ = inbox.send(ColonyMsg::Sleep { path, receiver: rx }).await;
        });
        (stop_tx, death_ack_rx)
    })
}

fn config(json: &str, drain_ms: u64) -> ColonyConfig {
    let mut c = ColonyConfig::parse_str(json).expect("the GH #850 colony.json keys parse");
    c.shutdown_drain_timeout_ms = drain_ms;
    c
}

async fn boot(
    db_file: &std::path::Path,
    cfg: ColonyConfig,
) -> (mpsc::Sender<ColonyMsg>, tokio::task::JoinHandle<()>) {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(db_file).expect("open colony.db");
    let root = db_file.parent().expect("db dir").to_path_buf();
    let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx,
        outputs_rx,
        db,
        CellFactoryRegistry::new(),
        root,
        cfg,
        None,
        None,
    )));
    (inbox_tx, join)
}

/// Shut down and return how long it took until the colony task ended.
async fn shutdown(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    join: tokio::task::JoinHandle<()>,
) -> Duration {
    let t0 = Instant::now();
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, join).await;
    t0.elapsed()
}

/// The end of the boot apply — every cell the boot knows is registered.
async fn initial_apply(inbox_tx: &mpsc::Sender<ColonyMsg>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::InitialApply {
            edges: Vec::new(),
            hive_scopes: Vec::new(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the boot apply answers")
        .expect("ack");
}

async fn route(inbox_tx: &mpsc::Sender<ColonyMsg>, msg: Message) {
    inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg,
        })
        .await
        .expect("colony inbox closed");
}

/// Route `n` messages to `target`, return their ids in send order.
async fn flood(inbox_tx: &mpsc::Sender<ColonyMsg>, target: &str, n: usize) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        let m = MessageBuilder::new(Path::new(target)).build();
        ids.push(m.id);
        route(inbox_tx, m).await;
    }
    ids
}

async fn drain_dead_letters(inbox_tx: &mpsc::Sender<ColonyMsg>) -> Vec<DeadLetter> {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
        .await
        .expect("colony inbox closed");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the DLQ drain answers")
        .expect("ack")
}

fn overflow_rows(db_file: &std::path::Path, cell: &str) -> usize {
    let conn =
        rusqlite::Connection::open_with_flags(db_file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open colony.db read-only");
    conn.busy_timeout(MARKER).expect("busy timeout");
    conn.query_row(
        "SELECT count(*) FROM mailbox_overflow WHERE cell_path = ?1",
        [cell],
        |r| r.get::<_, i64>(0),
    )
    .expect("the mailbox_overflow table exists") as usize
}

async fn wait_rows(db_file: &std::path::Path, cell: &str, what: &str, want: usize) {
    let deadline = Instant::now() + MARKER;
    loop {
        let n = overflow_rows(db_file, cell);
        if n == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {n} rows for {cell} after the marker, want {want}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// First life: flood each of `cells` with `n` messages while nobody reads,
/// every overflowing message straight to disk, and shut down with the rows
/// on disk. Returns the ids that are on disk per cell, in order.
async fn a_first_life_with_rows_on_disk(
    db_file: &std::path::Path,
    cells: &[&str],
    n: usize,
) -> Vec<Vec<Uuid>> {
    let (inbox_tx, join) = boot(
        db_file,
        config(r#"{"mailbox_overflow_spill_messages": 0}"#, 0),
    )
    .await;
    let mut stubs = Vec::new();
    for c in cells {
        stubs.push(register_stub(&inbox_tx, c).await);
    }
    let mut on_disk = Vec::new();
    for c in cells {
        let sent = flood(&inbox_tx, c, n).await;
        on_disk.push(sent[CAPACITY..].to_vec());
    }
    for (c, ids) in cells.iter().zip(&on_disk) {
        wait_rows(db_file, c, "the first life's overflow on disk", ids.len()).await;
    }
    shutdown(&inbox_tx, join).await;
    drop(stubs);
    on_disk
}

/// (I-1) A restart with rows on disk for a disconnected cell, a failed cell
/// and a path nobody registers any more: all three overflows are
/// dead-lettered `cell_inactive` — every row, in order — the table empties,
/// and the colony is quiescent: a shutdown with a 20 s drain budget ends at
/// once instead of waiting it out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_dead_letters_the_overflow_no_cell_can_take() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let cells = ["/off", "/failed", "/gone"];
    let on_disk = a_first_life_with_rows_on_disk(&db_file, &cells, 20).await;

    let (inbox_tx, join) = boot(&db_file, config("{}", 20_000)).await;
    register_dormant(&inbox_tx, "/off", None, false, false).await;
    register_dormant(&inbox_tx, "/failed", None, true, true).await;
    // `/gone` is not registered: its directory went away between the boots.
    initial_apply(&inbox_tx).await;

    for c in cells {
        wait_rows(&db_file, c, "rows once the boot settled the overflow", 0).await;
    }
    let dead = drain_dead_letters(&inbox_tx).await;
    for (c, ids) in cells.iter().zip(&on_disk) {
        let mine: Vec<&DeadLetter> = dead
            .iter()
            .filter(|d| d.resolved_target.as_str() == *c)
            .collect();
        assert_eq!(
            mine.iter().map(|d| d.message.id).collect::<Vec<_>>(),
            *ids,
            "every row of {c} is dead-lettered, in order"
        );
        assert!(
            mine.iter().all(|d| d.reason.as_code() == "cell_inactive"),
            "{c}: an overflow no cell can take is cell_inactive: {:?}",
            mine.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
        );
    }
    assert_eq!(
        dead.len(),
        on_disk.iter().map(Vec::len).sum::<usize>(),
        "and nothing else"
    );

    let took = shutdown(&inbox_tx, join).await;
    assert!(
        took < Duration::from_secs(10),
        "nothing is owed any more, so the drain ends at once — it took {took:?} of a 20 s budget"
    );
}

/// Review of K2, M-d: the fallback of OR-SN.K2.9. A colony that never saw its
/// boot apply (no registration, no `InitialApply`) and is shut down owes the
/// rows it hydrated to nobody -- the shutdown settles them on entering its
/// drain instead of waiting the budget out: every row is dead-lettered
/// `cell_inactive`, in order, the table empties, and the task ends far inside
/// a 20 s budget. Measured at `colony.db` after the task has ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_before_the_boot_apply_settles_the_overflow_it_hydrated() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let on_disk = a_first_life_with_rows_on_disk(&db_file, &["/early"], 20).await;

    let (inbox_tx, join) = boot(&db_file, config("{}", 20_000)).await;
    let took = shutdown(&inbox_tx, join).await;
    assert!(
        took < Duration::from_secs(10),
        "an overflow nobody adopted is settled, not waited on -- took {took:?} of 20 s"
    );
    assert_eq!(overflow_rows(&db_file, "/early"), 0, "the table empties");

    let conn =
        rusqlite::Connection::open_with_flags(&db_file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open colony.db read-only");
    conn.busy_timeout(MARKER).expect("busy timeout");
    let mut stmt = conn
        .prepare(
            "SELECT error_code, message_json FROM dead_letters \
             WHERE resolved_target = '/early' ORDER BY id",
        )
        .expect("the dead_letters table");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let ids: Vec<Uuid> = rows
        .iter()
        .map(|(_, m)| {
            let v: serde_json::Value = serde_json::from_str(m).expect("message_json");
            v["id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .expect("a message id")
        })
        .collect();
    assert_eq!(ids, on_disk[0], "every row is dead-lettered, in order");
    assert!(
        rows.iter().all(|(code, _)| code == "cell_inactive"),
        "{:?}",
        rows.iter().map(|(c, _)| c).collect::<Vec<_>>()
    );
}

/// (I-3, restart) A parked stateful cell with its overflow on disk: after the
/// boot the overflow is delivered into its parked mailbox and WAKES it — no
/// other message ever arrives to do that — and the cell, which goes back to
/// sleep every seven messages, gets every row, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_cell_is_woken_by_its_overflow_after_a_restart() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let on_disk = a_first_life_with_rows_on_disk(&db_file, &["/s"], 200).await;
    let want = &on_disk[0];

    let (inbox_tx, join) = boot(&db_file, config("{}", 0)).await;
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<Uuid>();
    let wake = sleepy_wake("/s", inbox_tx.clone(), seen_tx);
    register_dormant(&inbox_tx, "/s", Some(wake), true, false).await;
    initial_apply(&inbox_tx).await;

    let mut got = Vec::with_capacity(want.len());
    for i in 0..want.len() {
        let id = tokio::time::timeout(MARKER, seen_rx.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "row {i} of {} never reached the parked cell — nobody woke it",
                    want.len()
                )
            })
            .expect("seen channel");
        got.push(id);
    }
    assert!(
        got == *want,
        "the overflow reaches the parked cell in order (first mismatch at {:?})",
        got.iter().zip(want).position(|(a, b)| a != b)
    );
    wait_rows(&db_file, "/s", "rows after the delivery", 0).await;
    assert!(
        drain_dead_letters(&inbox_tx).await.is_empty(),
        "nothing dead-lettered"
    );
    shutdown(&inbox_tx, join).await;
}

/// (I-3, serving) A flood on a stateful cell that falls asleep every seven
/// messages: each time it parks, what is still in its overflow wakes it
/// again, and all five hundred arrive in the order they were sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_sleeps_mid_flood_is_woken_by_its_overflow() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (inbox_tx, join) = boot(&db_file, config("{}", 0)).await;
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<Uuid>();
    let wake = sleepy_wake("/s", inbox_tx.clone(), seen_tx);
    register_dormant(&inbox_tx, "/s", Some(wake), true, false).await;

    let sent = flood(&inbox_tx, "/s", 500).await;
    let mut got = Vec::with_capacity(sent.len());
    for i in 0..sent.len() {
        let id = tokio::time::timeout(MARKER, seen_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("message {i} of {} never arrived", sent.len()))
            .expect("seen channel");
        got.push(id);
    }
    assert!(
        got == sent,
        "a sleeping cell gets its overflow in order (first mismatch at {:?})",
        got.iter().zip(&sent).position(|(a, b)| a != b)
    );
    assert!(
        drain_dead_letters(&inbox_tx).await.is_empty(),
        "nothing dead-lettered"
    );
    shutdown(&inbox_tx, join).await;
}
