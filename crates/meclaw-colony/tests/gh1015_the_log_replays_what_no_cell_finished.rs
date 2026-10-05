//! GH #1015: the log is the source, the mailboxes are a cache.
//!
//! A delivery the colony logged to a cell counts as consumed only once that
//! cell's handler is done. Whatever the log holds as delivered and no cell
//! finished is replayed at the next start; a handler that runs again emits the
//! same follow-up ids, and an id the log already holds is not delivered twice.
//!
//! The SIGKILL lock runs a real child process (this test binary, re-executed
//! with `child_colony`) and kills it with SIGKILL in the middle of a chain
//! source -> /c1 -> /c2 -> /ziel; a second life of the same colony directory
//! finishes the chain. Counted at the receiver: its `message_log` and the
//! target cell's own book.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, PlannedEdge,
    RespawnFn, cell_task, colony_task,
};
use meclaw_core::{Body, Cell, CellOutput, Message, MessageBuilder, OutputSink, Path, Uuid};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const MARKER: Duration = Duration::from_secs(30);

struct Booted {
    inbox: mpsc::Sender<ColonyMsg>,
    outputs: mpsc::Sender<meclaw_core::CellEmission>,
    join: tokio::task::JoinHandle<()>,
    keep: Vec<Box<dyn std::any::Any + Send>>,
}

fn config(json: &str) -> ColonyConfig {
    let mut c = ColonyConfig::parse_str(json).expect("colony.json parses");
    c.shutdown_drain_timeout_ms = 0;
    c
}

/// A colony that drains before it stops: every consume mark of a finished
/// handler reaches the writer, so the next life replays nothing it finished.
fn config_drained() -> ColonyConfig {
    let mut c = config("{}");
    c.shutdown_drain_timeout_ms = 10_000;
    c
}

async fn boot(db_file: &std::path::Path, cfg: ColonyConfig) -> Booted {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(1024);
    let (outputs_tx, outputs_rx) = mpsc::channel(1024);
    let db = ColonyDb::open(db_file).expect("open colony.db");
    let root = db_file.parent().expect("db dir").to_path_buf();
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
    Booted {
        inbox: inbox_tx,
        outputs: outputs_tx,
        join,
        keep: Vec::new(),
    }
}

async fn register(
    b: &mut Booted,
    path: &str,
    sender: mpsc::Sender<Message>,
    join: tokio::task::JoinHandle<()>,
) {
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    b.keep.push(Box::new((peace_tx, backstop_tx)));
    let respawn: RespawnFn = Box::new(|| unreachable!("never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox
        .send(ColonyMsg::Register {
            path: Path::new(path),
            sender,
            join,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "gh1015-test".into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
}

/// A real cell behind `cell_task`, with the colony's inbox — it marks.
async fn register_cell<C: Cell + Send + 'static>(
    b: &mut Booted,
    path: &str,
    cell: C,
    capacity: usize,
) {
    let (tx, rx) = mpsc::channel::<Message>(capacity);
    let join = tokio::spawn(cell_task(
        Path::new(path),
        rx,
        b.outputs.clone(),
        cell,
        None,
        None,
        Some(b.inbox.clone()),
    ));
    register(b, path, tx, join).await;
}

/// A cell that takes nothing from its mailbox: every delivery stays open.
async fn register_hold(b: &mut Booted, path: &str, capacity: usize) {
    let (tx, rx) = mpsc::channel::<Message>(capacity);
    b.keep.push(Box::new(rx));
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    register(b, path, tx, join).await;
}

fn edge(from: &str, to: &str) -> PlannedEdge {
    PlannedEdge {
        id: Uuid::now_v7(),
        from: Path::new(from),
        to: Path::new(to),
        condition: None,
        modifier: None,
        is_default: false,
        lane: None,
        tap: false,
    }
}

async fn initial_apply(b: &Booted, edges: Vec<PlannedEdge>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox
        .send(ColonyMsg::InitialApply {
            edges,
            hive_scopes: Vec::new(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("initial apply ack");
}

async fn route(b: &Booted, msg: Message) {
    b.inbox
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg,
        })
        .await
        .expect("colony inbox closed");
}

async fn shutdown(b: Booted) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = b.inbox.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, b.join).await;
}

/// The process dies: the colony task is dropped mid-life, no teardown runs
/// (no ordered-stop record). What the writer committed stays.
async fn crash(b: Booted) {
    b.join.abort();
    let _ = b.join.await;
    drop(b.keep);
}

fn source(target: &str, i: usize) -> Message {
    MessageBuilder::new(Path::new(target))
        .ttl(meclaw_core::MESSAGE_DEFAULT_TTL)
        .body(Body::Inline(json!({ "messages": [], "i": i })))
        .build()
}

fn read(db_file: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(db_file).expect("open colony.db for reading")
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).expect("count query")
}

/// Records what it gets: (id, ttl).
struct Recorder(mpsc::UnboundedSender<(Uuid, u32)>);
impl Cell for Recorder {
    async fn handle(&mut self, msg: Message, _sink: &OutputSink) {
        let _ = self.0.send((msg.id, msg.ttl));
    }
}

/// Hands its body on to `to`, after `delay`.
struct Forward {
    to: &'static str,
    delay: Duration,
}
impl Cell for Forward {
    async fn handle(&mut self, msg: Message, sink: &OutputSink) {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let content = match msg.body {
            Body::Inline(v) => v,
            Body::Blob(_) => json!({}),
        };
        let _ = sink
            .push(CellOutput {
                target: Path::new(self.to),
                content,
            })
            .await;
    }
}

/// Emits its child, then never finishes: the delivery stays open while its
/// child is logged — the crash window of a lost consume mark, held still.
struct EmitAndHang {
    to: &'static str,
}
impl Cell for EmitAndHang {
    async fn handle(&mut self, msg: Message, sink: &OutputSink) {
        let content = match msg.body {
            Body::Inline(v) => v,
            Body::Blob(_) => json!({}),
        };
        let _ = sink
            .push(CellOutput {
                target: Path::new(self.to),
                content,
            })
            .await;
        std::future::pending::<()>().await;
    }
}

async fn collect(rx: &mut mpsc::UnboundedReceiver<(Uuid, u32)>, n: usize) -> Vec<(Uuid, u32)> {
    let mut got = Vec::new();
    let deadline = Instant::now() + MARKER;
    while got.len() < n {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        {
            Ok(Some(x)) => got.push(x),
            _ => break,
        }
    }
    got
}

/// Replay lock + order lock + budget lock: N open deliveries → exactly N
/// deliveries after the restart, the logged ids, in log order, each with the
/// `ttl` the log holds (the hop is not taken twice); the third life replays
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_replays_each_open_delivery_once_in_log_order_with_the_logged_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    const N: usize = 12;

    let mut b = boot(&db, config("{}")).await;
    register_hold(&mut b, "/sink", 64).await;
    initial_apply(&b, Vec::new()).await;
    let mut sent = Vec::new();
    for i in 0..N {
        let m = source("/sink", i);
        sent.push(m.id);
        route(&b, m).await;
    }
    shutdown(b).await;
    let conn = read(&db);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM delivery_open"),
        N as i64,
        "every delivery is open"
    );
    let logged_ttl: i64 = conn
        .query_row(
            "SELECT ttl FROM message_log WHERE id = ?1",
            [sent[0].to_string()],
            |r| r.get(0),
        )
        .expect("logged ttl");
    drop(conn);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut b = boot(&db, config("{}")).await;
    register_cell(&mut b, "/sink", Recorder(tx), 64).await;
    initial_apply(&b, Vec::new()).await;
    let got = collect(&mut rx, N).await;
    // Give the marks their batch, then stop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown(b).await;
    let ids: Vec<Uuid> = got.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, sent, "exactly the N logged ids, in log order");
    assert!(rx.try_recv().is_err(), "no delivery twice");
    for (_, ttl) in &got {
        assert_eq!(
            i64::from(*ttl),
            logged_ttl,
            "the replay hands on the logged budget, not one hop less"
        );
    }
    let conn = read(&db);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM delivery_open"),
        0,
        "consumed after processing"
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM message_log"),
        N as i64,
        "nothing logged twice, nothing deleted"
    );
    drop(conn);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut b = boot(&db, config("{}")).await;
    register_cell(&mut b, "/sink", Recorder(tx), 64).await;
    initial_apply(&b, Vec::new()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown(b).await;
    assert!(
        rx.try_recv().is_err(),
        "a consumed delivery is never replayed"
    );
}

/// Dedupe lock: a handler that emitted its child and never finished is
/// replayed; it emits the same child id again, which the log already holds —
/// the target gets the child exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_handler_does_not_deliver_its_logged_child_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");

    let mut b = boot(&db, config("{}")).await;
    register_cell(&mut b, "/a", EmitAndHang { to: "/b" }, 8).await;
    register_hold(&mut b, "/b", 8).await;
    initial_apply(&b, vec![edge("/a", "/b")]).await;
    let parent = source("/a", 1);
    let parent_id = parent.id;
    route(&b, parent).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown(b).await;
    let conn = read(&db);
    let child_id: String = conn
        .query_row(
            "SELECT id FROM message_log WHERE parent_message_id = ?1",
            [parent_id.to_string()],
            |r| r.get(0),
        )
        .expect("the child is logged");
    assert_eq!(
        count(&conn, "SELECT count(*) FROM delivery_open"),
        2,
        "parent and child are open"
    );
    drop(conn);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut b = boot(&db, config("{}")).await;
    register_cell(
        &mut b,
        "/a",
        Forward {
            to: "/b",
            delay: Duration::ZERO,
        },
        8,
    )
    .await;
    register_cell(&mut b, "/b", Recorder(tx), 8).await;
    initial_apply(&b, Vec::new()).await;
    let got = collect(&mut rx, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown(b).await;
    assert_eq!(got.len(), 1, "the child arrives");
    assert_eq!(got[0].0.to_string(), child_id, "with the id the log holds");
    assert!(rx.try_recv().is_err(), "and only once");
    let conn = read(&db);
    assert_eq!(count(&conn, "SELECT count(*) FROM delivery_open"), 0);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM message_log"),
        2,
        "no second log row for the child"
    );
}

/// Overflow overlap lock: messages in the mailbox and on disk (overflow
/// stage 2) at the stop are each delivered exactly once after the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overflow_and_replay_never_deliver_the_same_id_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    const N: usize = 10;
    let cfg = || config(r#"{"mailbox_overflow_spill_messages": 0}"#);

    let mut b = boot(&db, cfg()).await;
    register_hold(&mut b, "/sink", 4).await;
    initial_apply(&b, Vec::new()).await;
    let mut sent = Vec::new();
    for i in 0..N {
        let m = source("/sink", i);
        sent.push(m.id);
        route(&b, m).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown(b).await;
    let conn = read(&db);
    let spilled = count(&conn, "SELECT count(*) FROM mailbox_overflow");
    assert!(spilled > 0, "the overflow reached disk");
    drop(conn);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut b = boot(&db, cfg()).await;
    register_cell(&mut b, "/sink", Recorder(tx), 4).await;
    initial_apply(&b, Vec::new()).await;
    let got = collect(&mut rx, N).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown(b).await;
    let mut ids: Vec<Uuid> = got.iter().map(|(id, _)| *id).collect();
    assert!(rx.try_recv().is_err(), "nothing beyond the N");
    ids.sort();
    let mut want = sent.clone();
    want.sort();
    assert_eq!(ids, want, "each id once ({spilled} of them from disk)");
}

/// Poison lock: a delivery that never finishes is replayed three times, the
/// fourth boot dead-letters it as `replay_exhausted`; its log row stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_that_never_finishes_ends_as_replay_exhausted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let mut b = boot(&db, config("{}")).await;
    register_hold(&mut b, "/sink", 8).await;
    initial_apply(&b, Vec::new()).await;
    let m = source("/sink", 0);
    let id = m.id;
    route(&b, m).await;
    shutdown(b).await;
    // Review M-2: the first boot after an ordered stop does not count; the
    // three lives after it end in a crash and count 1, 2, 3; the fifth boot
    // dead-letters.
    for life in 0..5 {
        let mut b = boot(&db, config("{}")).await;
        register_hold(&mut b, "/sink", 8).await;
        initial_apply(&b, Vec::new()).await;
        if life < 4 {
            crash(b).await;
        } else {
            shutdown(b).await;
        }
    }
    let conn = read(&db);
    let codes: Vec<String> = conn
        .prepare("SELECT error_code FROM dead_letters")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .expect("dead letters");
    assert_eq!(codes, vec!["replay_exhausted".to_string()]);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM delivery_open"),
        0,
        "closed"
    );
    let kept: i64 = conn
        .query_row(
            "SELECT count(*) FROM message_log WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .expect("log row");
    assert_eq!(kept, 1, "the log row stays");
}

// ---------------------------------------------------------------- SIGKILL --

const CHAIN_N: usize = 200;

/// The target: books every delivery and its effect in ONE transaction of its
/// own database — the effect table is keyed by the message id, so a second
/// delivery of the same id has no second effect (the stateful-cell dedupe).
struct Ziel {
    conn: rusqlite::Connection,
    seen: mpsc::UnboundedSender<()>,
}
impl Cell for Ziel {
    async fn handle(&mut self, msg: Message, _sink: &OutputSink) {
        let tx = self.conn.transaction().expect("ziel tx");
        tx.execute(
            "INSERT INTO calls (id, trace) VALUES (?1, ?2)",
            [msg.id.to_string(), msg.trace_id.to_string()],
        )
        .expect("ziel call");
        let fresh = tx
            .execute(
                "INSERT OR IGNORE INTO effect (id, trace) VALUES (?1, ?2)",
                [msg.id.to_string(), msg.trace_id.to_string()],
            )
            .expect("ziel effect");
        tx.commit().expect("ziel commit");
        if fresh == 1 {
            let _ = self.seen.send(());
        }
    }
}

fn ziel_db(dir: &std::path::Path) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(dir.join("ziel.db")).expect("ziel.db");
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS calls (id TEXT NOT NULL, trace TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS effect (id TEXT PRIMARY KEY, trace TEXT NOT NULL);",
    )
    .expect("ziel schema");
    c
}

/// The child half: one life of the receiving colony. `inject` feeds the chain
/// and announces when the target has seen a part of it (the parent kills it
/// then); `resume` boots the same directory and runs until the target holds
/// every acknowledged message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "child process of a_sigkill_mid_chain_loses_nothing_and_doubles_nothing"]
async fn child_colony() {
    let Ok(dir) = std::env::var("GH1015_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mode = std::env::var("GH1015_MODE").unwrap_or_default();
    let db = dir.join("colony.db");
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let mut b = boot(&db, config("{}")).await;
    register_cell(
        &mut b,
        "/c1",
        Forward {
            to: "/c2",
            delay: Duration::from_millis(1),
        },
        1000,
    )
    .await;
    register_cell(
        &mut b,
        "/c2",
        Forward {
            to: "/ziel",
            delay: Duration::from_millis(3),
        },
        1000,
    )
    .await;
    register_cell(
        &mut b,
        "/ziel",
        Ziel {
            conn: ziel_db(&dir),
            seen: seen_tx,
        },
        1000,
    )
    .await;
    initial_apply(&b, vec![edge("/c1", "/c2"), edge("/c2", "/ziel")]).await;
    if mode == "inject" {
        for i in 0..CHAIN_N {
            route(&b, source("/c1", i)).await;
        }
        let mut n = 0;
        while n < CHAIN_N / 5 {
            if seen_rx.recv().await.is_none() {
                break;
            }
            n += 1;
        }
        use std::io::Write;
        println!("INJECTED");
        let _ = std::io::stdout().flush();
        std::future::pending::<()>().await;
    } else {
        let acked = {
            let c = read(&db);
            count(
                &c,
                "SELECT count(*) FROM message_log WHERE to_path = '/c1' AND from_path = '@external'",
            )
        };
        let have = {
            let c = ziel_db(&dir);
            count(&c, "SELECT count(*) FROM effect")
        };
        let mut n = have;
        let deadline = Instant::now() + Duration::from_secs(60);
        while n < acked {
            match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                seen_rx.recv(),
            )
            .await
            {
                Ok(Some(())) => n += 1,
                _ => break,
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown(b).await;
        println!("DONE");
    }
}

/// SIGKILL lock (Koordinator): a real child process, killed hard in the
/// middle of the chain; the second life brings every acknowledged message to
/// the target exactly once — 0 lost, 0 duplicate at the target, counted in the
/// receiver's log and in the target's own book.
#[test]
fn a_sigkill_mid_chain_loses_nothing_and_doubles_nothing() {
    use std::io::BufRead;
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary");
    let spawn = |mode: &str| {
        std::process::Command::new(&exe)
            .args([
                "--exact",
                "child_colony",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("GH1015_DIR", dir.path())
            .env("GH1015_MODE", mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child colony")
    };

    let mut first = spawn("inject");
    let out = first.stdout.take().expect("child stdout");
    let mut announced = false;
    for line in std::io::BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        if line.contains("INJECTED") {
            announced = true;
            break;
        }
    }
    assert!(announced, "the first life announced the chain under way");
    first.kill().expect("SIGKILL"); // Child::kill is SIGKILL on unix
    let status = first.wait().expect("reap");
    assert!(!status.success(), "killed, not exited");

    let (open_after_kill, ziel_before) = {
        let c = read(&dir.path().join("colony.db"));
        let z = ziel_db(dir.path());
        (
            count(&c, "SELECT count(*) FROM delivery_open"),
            count(&z, "SELECT count(*) FROM effect"),
        )
    };

    let second = spawn("resume");
    let output = second.wait_with_output().expect("second life");
    assert!(output.status.success(), "the second life ends cleanly");
    assert!(String::from_utf8_lossy(&output.stdout).contains("DONE"));

    let c = read(&dir.path().join("colony.db"));
    let z = ziel_db(dir.path());
    let acked = count(
        &c,
        "SELECT count(*) FROM message_log WHERE to_path = '/c1' AND from_path = '@external'",
    );
    let at_ziel_log = count(
        &c,
        "SELECT count(DISTINCT trace_id) FROM message_log WHERE to_path = '/ziel'",
    );
    let dup_in_log = count(
        &c,
        "SELECT count(*) FROM (SELECT trace_id FROM message_log WHERE to_path = '/ziel' GROUP BY trace_id HAVING count(*) > 1)",
    );
    let effects = count(&z, "SELECT count(*) FROM effect");
    let effect_traces = count(&z, "SELECT count(DISTINCT trace) FROM effect");
    let calls = count(&z, "SELECT count(*) FROM calls");
    let open_end = count(&c, "SELECT count(*) FROM delivery_open");
    println!(
        "GH1015-SIGKILL acked={acked} open_after_kill={open_after_kill} ziel_before={ziel_before} \
         ziel_effects={effects} ziel_calls={calls} suppressed={} dup_in_log={dup_in_log} open_end={open_end}",
        calls - effects
    );
    assert!(
        acked > 0 && open_after_kill > 0,
        "the kill hit the chain mid-way"
    );
    assert_eq!(
        at_ziel_log, acked,
        "0 lost: every acknowledged message reached the target's log"
    );
    assert_eq!(effect_traces, acked, "0 lost at the target");
    assert_eq!(effects, acked, "0 duplicate effect at the target");
    assert_eq!(
        dup_in_log, 0,
        "0 duplicate at the target in the receiver's log"
    );
    assert_eq!(open_end, 0, "nothing left open");
}

// ------------------------------------------------------------ fix round 1 --

/// Review M-2: the poison counter counts crashes, not starts. Four ordered
/// restarts with a delivery no cell ever finishes: it is replayed every time
/// and never becomes `replay_exhausted`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordered_restarts_never_exhaust_a_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let mut b = boot(&db, config("{}")).await;
    register_hold(&mut b, "/sink", 8).await;
    initial_apply(&b, Vec::new()).await;
    route(&b, source("/sink", 0)).await;
    shutdown(b).await;
    for _ in 0..4 {
        let mut b = boot(&db, config("{}")).await;
        register_hold(&mut b, "/sink", 8).await;
        initial_apply(&b, Vec::new()).await;
        shutdown(b).await;
    }
    let conn = read(&db);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM dead_letters"),
        0,
        "no dead letter"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM delivery_open WHERE replays = 0"
        ),
        1,
        "still open, no attempt counted"
    );
}

/// Emits one child to `/b` per source, and (with `colony`) one request to an
/// unknown `/colony…` endpoint, then ends.
struct Fan {
    colony: bool,
}
impl Cell for Fan {
    async fn handle(&mut self, msg: Message, sink: &OutputSink) {
        if msg.parent_message_id.is_some() {
            return;
        }
        let _ = sink
            .push(CellOutput {
                target: Path::new("/b"),
                content: json!({ "header": { "k": "v" }, "messages": [] }),
            })
            .await;
        if self.colony {
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/colony/gh1015-probe"),
                    content: json!({ "messages": [] }),
                })
                .await;
        }
    }
}

/// Records, at the instant the writer closes a delivery of `/a`, how many
/// children the log holds for it and how many dead letters its trace has.
/// A trigger fires inside the writer's own transaction: it sees the exact
/// order of the ops, whatever the batching.
fn install_close_probe(db: &std::path::Path) {
    read(db)
        .execute_batch(
            "CREATE TABLE probe (id TEXT NOT NULL, kids INTEGER NOT NULL, dlq INTEGER NOT NULL);
             CREATE TRIGGER probe_close AFTER DELETE ON delivery_open WHEN old.cell_path = '/a'
               AND (SELECT parent_message_id FROM message_log WHERE id = old.message_id) IS NULL
             BEGIN
               INSERT INTO probe VALUES (
                 old.message_id,
                 (SELECT count(*) FROM message_log WHERE parent_message_id = old.message_id),
                 (SELECT count(*) FROM dead_letters WHERE trace_id =
                    (SELECT trace_id FROM message_log WHERE id = old.message_id)));
             END;",
        )
        .expect("probe");
}

/// Review M-1 (trap 1) and I-1, deterministic: a delivery is never closed
/// before the work of its handler — the child's log row, and the dispatch of
/// a `/colony…` request (here: its dead letter) — is in the writer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_consume_mark_is_never_committed_before_its_handlers_work() {
    const N: usize = 40;
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let mut b = boot(&db, config_drained()).await;
    install_close_probe(&db);
    let (tx, mut rx) = mpsc::unbounded_channel();
    register_cell(&mut b, "/a", Fan { colony: true }, 64).await;
    register_cell(&mut b, "/b", Recorder(tx), 64).await;
    initial_apply(&b, vec![edge("/a", "/b")]).await;
    for i in 0..N {
        route(&b, source("/a", i)).await;
    }
    assert_eq!(collect(&mut rx, N).await.len(), N, "every child arrived");
    shutdown(b).await;
    let conn = read(&db);
    let rows: Vec<(i64, i64)> = conn
        .prepare("SELECT kids, dlq FROM probe")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .expect("probe rows");
    assert_eq!(rows.len(), N, "every delivery of /a closed");
    assert!(
        rows.iter().all(|&(kids, dlq)| kids == 1 && dlq == 1),
        "closed only behind its child's log row and its /colony dispatch: {rows:?}"
    );
}

/// Review C-1: two matching edges A→B (different conditions) fan one emission
/// out twice to B — two deliveries with two ids, two log rows, no collision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_matching_edges_to_one_target_deliver_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let mut b = boot(&db, config("{}")).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    register_cell(&mut b, "/a", Fan { colony: false }, 64).await;
    register_cell(&mut b, "/b", Recorder(tx), 64).await;
    let mut conditional = edge("/a", "/b");
    conditional.condition =
        Some(meclaw_colony::cel_eval::parse_condition("hop.k == 'v'").expect("condition"));
    initial_apply(&b, vec![edge("/a", "/b"), conditional]).await;
    route(&b, source("/a", 0)).await;
    route(&b, source("/a", 1)).await;
    let got = collect(&mut rx, 4).await;
    shutdown(b).await;
    let ids: std::collections::HashSet<Uuid> = got.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        (got.len(), ids.len()),
        (4, 4),
        "two per source, four distinct ids"
    );
    let conn = read(&db);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_log WHERE to_path = '/b'"
        ),
        4,
        "four log rows"
    );
}

fn keyed(key: &str, replay: bool) -> meclaw_core::CellEmission {
    let mut header = json!({ meclaw_colony::DELIVERY_KEY_HEADER: key });
    if replay {
        header[meclaw_colony::DELIVERY_REPLAY_HEADER] = json!(true);
    }
    meclaw_core::CellEmission {
        sender_path: Path::new("/mount"),
        parent_message_id: None,
        trace_id: Uuid::now_v7(),
        input_ttl: meclaw_core::MESSAGE_DEFAULT_TTL,
        input_headers: meclaw_core::Headers::default(),
        input_reply_to: None,
        target: Path::new("/h"),
        content: json!({ "header": header, "messages": [] }),
        direct_reply: false,
    }
}

async fn keyed_life(
    db: &std::path::Path,
) -> (
    Booted,
    mpsc::UnboundedReceiver<(Uuid, u32)>,
    mpsc::UnboundedReceiver<(Uuid, u32)>,
) {
    let mut b = boot(db, config_drained()).await;
    let (dtx, drx) = mpsc::unbounded_channel();
    let (htx, hrx) = mpsc::unbounded_channel();
    register_cell(&mut b, "/direct", Recorder(dtx), 64).await;
    register_cell(&mut b, "/h/in", Recorder(htx), 64).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox
        .send(ColonyMsg::InitialApply {
            edges: vec![
                edge("/mount", "/direct"),
                edge("/mount", "/h"),
                edge("/h", "/h/in"),
            ],
            hive_scopes: vec![Path::new("/h")],
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("initial apply ack");
    (b, drx, hrx)
}

/// Review C-2 + I-4, at the receiver: a peer hand-on the ingress repeats from
/// its inbox (`delivery_key` + `delivery_replay`) after a restart is dropped —
/// to a cell directly AND through a hive transit (the mount → hive path) —
/// one log row, one delivery each; a fresh key behind it still arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_keyed_hand_on_is_dropped_direct_and_through_a_hive_transit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let k1 = format!("peer-a:{}", Uuid::now_v7());
    let k2 = format!("peer-a:{}", Uuid::now_v7());
    {
        let (b, mut drx, mut hrx) = keyed_life(&db).await;
        b.outputs.send(keyed(&k1, false)).await.expect("emit");
        assert_eq!(collect(&mut drx, 1).await.len(), 1, "life 1: direct");
        assert_eq!(
            collect(&mut hrx, 1).await.len(),
            1,
            "life 1: through the transit"
        );
        shutdown(b).await;
    }
    let (b, mut drx, mut hrx) = keyed_life(&db).await;
    b.outputs.send(keyed(&k1, true)).await.expect("repeat");
    b.outputs.send(keyed(&k2, false)).await.expect("fresh");
    let d = collect(&mut drx, 1).await;
    let h = collect(&mut hrx, 1).await;
    shutdown(b).await;
    let extra: Vec<(Uuid, u32)> = std::iter::from_fn(|| drx.try_recv().ok())
        .chain(std::iter::from_fn(|| hrx.try_recv().ok()))
        .collect();
    let conn = read(&db);
    let logged = |to: &str| -> Vec<String> {
        conn.prepare("SELECT id FROM message_log WHERE to_path = ?1 ORDER BY rowid")
            .and_then(|mut s| s.query_map([to], |r| r.get(0))?.collect())
            .expect("log ids")
    };
    let (direct, inner) = (logged("/direct"), logged("/h/in"));
    assert!(
        extra.is_empty(),
        "nothing behind the fresh key: life 2 got {d:?} {h:?} + {extra:?}; log /direct {direct:?} /h/in {inner:?}"
    );
    assert_eq!(
        (direct.len(), inner.len()),
        (2, 2),
        "one log row per key and target"
    );
    assert_eq!(
        d[0].0.to_string(),
        direct[1],
        "life 2 delivered only the fresh key (direct)"
    );
    assert_eq!(
        h[0].0.to_string(),
        inner[1],
        "life 2 delivered only the fresh key (transit)"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message_log WHERE to_path = '/h'"
        ),
        2
    );
}

/// Review I-6: a hand-on keyed by a UUID (a frame id) takes it as the seed —
/// its children carry the frame's v7 time; any other key carries the time
/// stamped as `delivery_key_ms`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyed_ids_carry_a_real_time_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("colony.db");
    let frame = Uuid::now_v7();
    let (b, mut drx, _hrx) = keyed_life(&db).await;
    b.outputs
        .send(keyed(&frame.to_string(), false))
        .await
        .expect("uuid key");
    let mut other = keyed("peer-a:local-7", false);
    other.content["header"][meclaw_colony::DELIVERY_KEY_MS_HEADER] = json!(1_759_000_000_123u64);
    b.outputs.send(other).await.expect("other key");
    let got = collect(&mut drx, 2).await;
    shutdown(b).await;
    let ms = |id: Uuid| id.as_u128() >> 80;
    assert_eq!(got.len(), 2);
    assert_eq!(ms(got[0].0), ms(frame), "the frame's time");
    assert_eq!(ms(got[1].0), 1_759_000_000_123u128, "the stamped time");
}
