//! GH #1015: a SIGKILL in the middle of a chain into a REAL `store` cell.
//!
//! The variant of the colony's SIGKILL lock with the shipped store as the
//! target: source -> /c1 -> /c2 -> /ziel, where /c2 turns every message into
//! one `insert` of its trace id into table `t` and /ziel is a `store` cell
//! spawned by the real `StoreCellFactory` (lazy, woken on its first delivery,
//! with its own `cell.db`). The child process (this test binary, re-executed
//! with `child_colony`) is killed hard once a fifth of the chain has landed;
//! a second life of the same directory finishes it.
//!
//! Counted at the receiver: the colony's `message_log` and the store's own
//! `cell.db` (`t` and its dedupe book `meclaw_consumed`). The store's answers
//! go back to /c2, which ignores them (or dead-letter without a route).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig,
    PlannedEdge, RespawnFn, SpawnedCellKind, cell_task, colony_task,
};
use meclaw_core::{Body, Cell, CellOutput, Message, MessageBuilder, OutputSink, Path, Uuid};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const MARKER: Duration = Duration::from_secs(30);
const CHAIN_N: usize = 200;

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

/// The real `store` cell, the way the bootstrap registers a stateful cell:
/// the factory returns it dormant and the colony wakes it on first delivery.
async fn register_store(b: &mut Booted, path: &str, cell_dir: &std::path::Path) {
    std::fs::create_dir_all(cell_dir).expect("store cell dir");
    let spawned = Arc::new(meclaw_cells::store::StoreCellFactory)
        .spawn_cell(
            Path::new(path),
            json!({ "schema": { "t": { "trace": "text" } } }),
            b.outputs.clone(),
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            b.inbox.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("store params are well-formed");
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        stop_tx,
        death_ack_rx,
        respawn,
    } = spawned
    else {
        unreachable!("the stateful store factory returns Dormant");
    };
    b.keep.push(Box::new((stop_tx, death_ack_rx)));
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox
        .send(ColonyMsg::RegisterDormant {
            path: Path::new(path),
            sender,
            receiver,
            respawn,
            wake: Some(wake),
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "store".into(),
            active: true,
            failed: false,
            dormant: false,
            eager_on_reconnect: false,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register dormant ack");
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

/// A read-only look into the store's `cell.db`: 0 while the store has not
/// woken yet (no file, no table) — never creates the file itself.
fn store_count(cell_dir: &std::path::Path, sql: &str) -> i64 {
    let file = cell_dir.join("cell.db");
    if !file.exists() {
        return 0;
    }
    let Ok(c) =
        rusqlite::Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return 0;
    };
    c.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
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

/// Turns a chain message into one store `insert` of its trace id. The store's
/// answer (it replies to its sender) carries no `i` and is ignored.
struct StoreOp {
    to: &'static str,
    delay: Duration,
}
impl Cell for StoreOp {
    async fn handle(&mut self, msg: Message, sink: &OutputSink) {
        let Body::Inline(v) = &msg.body else { return };
        let Some(i) = v.get("i").and_then(|i| i.as_u64()) else {
            return;
        };
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let op = json!({
            "operation": "insert",
            "table": "t",
            "row": { "trace": msg.trace_id.to_string() },
        });
        let _ = sink
            .push(CellOutput {
                target: Path::new(self.to),
                content: json!({ "messages": [{
                    "origin": "assistant",
                    "type": "tool_call",
                    "text": op.to_string(),
                    "id": format!("call_{i}"),
                }]}),
            })
            .await;
    }
}

const ROWS: &str = "SELECT count(*) FROM t";

/// The child half: one life of the receiving colony. `inject` feeds the chain
/// and announces once the store holds a fifth of it (the parent kills it
/// then); `resume` boots the same directory and runs until the store holds
/// every acknowledged message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "child process of a_sigkill_mid_chain_into_a_store_loses_nothing_and_doubles_nothing"]
async fn child_colony() {
    let Ok(dir) = std::env::var("GH1015S_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mode = std::env::var("GH1015S_MODE").unwrap_or_default();
    let db = dir.join("colony.db");
    let store_dir = dir.join("ziel");
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
        StoreOp {
            to: "/ziel",
            delay: Duration::from_millis(3),
        },
        1000,
    )
    .await;
    register_store(&mut b, "/ziel", &store_dir).await;
    initial_apply(&b, vec![edge("/c1", "/c2"), edge("/c2", "/ziel")]).await;
    if mode == "inject" {
        for i in 0..CHAIN_N {
            route(&b, source("/c1", i)).await;
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while store_count(&store_dir, ROWS) < (CHAIN_N / 5) as i64 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        use std::io::Write;
        println!("INJECTED");
        let _ = std::io::stdout().flush();
        std::future::pending::<()>().await;
    } else {
        let acked = count(
            &read(&db),
            "SELECT count(*) FROM message_log WHERE to_path = '/c1' AND from_path = '@external'",
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        while store_count(&store_dir, ROWS) < acked && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown(b).await;
        println!("DONE");
    }
}

/// SIGKILL lock with the shipped store as the target: a real child process,
/// killed hard mid-chain; the second life brings every acknowledged message
/// into the store's table exactly once — 0 lost, 0 duplicate rows, counted in
/// the receiver's log and in the store's own `cell.db`.
#[test]
fn a_sigkill_mid_chain_into_a_store_loses_nothing_and_doubles_nothing() {
    use std::io::BufRead;
    let dir = tempfile::tempdir().expect("tempdir");
    let store_dir = dir.path().join("ziel");
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
            .env("GH1015S_DIR", dir.path())
            .env("GH1015S_MODE", mode)
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

    let open_after_kill = count(
        &read(&dir.path().join("colony.db")),
        "SELECT count(*) FROM delivery_open",
    );
    let rows_before = store_count(&store_dir, ROWS);

    let second = spawn("resume");
    let output = second.wait_with_output().expect("second life");
    assert!(output.status.success(), "the second life ends cleanly");
    assert!(String::from_utf8_lossy(&output.stdout).contains("DONE"));

    let c = read(&dir.path().join("colony.db"));
    let acked = count(
        &c,
        "SELECT count(*) FROM message_log WHERE to_path = '/c1' AND from_path = '@external'",
    );
    let dup_in_log = count(
        &c,
        "SELECT count(*) FROM (SELECT trace_id FROM message_log WHERE to_path = '/ziel' GROUP BY trace_id HAVING count(*) > 1)",
    );
    let open_end = count(&c, "SELECT count(*) FROM delivery_open");
    let rows = store_count(&store_dir, ROWS);
    let distinct = store_count(&store_dir, "SELECT count(DISTINCT trace) FROM t");
    let consumed = store_count(&store_dir, "SELECT count(*) FROM meclaw_consumed");
    println!(
        "GH1015-SIGKILL-STORE acked={acked} rows={rows} distinct={distinct} consumed={consumed} \
         open_end={open_end} open_after_kill={open_after_kill} rows_before={rows_before} \
         dup_in_log={dup_in_log}"
    );
    assert!(
        acked > 0 && open_after_kill > 0,
        "the kill hit the chain mid-way"
    );
    assert_eq!(rows, acked, "0 lost, 0 duplicate rows in the store");
    assert_eq!(
        distinct, acked,
        "every acknowledged trace is in the store once"
    );
    assert_eq!(
        consumed, rows,
        "every row is booked once in meclaw_consumed"
    );
    assert_eq!(
        dup_in_log, 0,
        "0 duplicate at the store in the receiver's log"
    );
    assert_eq!(open_end, 0, "nothing left open");
}
