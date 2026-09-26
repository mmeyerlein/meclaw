//! GH #850 (R-SN-5, ADR-0045) — a flood on one cell neither stops the colony
//! nor loses a message.
//!
//! Before the overflow, `route()` delivered with `entry.handle.send(msg).await`
//! and the waiter was the colony's routing loop: a cell whose mailbox was full
//! stopped ALL routing until it drained, and a cell slower than its producers
//! ended the colony by watchdog. Now a full mailbox overflows per cell — in
//! order, to memory and then to `colony.db` — and only above the per-cell cap
//! does a message die, as `mailbox_full`.
//!
//! Every case measures at the receiver: the stub's mailbox is the cell, the
//! test holds its receiver and reads what arrives, in what order.

use meclaw_colony::watchdog::{Beat, HEARTBEAT_CAPACITY, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, DeadLetter,
    RespawnFn, colony_task,
};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Failure marker (30 s convention).
const MARKER: Duration = Duration::from_secs(30);
/// Mailbox of the flooded cell. Four makes "full" a fact of the setup.
const CAPACITY: usize = 4;

/// A registered stand-in for a cell: the colony holds its mailbox sender, the
/// test holds the receiver and decides when to read.
struct Stub {
    rx: mpsc::Receiver<Message>,
    _peace_tx: oneshot::Sender<()>,
    _backstop_tx: oneshot::Sender<()>,
}

async fn register_stub(inbox_tx: &mpsc::Sender<ColonyMsg>, path: &str, capacity: usize) -> Stub {
    let (tx, rx) = mpsc::channel::<Message>(capacity);
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
        rx,
        _peace_tx: peace_tx,
        _backstop_tx: backstop_tx,
    }
}

/// `colony.json` for one case. Parsed from JSON so each case names the keys it
/// relies on; the drain is off because a stub never acknowledges a delivery.
fn config(json: &str) -> ColonyConfig {
    let mut c = ColonyConfig::parse_str(json).expect("the GH #850 colony.json keys parse");
    c.shutdown_drain_timeout_ms = 0;
    c
}

async fn boot(
    db_file: &std::path::Path,
    cfg: ColonyConfig,
    hb_tx: Option<mpsc::Sender<Beat>>,
) -> (mpsc::Sender<ColonyMsg>, tokio::task::JoinHandle<()>) {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(db_file).expect("open colony.db");
    let root = db_file.parent().expect("db dir").to_path_buf();
    let mut task_cfg = ColonyTaskConfig::new(
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
    );
    if let Some(hb) = hb_tx {
        task_cfg = task_cfg.with_heartbeat(hb);
    }
    let join = tokio::spawn(colony_task(task_cfg));
    (inbox_tx, join)
}

async fn shutdown(inbox_tx: &mpsc::Sender<ColonyMsg>, join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, join).await;
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

/// `n` messages for `target`, in the order they will be sent.
fn flood(target: &str, n: usize) -> Vec<Message> {
    (0..n)
        .map(|_| MessageBuilder::new(Path::new(target)).build())
        .collect()
}

async fn read_n(stub: &mut Stub, n: usize) -> Vec<Uuid> {
    let mut got = Vec::with_capacity(n);
    for i in 0..n {
        let m = tokio::time::timeout(MARKER, stub.rx.recv())
            .await
            .unwrap_or_else(|_| panic!("message {i} of {n} did not arrive within the marker"))
            .expect("mailbox closed");
        got.push(m.id);
    }
    got
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

fn overflow_rows(db_file: &std::path::Path, cell: &str) -> Vec<String> {
    let conn =
        rusqlite::Connection::open_with_flags(db_file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open colony.db read-only");
    conn.busy_timeout(MARKER).expect("busy timeout");
    let mut stmt = conn
        .prepare("SELECT message_id FROM mailbox_overflow WHERE cell_path = ?1 ORDER BY seq")
        .expect("the mailbox_overflow table exists");
    stmt.query_map([cell], |r| r.get::<_, String>(0))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect()
}

/// Poll the table until `pred` holds for the row count (failure marker 30 s).
async fn wait_rows(
    db_file: &std::path::Path,
    cell: &str,
    what: &str,
    pred: impl Fn(usize) -> bool,
) {
    let deadline = Instant::now() + MARKER;
    loop {
        let n = overflow_rows(db_file, cell).len();
        if pred(n) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {n} rows after the marker"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// (1) Five thousand messages on a cell that starts reading only after two
/// seconds: the colony keeps routing — a second cell gets its message while
/// the first is still flooded — no fatal watchdog trip, and all five thousand
/// arrive in the order they were sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_neither_stops_the_colony_nor_loses_a_message() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, join) = boot(&db_file, config("{}"), Some(hb_tx)).await;
    let mut slow = register_stub(&inbox_tx, "/slow", CAPACITY).await;
    let mut fast = register_stub(&inbox_tx, "/fast", CAPACITY).await;

    let (trip_tx, mut trip_rx) = mpsc::channel::<WatchdogTrip>(64);
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    let watchdog = tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        hb_rx,
        trip_tx,
        5,
        Duration::from_millis(100),
        armed_rx,
        WatchdogOnTrip::Exit,
        None,
    ));
    let _ = armed_tx.send(());

    let t0 = Instant::now();
    let msgs = flood("/slow", 5_000);
    let sent: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    let (fast_sent_tx, fast_sent_rx) = oneshot::channel::<Instant>();
    let sender = inbox_tx.clone();
    let flooder = tokio::spawn(async move {
        for m in msgs {
            route(&sender, m).await;
        }
        let at = Instant::now();
        route(&sender, MessageBuilder::new(Path::new("/fast")).build()).await;
        let _ = fast_sent_tx.send(at);
    });

    // The second cell's message, while the first cell reads nothing.
    let reader_starts = t0 + Duration::from_secs(2);
    let got_fast = tokio::time::timeout_at(reader_starts.into(), fast.rx.recv()).await;
    let fast_at = Instant::now();
    assert!(
        matches!(got_fast, Ok(Some(_))),
        "the second cell must get its message while the first is flooded — the \
         routing loop stood still behind a full mailbox"
    );
    let fast_sent_at = fast_sent_rx.await.expect("the flooder sent the probe");
    assert!(
        fast_at.duration_since(fast_sent_at) < Duration::from_secs(1),
        "the probe took {:?} from its send to its cell",
        fast_at.duration_since(fast_sent_at)
    );

    tokio::time::sleep_until(reader_starts.into()).await;
    let got = read_n(&mut slow, sent.len()).await;
    assert_eq!(got.len(), sent.len());
    assert!(
        got == sent,
        "all five thousand arrive, in the order they were sent (first mismatch at {:?})",
        got.iter().zip(&sent).position(|(a, b)| a != b)
    );
    flooder.await.expect("flooder");

    let mut trips: Vec<WatchdogTrip> = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        trips.push(t);
    }
    assert!(
        !trips.iter().any(|t| t.is_fatal(WatchdogOnTrip::Exit)),
        "a full mailbox never trips the watchdog: {trips:?}"
    );
    assert!(
        drain_dead_letters(&inbox_tx).await.is_empty(),
        "below the cap nothing is dead-lettered"
    );
    shutdown(&inbox_tx, join).await;
    watchdog.abort();
}

/// (2) A long flood spills to `colony.db` in blocks — rows in
/// `mailbox_overflow` while the cell reads nothing — and after the drain the
/// table is empty again; the order survives the round trip through the disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_flood_spills_to_disk_and_keeps_its_order() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (inbox_tx, join) = boot(
        &db_file,
        config(r#"{"mailbox_overflow_spill_messages": 100}"#),
        None,
    )
    .await;
    let mut slow = register_stub(&inbox_tx, "/slow", CAPACITY).await;

    let msgs = flood("/slow", 5_000);
    let sent: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    for m in msgs {
        route(&inbox_tx, m).await;
    }
    wait_rows(&db_file, "/slow", "rows during the flood", |n| n > 0).await;

    let got = read_n(&mut slow, sent.len()).await;
    assert!(
        got == sent,
        "the order survives the disk (first mismatch at {:?})",
        got.iter().zip(&sent).position(|(a, b)| a != b)
    );
    wait_rows(&db_file, "/slow", "rows after the drain", |n| n == 0).await;
    shutdown(&inbox_tx, join).await;
}

/// (3) Above the per-cell cap — and only there — a message is dead-lettered as
/// `mailbox_full`: with a mailbox of four and a cap of fifty, exactly the
/// messages after the first fifty-four die, and none of the others is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_what_exceeds_the_cap_is_dead_lettered() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (inbox_tx, join) = boot(
        &db_file,
        config(r#"{"mailbox_overflow_cap_messages": 50}"#),
        None,
    )
    .await;
    let mut slow = register_stub(&inbox_tx, "/slow", CAPACITY).await;

    let msgs = flood("/slow", 200);
    let sent: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    for m in msgs {
        route(&inbox_tx, m).await;
    }
    let dead = drain_dead_letters(&inbox_tx).await;
    let kept = CAPACITY + 50;
    assert_eq!(
        dead.len(),
        sent.len() - kept,
        "exactly the messages above the cap"
    );
    assert!(
        dead.iter().all(|d| d.reason.as_code() == "mailbox_full"),
        "and they say why: {:?}",
        dead.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
    );
    let dead_ids: Vec<Uuid> = dead.iter().map(|d| d.message.id).collect();
    assert_eq!(dead_ids, sent[kept..].to_vec(), "the newest die, in order");

    let got = read_n(&mut slow, kept).await;
    assert_eq!(got, sent[..kept].to_vec(), "no other message is lost");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), slow.rx.recv())
            .await
            .is_err(),
        "and nothing arrives twice"
    );
    shutdown(&inbox_tx, join).await;
}

/// (4) A restart with rows in the table: after the boot the cell gets them, in
/// order, and the table empties.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_overflow_on_disk_is_delivered_after_a_restart() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    // Everything that overflows goes to disk at once — the case is about
    // stage 2, and stage 1 is lost on a restart like a mailbox's content.
    let cfg = r#"{"mailbox_overflow_spill_messages": 0}"#;

    let (inbox_tx, join) = boot(&db_file, config(cfg), None).await;
    let first_life = register_stub(&inbox_tx, "/slow", CAPACITY).await;
    let msgs = flood("/slow", 300);
    let sent: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    for m in msgs {
        route(&inbox_tx, m).await;
    }
    let on_disk = sent.len() - CAPACITY;
    wait_rows(
        &db_file,
        "/slow",
        "every overflowing message on disk",
        |n| n == on_disk,
    )
    .await;
    let rows = overflow_rows(&db_file, "/slow");
    let expected: Vec<String> = sent[CAPACITY..].iter().map(Uuid::to_string).collect();
    assert_eq!(
        rows, expected,
        "the table holds the overflow in arrival order"
    );
    shutdown(&inbox_tx, join).await;
    drop(first_life);

    let (inbox_tx, join) = boot(&db_file, config(cfg), None).await;
    let mut second_life = register_stub(&inbox_tx, "/slow", CAPACITY).await;
    let got = read_n(&mut second_life, on_disk).await;
    assert_eq!(
        got,
        sent[CAPACITY..].to_vec(),
        "delivered after the boot, in order"
    );
    wait_rows(&db_file, "/slow", "rows after the delivery", |n| n == 0).await;
    shutdown(&inbox_tx, join).await;
}

/// (5) Drift lock: the overflow delivers exactly what `route()` delivers — the
/// same spent TTL and the same resolved target, every other field untouched.
/// One message goes through the corridor (the mailbox has room), its twin
/// through the overflow (the mailbox is full); the receiver compares them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_overflow_delivers_what_route_delivers() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (inbox_tx, join) = boot(&db_file, config("{}"), None).await;
    let mut cell = register_stub(&inbox_tx, "/d", 1).await;

    let make = || {
        MessageBuilder::new(Path::new("./d"))
            .ttl(9)
            .reply_to(Path::new("/r"))
            .build()
    };
    let (via_route, via_overflow) = (make(), make());
    let (a_id, b_id) = (via_route.id, via_overflow.id);
    route(&inbox_tx, via_route).await;
    route(&inbox_tx, via_overflow).await;
    // Fence: the colony has routed both before the cell reads anything, so the
    // mailbox of one is full when the twin arrives and the twin overflows.
    assert!(drain_dead_letters(&inbox_tx).await.is_empty());

    let a = tokio::time::timeout(MARKER, cell.rx.recv())
        .await
        .expect("the corridor delivery")
        .expect("mailbox");
    let b = tokio::time::timeout(MARKER, cell.rx.recv())
        .await
        .expect("the overflow delivery")
        .expect("mailbox");
    assert_eq!((a.id, b.id), (a_id, b_id), "in order");
    assert_eq!(a.target.as_str(), "/d");
    assert_eq!(
        b.target.as_str(),
        a.target.as_str(),
        "the same resolved target"
    );
    assert_eq!(a.ttl, 8);
    assert_eq!(b.ttl, a.ttl, "the same spent TTL");
    assert_eq!(b.reply_to, a.reply_to);
    assert_eq!(b.parent_message_id, a.parent_message_id);
    assert_eq!(b.correlation_id, a.correlation_id);
    shutdown(&inbox_tx, join).await;
}

/// (6) Drift lock, stage 2: a message that went through the TABLE comes back
/// exactly as `route()` delivers its twin — not only id, TTL and target, but
/// body, both header compartments, trace, parent, correlation, `reply_to` and
/// `created_at`. Every overflowing message goes to disk at once here
/// (`mailbox_overflow_spill_messages: 0`), and the probe proves the twin was
/// read back from the table rather than handed over from memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_overflow_from_disk_delivers_what_route_delivers() {
    use meclaw_colony::OverflowProbe;
    use meclaw_core::serde_json::json;

    let td = tempfile::TempDir::new().expect("tempdir");
    let db_file = td.path().join("colony.db");
    let (probe_tx, mut probe_rx) = mpsc::unbounded_channel::<OverflowProbe>();
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(&db_file).expect("open colony.db");
    let join = tokio::spawn(colony_task(
        ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx,
            outputs_rx,
            db,
            CellFactoryRegistry::new(),
            td.path().to_path_buf(),
            config(r#"{"mailbox_overflow_spill_messages": 0}"#),
            None,
            None,
        )
        .with_overflow_probe(probe_tx),
    ));
    let mut cell = register_stub(&inbox_tx, "/d", 1).await;

    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("session".into(), json!({"key": "k-1", "turn": 3}));
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_view"));
    let mut via_route = MessageBuilder::new(Path::new("./d"))
        .ttl(9)
        .reply_to(Path::new("/r"))
        .parent_message_id(Uuid::now_v7())
        .correlation_id(Uuid::now_v7())
        .context(context)
        .hop(hop)
        .body(meclaw_core::Body::Inline(json!({
            "system": {"persona": {"text": "p"}},
            "messages": [{"origin": "user", "type": "text", "text": "grüße, \"quoted\""}],
            "extra": [1, 2.5, null, true]
        })))
        .build();
    via_route.created_at = 1_700_000_123;
    let mut via_disk = via_route.clone();
    via_disk.id = Uuid::now_v7();
    let (a_id, b_id) = (via_route.id, via_disk.id);
    route(&inbox_tx, via_route).await;
    route(&inbox_tx, via_disk).await;
    // Fence: both are routed before the cell reads, so the twin overflows.
    assert!(drain_dead_letters(&inbox_tx).await.is_empty());

    let a = tokio::time::timeout(MARKER, cell.rx.recv())
        .await
        .expect("the corridor delivery")
        .expect("mailbox");
    let b = tokio::time::timeout(MARKER, cell.rx.recv())
        .await
        .expect("the delivery from disk")
        .expect("mailbox");
    assert_eq!((a.id, b.id), (a_id, b_id), "in order");

    let mut read_back = 0;
    while let Ok(p) = probe_rx.try_recv() {
        if let OverflowProbe::TableRead(path, n) = p
            && path.as_str() == "/d"
        {
            read_back += n;
        }
    }
    assert!(read_back >= 1, "the twin came back from the table");

    assert_eq!(b.trace_id, a.trace_id, "trace");
    assert_eq!(b.parent_message_id, a.parent_message_id, "parent");
    assert_eq!(b.correlation_id, a.correlation_id, "correlation");
    assert_eq!(b.target, a.target, "resolved target");
    assert_eq!(a.target.as_str(), "/d");
    assert_eq!(b.reply_to, a.reply_to, "reply_to");
    assert_eq!((a.ttl, b.ttl), (8, 8), "the same spent TTL");
    assert_eq!(b.created_at, a.created_at, "created_at");
    assert_eq!(a.created_at, 1_700_000_123);
    assert_eq!(
        format!("{:?}", b.headers),
        format!("{:?}", a.headers),
        "both header compartments"
    );
    assert_eq!(format!("{:?}", b.body), format!("{:?}", a.body), "the body");
    shutdown(&inbox_tx, join).await;
}
