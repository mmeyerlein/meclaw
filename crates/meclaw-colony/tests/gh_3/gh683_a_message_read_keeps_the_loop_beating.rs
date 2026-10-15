//! GH #683 — a page of the message browser is not the colony's business.
//!
//! Measured on a live colony (2026-09-13): 902 `slow_work_item` trips and one
//! fatal `stuck_work_item`, all of them with `witness=kept` and
//! `supervisor_lag=0` — the host was fine, the loop alone was quiet. The quiet
//! stretch was the loop's OWN `ReadMessages` arm: it awaits the SQL of a read
//! that needs nothing from the loop but the path of the database file.
//!
//! Two halves, both positive:
//!
//! 1. the structural one — the loop answers a cheap in-memory message while the
//!    read is still running. Before the fix that is impossible: the inbox is
//!    FIFO and the arm holds the loop until the SQL returns;
//! 2. the load one — two hundred reads through the production call site, judged
//!    by the shipped supervisor against the REAL beat stream, never produce a
//!    `Stop`.
//!
//! And a third, the name: the read declares itself, so a trip that happens near
//! it is never reported as `work_item=none`.

use meclaw_colony::api_dto::{MessageLogFilter, ReadMessagesReply};
use meclaw_colony::watchdog::{Beat, HEARTBEAT_CAPACITY, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, RespawnFn,
    colony_task,
};
use meclaw_core::{Message, Path, Uuid};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const ROWS: usize = 5_000;
/// Rows carrying a body of [`BIG_BODY_BYTES`]. What makes a scan expensive is
/// the bytes the inner select materialises, not the blob files on disk: a
/// `blob` row carries a uuid in `body_payload` and costs the loop nothing.
const FAT_ROWS: usize = 200;
const BIG_BODY_BYTES: usize = 100 * 1024;
const BLOB_ROWS: usize = 20;
const READS: usize = 200;
/// The label the read must declare (GH #571's spelling, same shape).
const LABEL: &str = "colony-read /colony/messages";

/// A registered stand-in for a cell: the colony holds a plain mailbox sender, the
/// test holds the receiver. Enough to be a routable node without booting a cell
/// task.
struct Stub {
    _rx: mpsc::Receiver<Message>,
    _peace_tx: oneshot::Sender<()>,
    _backstop_tx: oneshot::Sender<()>,
}

async fn register_stub(inbox_tx: &mpsc::Sender<ColonyMsg>, path: Path) -> Stub {
    let (tx, rx) = mpsc::channel::<Message>(256);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("the stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Register {
            path,
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

/// A colony with one registered stub at `/n000` — enough for the in-memory edge
/// lookup below to have a node to be asked about.
async fn boot(
    td: &std::path::Path,
    hb_tx: mpsc::Sender<Beat>,
) -> (mpsc::Sender<ColonyMsg>, Stub, tokio::task::JoinHandle<()>) {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(&td.join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx,
            outputs_rx,
            db,
            CellFactoryRegistry::new(),
            td.to_path_buf(),
            // The stub is a mailbox, not a cell task: nothing ever acks a
            // delivery, so the graceful drain would wait out its whole budget at
            // the end of the test. Ruling O7's documented off switch skips it.
            ColonyConfig {
                shutdown_drain_timeout_ms: 0,
                ..ColonyConfig::default()
            },
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));
    let stub = register_stub(&inbox_tx, Path::new("/n000")).await;
    (inbox_tx, stub, colony_join)
}

async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}

/// Write the log rows straight into `message_log`. Test data, not a production
/// path — the read under test goes through the colony inbox like every caller.
///
/// The issue's incident had blob bodies of ~500 KB; the loop never saw those
/// bytes. What the loop paid for was the scan: the inner select materialises
/// `scan_budget` rows with all twelve columns and the `COUNT(*)` probe walks
/// the same window a second time, so what makes a page expensive is large
/// INLINE bodies.
/// Hence 5000 rows of which 200 carry ~100 KB inline (the size class of the
/// incident, scaled to what a tmpfs `TMPDIR` comfortably holds — the file stays
/// under ~30 MB), plus a handful of `blob` rows for the picture.
fn fill_message_log(db_file: &std::path::Path) {
    let conn = rusqlite::Connection::open(db_file).expect("open colony.db for the fixture");
    set_busy_timeout(&conn);
    let fat = "x".repeat(BIG_BODY_BYTES);
    let tx = conn.unchecked_transaction().expect("begin");
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO message_log (id, trace_id, parent_message_id, correlation_id,
                     ttl, from_path, to_path, reply_to, headers, body_kind, body_payload,
                     created_at)
                 VALUES (?1, ?2, NULL, NULL, 8, ?3, ?4, NULL, '{}', ?5, ?6, ?7)",
            )
            .expect("prepare insert");
        for i in 0..ROWS {
            let blob = i < BLOB_ROWS;
            let fatty = !blob && i < BLOB_ROWS + FAT_ROWS;
            let (kind, payload) = if blob {
                ("blob", Uuid::now_v7().to_string())
            } else if fatty {
                ("inline", format!(r#"{{"messages":[{{"text":"{fat}"}}]}}"#))
            } else {
                ("inline", r#"{"messages":[]}"#.to_string())
            };
            stmt.execute(rusqlite::params![
                Uuid::now_v7().to_string(),
                Uuid::now_v7().to_string(),
                "/probe",
                format!("/screen/pane{:03}", i % 8),
                kind,
                payload,
                1_757_700_000i64 + i as i64,
            ])
            .expect("insert a log row");
        }
    }
    tx.commit().expect("commit the fixture");
}

fn set_busy_timeout(conn: &rusqlite::Connection) {
    conn.busy_timeout(Duration::from_secs(30))
        .expect("busy timeout");
}

fn a_page() -> MessageLogFilter {
    MessageLogFilter {
        limit: 100,
        scan_budget: 5_000,
        ..MessageLogFilter::default()
    }
}

/// The loop is free while the read runs.
///
/// `ReadInboundEdges` is an in-memory lookup on the edge table answered inside
/// the loop (`colony.rs`, the arm right below the read ones) — nanoseconds
/// against a `Vec`. It is sent AFTER the read and must be answered BEFORE it.
/// Before the fix that ordering cannot happen at all: the inbox is FIFO and the
/// read's ack is sent before the loop looks at the next message. After the fix
/// the discriminator is orders of magnitude wide — a hash lookup against
/// `spawn_blocking` plus opening a SQLite file plus two passes over 5000 rows —
/// which is why this is a semantic discriminator and not a race.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_no_longer_holds_the_loop() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, _hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, _stub, colony_join) = boot(td.path(), hb_tx).await;
    fill_message_log(&td.path().join("colony.db"));

    let (read_ack_tx, mut read_ack_rx) = oneshot::channel::<ReadMessagesReply>();
    inbox_tx
        .send(ColonyMsg::ReadMessages {
            filter: a_page(),
            ack: read_ack_tx,
        })
        .await
        .expect("colony inbox closed");

    let (edge_ack_tx, edge_ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadInboundEdges {
            of: Path::new("/n000"),
            ack: edge_ack_tx,
        })
        .await
        .expect("colony inbox closed");

    let _inbound = tokio::time::timeout(Duration::from_secs(30), edge_ack_rx)
        .await
        .expect("the in-memory lookup answers within the failure marker")
        .expect("the ack channel stays open");
    assert!(
        read_ack_rx.try_recv().is_err(),
        "the loop answered a message that arrived AFTER the read, so the read is \
         not what it is standing in; a read that has already answered here means \
         the arm still holds the loop"
    );

    let reply = tokio::time::timeout(Duration::from_secs(30), read_ack_rx)
        .await
        .expect("the detached read still answers")
        .expect("the ack channel stays open");
    assert_eq!(
        reply.entries.len(),
        100,
        "a full page comes back: {reply:?}"
    );

    shutdown(inbox_tx, colony_join).await;
}

/// Two hundred reads AT ONCE, judged by the shipped supervisor against the real
/// beats.
///
/// What this checks is the attribution — no trip that blames the loop
/// (`colony_loop`) — and, once, the concurrency: every read is in flight before
/// the first one is awaited, which is the shape of the incident (several
/// windows of a screen paging in bursts) and the load the admission control in
/// the read arms is sized for. It does not check latency: how long the burst
/// takes is the reads' business, not the loop's.
///
/// The supervisor runs LIVE alongside the reads — `run_watchdog` on the shipped
/// policy, five windows of 100 ms, exactly as `gh571` measures it — rather than
/// a `Watchdog::new(5)` replayed over a recorded stream: the beat stream carries
/// no timestamps, and the live supervisor is the very judgement the incident was
/// reported by. The heartbeat runs at the PRODUCTION capacity, not at a
/// test-only generous one: a channel that drops the loop's newest word under
/// burst is how a talking loop came to look like a silent one, and a test that
/// sized it generously would measure a different world than the one that broke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hundred_reads_never_starve_the_loop() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, _stub, colony_join) = boot(td.path(), hb_tx).await;
    fill_message_log(&td.path().join("colony.db"));

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

    // All sends first, then the acks: every read is in the inbox before one
    // answer is looked at (no `futures` crate in this crate's dev-deps, and a
    // `Vec` of oneshots is the same join without it).
    let mut acks = Vec::with_capacity(READS);
    for _ in 0..READS {
        let (ack_tx, ack_rx) = oneshot::channel();
        inbox_tx
            .send(ColonyMsg::ReadMessages {
                filter: a_page(),
                ack: ack_tx,
            })
            .await
            .expect("colony inbox closed");
        acks.push(ack_rx);
    }
    let started = std::time::Instant::now();
    for ack_rx in acks {
        let reply = tokio::time::timeout(Duration::from_secs(60), ack_rx)
            .await
            .expect("every read answers within the failure marker")
            .expect("the ack channel stays open");
        assert_eq!(reply.entries.len(), 100);
    }
    eprintln!(
        "gh683: {READS} concurrent reads answered in {} ms",
        started.elapsed().as_millis()
    );

    let mut trips: Vec<WatchdogTrip> = Vec::new();
    while let Ok(t) = trip_rx.try_recv() {
        trips.push(t);
    }
    assert!(
        !trips.iter().any(|t| t.starved() == "colony_loop"),
        "a burst of reads is work, not a parked loop that stopped answering — \
         trips were {trips:?}"
    );

    shutdown(inbox_tx, colony_join).await;
    watchdog.abort();
}

/// The read says what it is, so a trip that happens near it is never `work_item=none`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_read_declares_itself() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, _stub, colony_join) = boot(td.path(), hb_tx).await;
    fill_message_log(&td.path().join("colony.db"));

    // Everything the boot said is not under test; the channel starts empty so
    // the production capacity is never what decides whether the label fits.
    while hb_rx.try_recv().is_ok() {}

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::ReadMessages {
            filter: a_page(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    let reply = tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("the read answers within the failure marker")
        .expect("the ack channel stays open");
    assert_eq!(reply.entries.len(), 100);

    let mut labelled: Vec<String> = Vec::new();
    while let Ok(b) = hb_rx.try_recv() {
        if let Beat::WorkingOn(w) = b {
            labelled.push(w.as_str().to_string());
        }
    }
    assert_eq!(
        labelled.iter().filter(|l| l.as_str() == LABEL).count(),
        1,
        "one read declares itself exactly once, under the endpoint's name; \
         labels were {labelled:?}"
    );

    shutdown(inbox_tx, colony_join).await;
}
