//! GH #571 — a `/colony/*` read is a work item with a name.
//!
//! GH #165 gave the colony loop a way to say what it is inside, and GH #439
//! taught the mutation path to use it: a trip that happens during a declared
//! operation reports `slow_work_item` under that operation's name instead of
//! `colony_loop`, and only the second of those verdicts ends the process.
//!
//! The READS never declared anything. `dispatch_colony_endpoint` received the
//! work pulse and handed it straight on to `handle_mutation`; for
//! `/colony/graph`, `/colony/registry`, `/colony/templates`, `/colony/trace` and
//! `/colony/ledger` it was never ticked and never labelled. A read that took long
//! was therefore pure silence — no name, no budget — and the supervisor read that
//! silence as a parked loop that had stopped answering. That is the fatal verdict
//! the issue measured at the top of every minute, when a display refresh takes a
//! `/colony/graph` read.
//!
//! Two halves, both positive:
//! * the load half — a thousand reads through the production call site never
//!   produce the incident's verdict;
//! * the naming half — a loop that goes quiet INSIDE a read is reported under the
//!   endpoint it was reading, on the same budget every other declared item gets.

use meclaw_colony::watchdog::{Beat, HEARTBEAT_CAPACITY, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, RespawnFn, colony_task,
};
use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, Headers, Message, Path, Uuid};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Nodes in the fixture topology — the order of magnitude of a real deployment,
/// so the read the loop is judged on is a real projection and not a two-entry
/// one.
const NODES: usize = 100;
/// Reads in the load half.
const READS: usize = 1_000;
/// The label the dispatcher must declare for the endpoint under test.
const LABEL: &str = "colony-read /colony/graph";

/// A registered stand-in for a cell: the colony holds a plain mailbox sender, the
/// test holds the receiver. Enough to be a routable node and a reply anchor
/// without booting a cell task.
struct Stub {
    rx: mpsc::Receiver<Message>,
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
        rx,
        _peace_tx: peace_tx,
        _backstop_tx: backstop_tx,
    }
}

async fn add_edge(inbox_tx: &mpsc::Sender<ColonyMsg>, from: Path, to: Path) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::AddEdge {
            id: Uuid::now_v7(),
            from,
            to,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx.await.expect("add_edge ack");
}

/// A colony of [`NODES`] registered stubs, chained by edges, plus the probe whose
/// out-edge lands on `/colony/graph` — the shipped shape of a read.
async fn boot(
    td: &std::path::Path,
    hb_tx: mpsc::Sender<Beat>,
) -> (
    mpsc::Sender<ColonyMsg>,
    mpsc::Sender<CellEmission>,
    Stub,
    tokio::task::JoinHandle<()>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel::<CellEmission>(256);
    let db = ColonyDb::open(&td.join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            CellFactoryRegistry::new(),
            td.to_path_buf(),
            // The stubs are mailboxes, not cell tasks: nothing ever acks a
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

    let probe = Path::new("/probe");
    let stub = register_stub(&inbox_tx, probe.clone()).await;
    add_edge(&inbox_tx, probe.clone(), Path::new("/colony/graph")).await;
    for i in 0..NODES {
        register_stub(&inbox_tx, Path::new(&format!("/n{i:03}"))).await;
    }
    for i in 1..NODES {
        add_edge(
            &inbox_tx,
            Path::new(&format!("/n{:03}", i - 1)),
            Path::new(&format!("/n{i:03}")),
        )
        .await;
    }
    (inbox_tx, outputs_tx, stub, colony_join)
}

async fn emit_read(outputs_tx: &mpsc::Sender<CellEmission>) {
    outputs_tx
        .send(CellEmission {
            sender_path: Path::new("/probe"),
            parent_message_id: Some(Uuid::now_v7()),
            trace_id: Uuid::now_v7(),
            input_ttl: 8,
            input_headers: Headers::default(),
            input_reply_to: None,
            target: Path::new("/sink"),
            content: json!({"messages": []}),
            direct_reply: false,
        })
        .await
        .expect("the outputs channel is the production emission path");
}

async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}

/// Half 1 — a thousand reads through the production call site, watched by a
/// supervisor on the shipped policy, never produce the incident's verdict.
///
/// The heartbeat runs at the PRODUCTION capacity, not at a test-only 1024: a
/// channel that drops the loop's newest word under burst is precisely how a
/// talking loop came to look like a silent one, so a test that sizes the channel
/// generously would measure a different world than the one that broke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thousand_reads_never_starve_the_loop() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, outputs_tx, mut stub, colony_join) = boot(td.path(), hb_tx).await;

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

    for _ in 0..READS {
        emit_read(&outputs_tx).await;
        let reply = tokio::time::timeout(Duration::from_secs(30), stub.rx.recv())
            .await
            .expect("every read answers within the failure-marker timeout")
            .expect("the reply mailbox stays open");
        match reply.body {
            meclaw_core::Body::Inline(v) => assert!(
                v["graph"]["nodes"].is_array(),
                "every read answers a topology: {v}"
            ),
            other => panic!("the graph reply is an inline body, got {other:?}"),
        }
    }

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

/// Half 2 — a loop that goes quiet inside a read is reported under the endpoint.
///
/// The silence is produced deterministically instead of raced for, exactly as
/// `gh439_a_large_instantiation_keeps_beating` does it: a relay forwards the
/// colony's real beats to the supervisor and stops as soon as it has passed on
/// the read's own declaration. From the supervisor's side that is a loop which
/// announced an operation and then said nothing more — the shape of the incident
/// — and it makes the trip a certainty rather than a timing accident.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trip_inside_a_read_names_the_endpoint() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (inbox_tx, outputs_tx, mut stub, colony_join) = boot(td.path(), hb_tx).await;

    let (relay_tx, relay_rx) = mpsc::channel::<Beat>(64);
    let (trip_tx, mut trip_rx) = mpsc::channel::<WatchdogTrip>(8);
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    let watchdog = tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        relay_rx,
        trip_tx,
        5,
        Duration::from_millis(10),
        armed_rx,
        // Log-only so a failure of this test is an assertion and not a process
        // that walks out; the fatality rule itself is asserted below.
        WatchdogOnTrip::LogOnly,
        None,
    ));
    let relay = tokio::spawn(async move {
        while let Some(b) = hb_rx.recv().await {
            let inside_the_read = matches!(&b, Beat::WorkingOn(w) if w.as_str() == LABEL);
            if relay_tx.send(b).await.is_err() {
                return;
            }
            if inside_the_read {
                // The loop has declared the read. From here it is silent — which
                // is what a read that does not return looks like from outside.
                // The sender is HELD (not dropped): a closed channel would be
                // read as `colony_task_gone`, a different finding.
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(relay_tx);
                return;
            }
        }
    });
    let _ = armed_tx.send(());

    emit_read(&outputs_tx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), stub.rx.recv()).await;

    let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
        .await
        .expect("a loop that stopped talking inside a declared read must trip")
        .expect("a trip");
    assert_eq!(
        trip.work_item.as_ref().map(|w| w.as_str()),
        Some(LABEL),
        "the trip names the endpoint it was reading: {trip}"
    );
    assert!(
        trip.in_flight_work,
        "the last word was a declared work item: {trip}"
    );
    assert_ne!(
        trip.starved(),
        "colony_loop",
        "a trip inside a declared read is never judged a parked loop: {trip}"
    );
    assert!(
        !trip.is_fatal(WatchdogOnTrip::Exit),
        "a slow declared read must not end the process under the shipped policy: {trip}"
    );

    shutdown(inbox_tx, colony_join).await;
    relay.abort();
    watchdog.abort();
}
