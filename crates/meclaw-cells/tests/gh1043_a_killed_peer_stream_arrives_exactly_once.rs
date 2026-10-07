//! GH #1043: a peer stream, a receiving colony killed again and again, and
//! every acknowledged frame arrives exactly once.
//!
//! The lab observation behind the issue: 300 peer messages, one SIGKILL of
//! the receiving colony, and one frame the mount had acknowledged never
//! reached the agent behind the gate (`lost 1`); a second identical run lost
//! nothing. The receiving gate is a chain of compare-and-set rounds against
//! its book (a `store`): it lays a row, claims it with
//! `update ... where state = 'open'`, and goes on only when the store says
//! `rows_affected: 1`. Anything else reads "taken elsewhere" and ends the
//! frame's way, silently.
//!
//! This lock builds that shape from shipped parts: the real `meclaw` proxy
//! (mount, inbox in its own `cell.db`, built by the shipped
//! `ProxyCellFactory`) on a real listener, a gate cell doing the two rounds,
//! and the shipped `store` as its book. A child process (this test binary,
//! re-executed with `child_colony`) is one life of the receiving colony. The
//! parent is the sender: it posts every frame with a stable frame id until
//! the mount answers `200`, and kills the child with SIGKILL at a random
//! point while the stream is being handed on, several times in a row. A last
//! life finishes. Counted at the receiver, in the book's own `cell.db`: every
//! frame's arrival row exists exactly once — and the kills did hit the window
//! the lock is about: the book counts the replays it answered from its record
//! (`meclaw_consumed.replays`), and a run whose kills replayed nothing proves
//! nothing (review of GH #1043, finding 5).

use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::proxy::meclaw::wire::{FRAME_ID_HEADER, SENT_MS_HEADER};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig,
    ContractView, PlannedEdge, RespawnFn, SpawnedCellKind, SurfaceRegistry, cell_task, colony_task,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Cell, CellOutput, Message, OutputSink, Path, Uuid};
use meclaw_testing::surface_listener::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The lab's stream length.
const FRAMES: usize = 300;
/// Kills per round; a last life follows each round's kills.
const KILLS: usize = 6;
const HDR: &str = "X-Meclaw-Peer";
/// Failure-marker timeout (30 s convention).
const WAIT: Duration = Duration::from_secs(30);

// ------------------------------------------------------------- the child

struct Booted {
    inbox: mpsc::Sender<ColonyMsg>,
    outputs: mpsc::Sender<meclaw_core::CellEmission>,
    keep: Vec<Box<dyn std::any::Any + Send>>,
}

fn boot(db_file: &std::path::Path) -> Booted {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(1024);
    let (outputs_tx, outputs_rx) = mpsc::channel(1024);
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
    Booted {
        inbox: inbox_tx,
        outputs: outputs_tx,
        keep: vec![Box::new(join)],
    }
}

#[allow(clippy::too_many_arguments)]
async fn register_active(
    b: &mut Booted,
    path: &str,
    cell_type: &str,
    sender: mpsc::Sender<Message>,
    join: tokio::task::JoinHandle<()>,
    peace_rx: oneshot::Receiver<()>,
    backstop_rx: oneshot::Receiver<()>,
    stop: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    respawn: RespawnFn,
) {
    let (stop_tx, death_ack_rx) = match stop {
        Some((s, d)) => (Some(s), Some(d)),
        None => (None, None),
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    b.inbox
        .send(ColonyMsg::Register {
            path: Path::new(path),
            sender,
            join,
            peace_rx,
            backstop_rx,
            stop_tx,
            death_ack_rx,
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: cell_type.into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("register ack");
}

/// The shipped `meclaw` proxy, built by the shipped factory: its mount takes
/// the name `peer` on `surfaces`, its inbox lives in `<dir>/cell.db`.
async fn register_peer(b: &mut Booted, surfaces: &Arc<SurfaceRegistry>, dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("proxy cell dir");
    let params = json!({"platform": "meclaw", "mount": "peer", "identity_header": HDR,
        "boundary": "north", "emit_to": "/gate", "external_timeout_ms": 2000,
        "egress": ["http://127.0.0.1:1"], "lanes": {
            "accepts": [{"route": "topic", "fields": ["topic"], "because": "a subject"}],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}});
    let spawned = Arc::new(ProxyCellFactory::new(Arc::clone(surfaces)))
        .spawn_cell(
            Path::new("/friend"),
            params,
            b.outputs.clone(),
            dir.to_path_buf(),
            ContractView::default(),
            b.inbox.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("peer params are well-formed");
    let SpawnedCellKind::Active {
        sender,
        join,
        peace_rx,
        stop_tx,
        death_ack_rx,
        backstop_rx,
        respawn,
    } = spawned
    else {
        unreachable!("a proxy is long-running and spawns at once");
    };
    register_active(
        b,
        "/friend",
        "proxy",
        sender,
        join,
        peace_rx,
        backstop_rx,
        Some((stop_tx, death_ack_rx)),
        respawn,
    )
    .await;
}

/// A cell behind `cell_task`, with the colony's inbox (it marks consumed).
async fn register_cell<C: Cell + Send + 'static>(b: &mut Booted, path: &str, cell: C) {
    let (tx, rx) = mpsc::channel::<Message>(1000);
    let join = tokio::spawn(cell_task(
        Path::new(path),
        rx,
        b.outputs.clone(),
        cell,
        None,
        None,
        Some(b.inbox.clone()),
    ));
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    b.keep.push(Box::new((peace_tx, backstop_tx)));
    let respawn: RespawnFn = Box::new(|| unreachable!("never respawned"));
    register_active(
        b,
        path,
        "gh1043-gate",
        tx,
        join,
        peace_rx,
        backstop_rx,
        None,
        respawn,
    )
    .await;
}

/// The shipped `store` as the gate's book, dormant until its first delivery
/// (the way the bootstrap registers a stateful cell).
async fn register_book(b: &mut Booted, dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("book cell dir");
    let spawned = Arc::new(meclaw_cells::store::StoreCellFactory)
        .spawn_cell(
            Path::new("/book"),
            json!({ "schema": {
                "pending": { "key": "text", "state": "text" },
                "arrived": { "key": "text" },
            } }),
            b.outputs.clone(),
            dir.to_path_buf(),
            ContractView::default(),
            b.inbox.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("book params are well-formed");
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
            path: Path::new("/book"),
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

/// The receiving gate, in the shape of the lab's: an arrival lays a `pending`
/// row; the answer to the lay, if it moved one row, claims the row
/// (`open` → `claimed`, compare-and-set); the answer to the claim, if it moved
/// one row, hands the frame on (one `arrived` row). Any other answer reads
/// "taken elsewhere" and ends the frame's way — the gate's `affected(0) != 1`.
struct Gate;

impl Gate {
    async fn call(sink: &OutputSink, id: String, op: Value) {
        let _ = sink
            .push(CellOutput {
                target: Path::new("/book"),
                content: json!({ "messages": [{
                    "origin": "assistant",
                    "type": "tool_call",
                    "text": op.to_string(),
                    "id": id,
                }]}),
            })
            .await;
    }
}

impl Cell for Gate {
    async fn handle(&mut self, msg: Message, sink: &OutputSink) {
        let Body::Inline(v) = &msg.body else { return };
        if let Some(key) = v.get("topic").and_then(Value::as_str) {
            let op = json!({"operation": "insert", "table": "pending",
                            "row": {"key": key, "state": "open"}});
            Self::call(sink, format!("lay:{key}"), op).await;
            return;
        }
        // A refusal of the peer cell (a mount that failed, a refused frame)
        // goes on stderr, where the parent reads it.
        if msg.headers.hop.get("peer_event").and_then(Value::as_str) == Some("refused") {
            eprintln!("gh1043 gate: refused {:?} {v}", msg.headers.hop);
            return;
        }
        let Some(turn) = v.get("messages").and_then(|m| m.get(0)) else {
            return;
        };
        if turn.get("type").and_then(Value::as_str) != Some("tool_result") {
            return;
        }
        let affected = msg.headers.hop.get("rows_affected").and_then(Value::as_i64);
        if affected != Some(1) {
            return;
        }
        let id = turn.get("id").and_then(Value::as_str).unwrap_or_default();
        match id.split_once(':') {
            Some(("lay", key)) => {
                let op = json!({"operation": "update", "table": "pending",
                                "set": {"state": "claimed"},
                                "where": {"key": key, "state": "open"}});
                Self::call(sink, format!("claim:{key}"), op).await;
            }
            Some(("claim", key)) => {
                let op = json!({"operation": "insert", "table": "arrived",
                                "row": {"key": key}});
                Self::call(sink, format!("arrive:{key}"), op).await;
            }
            _ => {}
        }
    }
}

/// The child half: one life of the receiving colony. Prints `LISTEN <addr>`
/// once the mount is up, then runs until it is killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "child process of a_killed_peer_stream_delivers_every_acknowledged_frame_exactly_once"]
async fn child_colony() {
    let Ok(dir) = std::env::var("GH1043_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    // Each step of the boot says where it is on stderr (the parent shows the
    // file when a life does not come up).
    let step = |what: &str| eprintln!("gh1043 child: {what}");
    let mut b = boot(&dir.join("colony.db"));
    let surfaces = Arc::new(SurfaceRegistry::new());
    register_peer(&mut b, &surfaces, &dir.join("friend")).await;
    step("peer registered");
    register_cell(&mut b, "/gate", Gate).await;
    register_book(&mut b, &dir.join("book")).await;
    step("gate and book registered");
    initial_apply(
        &b,
        vec![
            edge("/friend", "/gate"),
            edge("/gate", "/book"),
            edge("/book", "/gate"),
        ],
    )
    .await;
    step("initial apply acknowledged");
    wait_for_mount(&surfaces, "peer").await;
    step("mount up");
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    {
        use std::io::Write;
        println!("LISTEN {addr}");
        let _ = std::io::stdout().flush();
    }
    std::future::pending::<()>().await;
}

// ------------------------------------------------------------ the parent

/// xorshift64*: a reproducible kill schedule from a printed seed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo).max(1)
    }
}

/// One frame as the sender keeps it in its outbox: a stable id, the body as
/// posted every time (its `topic` is the key the gate books), and whether the
/// mount has answered `200`.
struct Outgoing {
    id: Uuid,
    body: Value,
    acked: bool,
}

fn outgoing(i: usize) -> Outgoing {
    let key = format!("k{i:04}");
    Outgoing {
        id: Uuid::now_v7(),
        body: json!({"v": 1, "type": "message", "lane": "topic",
            "trace_id": Uuid::now_v7().to_string(), "ttl": 5, "context": {},
            "body": {"messages": [], "topic": key}}),
        acked: false,
    }
}

/// One POST as the sending proxy makes it; `true` on `200`.
async fn post(client: &reqwest::Client, addr: &str, f: &Outgoing) -> bool {
    let sent_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    client
        .post(format!("http://{addr}/peer/"))
        .header(HDR, "south")
        .header(FRAME_ID_HEADER, f.id.to_string())
        .header(SENT_MS_HEADER, sent_ms.to_string())
        .json(&f.body)
        .send()
        .await
        .is_ok_and(|r| r.status().as_u16() == 200)
}

/// Starts one life and returns it with the address its mount listens on.
async fn life(dir: &std::path::Path) -> (tokio::process::Child, String) {
    use tokio::io::AsyncBufReadExt;
    let exe = std::env::current_exe().expect("test binary");
    let mut child = tokio::process::Command::new(&exe)
        .args([
            "--exact",
            "child_colony",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("GH1043_DIR", dir)
        .stdout(std::process::Stdio::piped())
        .stderr(life_log(dir))
        .kill_on_drop(true)
        .spawn()
        .expect("spawn child colony");
    let out = child.stdout.take().expect("child stdout");
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut tx = Some(tx);
        let mut lines = tokio::io::BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Only the `LISTEN` line takes the sender; libtest prints its own
            // lines first (`running 1 test`), and its `test child_colony ... `
            // has no line end, so the announcement follows it on the same line.
            if let Some(addr) = line.split_once("LISTEN ").map(|(_, a)| a.trim())
                && let Some(tx) = tx.take()
            {
                let _ = tx.send(addr.to_string());
            }
        }
    });
    let addr = match tokio::time::timeout(WAIT + Duration::from_secs(15), rx).await {
        Ok(Ok(addr)) => addr,
        other => panic!(
            "the life did not announce its mount ({other:?}); its stderr:\n{}",
            std::fs::read_to_string(dir.join("lives.stderr")).unwrap_or_default()
        ),
    };
    (child, addr)
}

/// Every life's stderr, appended to one file next to the colony, so a life
/// that dies before its mount is up says why.
fn life_log(dir: &std::path::Path) -> std::process::Stdio {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("lives.stderr"))
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null())
}

/// SIGKILL (`Child::start_kill` is SIGKILL on unix), then reap.
async fn kill(child: &mut tokio::process::Child) {
    child.start_kill().expect("SIGKILL");
    let status = child.wait().await.expect("reap");
    assert!(!status.success(), "killed, not exited");
}

fn book(dir: &std::path::Path) -> Option<rusqlite::Connection> {
    let file = dir.join("book").join("cell.db");
    if !file.exists() {
        return None;
    }
    rusqlite::Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

fn count(dir: &std::path::Path, sql: &str) -> i64 {
    book(dir)
        .and_then(|c| c.query_row(sql, [], |r| r.get(0)).ok())
        .unwrap_or(0)
}

/// The replays the book answered from its record so far.
const REPLAYS: &str = "SELECT coalesce(sum(replays), 0) FROM meclaw_consumed";

/// What one round measured.
#[derive(Debug)]
struct Round {
    seed: u64,
    acked: usize,
    arrived_distinct: i64,
    arrived_rows: i64,
    claimed_never_arrived: i64,
    laid_never_claimed: i64,
    laid_twice: i64,
    /// Replayed answers the book gave from its record, per life (each life's
    /// replays happen at its boot, after the kill before it).
    replays: Vec<i64>,
}

/// One round: `KILLS` lives, each posting a share of fresh frames (plus the
/// unacknowledged ones) and killed with SIGKILL at a random point of its
/// stream; then a last life that posts what is left and finishes.
async fn round(seed: u64) -> Round {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client");
    let mut rng = Rng(seed | 1);
    let mut frames: Vec<Outgoing> = Vec::new();
    let share = FRAMES / (KILLS + 1);
    let mut replays = Vec::new();
    let mut replays_before = 0;
    for _ in 0..KILLS {
        let (mut child, addr) = life(dir.path()).await;
        for _ in 0..share {
            frames.push(outgoing(frames.len()));
        }
        // The stream of this life: every frame not yet acknowledged, oldest
        // first, one every 2 ms; the kill falls anywhere in it or just after.
        let todo: Vec<usize> = (0..frames.len()).filter(|&i| !frames[i].acked).collect();
        let kill_at = Duration::from_millis(rng.range(20, 2 * todo.len() as u64 + 60));
        let mut acked = Vec::new();
        {
            let stream = async {
                for &i in &todo {
                    if post(&client, &addr, &frames[i]).await {
                        acked.push(i);
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                std::future::pending::<()>().await;
            };
            tokio::select! {
                () = stream => {}
                () = tokio::time::sleep(kill_at) => {}
            }
        }
        kill(&mut child).await;
        let total = count(dir.path(), REPLAYS);
        replays.push(total - replays_before);
        replays_before = total;
        for i in acked {
            frames[i].acked = true;
        }
    }
    while frames.len() < FRAMES {
        frames.push(outgoing(frames.len()));
    }
    // The last life: everything still unacknowledged until the mount answers.
    let (mut child, addr) = life(dir.path()).await;
    let deadline = Instant::now() + WAIT;
    while frames.iter().any(|f| !f.acked) && Instant::now() < deadline {
        for f in frames.iter_mut().filter(|f| !f.acked) {
            f.acked = post(&client, &addr, f).await;
        }
    }
    // The chain runs out: done when every frame arrived, or when nothing
    // moved for 3 s (a lost frame never comes).
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut last, mut since) = (-1, Instant::now());
    while Instant::now() < deadline {
        let n = count(dir.path(), "SELECT count(*) FROM arrived");
        if n != last {
            (last, since) = (n, Instant::now());
        }
        if (n as usize >= FRAMES && since.elapsed() > Duration::from_millis(500))
            || since.elapsed() > Duration::from_secs(3)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    kill(&mut child).await;
    replays.push(count(dir.path(), REPLAYS) - replays_before);
    Round {
        seed,
        replays,
        acked: frames.iter().filter(|f| f.acked).count(),
        arrived_distinct: count(dir.path(), "SELECT count(DISTINCT key) FROM arrived"),
        arrived_rows: count(dir.path(), "SELECT count(*) FROM arrived"),
        claimed_never_arrived: count(
            dir.path(),
            "SELECT count(*) FROM pending WHERE state = 'claimed'
               AND key NOT IN (SELECT key FROM arrived)",
        ),
        laid_never_claimed: count(
            dir.path(),
            "SELECT count(*) FROM pending WHERE state = 'open'",
        ),
        laid_twice: count(
            dir.path(),
            "SELECT count(*) FROM (SELECT key FROM pending GROUP BY key HAVING count(*) > 1)",
        ),
    }
}

/// The lock: a receiving colony killed at random points of a peer stream,
/// several times in a row, delivers every acknowledged frame to the end of
/// its gate exactly once — no frame lost in the hand-on, none doubled.
/// Three rounds of `KILLS` kills each by default (`GH1043_ROUNDS` sets
/// another count); `GH1043_SEED` replays one schedule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_killed_peer_stream_delivers_every_acknowledged_frame_exactly_once() {
    let rounds: usize = std::env::var("GH1043_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let fixed: Option<u64> = std::env::var("GH1043_SEED")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut bad = Vec::new();
    let mut replayed = 0;
    for r in 0..rounds {
        let seed = fixed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                ^ (r as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        });
        let m = round(seed).await;
        println!(
            "GH1043 round={r} seed={} kills={KILLS} frames={FRAMES} acked={} arrived={} rows={} \
             claimed_never_arrived={} laid_never_claimed={} laid_twice={} replays_per_life={:?}",
            m.seed,
            m.acked,
            m.arrived_distinct,
            m.arrived_rows,
            m.claimed_never_arrived,
            m.laid_never_claimed,
            m.laid_twice,
            m.replays
        );
        replayed += m.replays.iter().sum::<i64>();
        if m.acked != FRAMES
            || m.arrived_distinct != FRAMES as i64
            || m.arrived_rows != FRAMES as i64
        {
            bad.push(m);
        }
    }
    assert!(
        bad.is_empty(),
        "every acknowledged frame arrives exactly once ({rounds} rounds, {KILLS} kills each); \
         failed rounds: {bad:?}"
    );
    assert!(
        replayed > 0,
        "the book gave no replayed answers ({rounds} rounds): the run never hit the window \
         between a store's commit and its answer reaching the log, so it proves nothing"
    );
}
