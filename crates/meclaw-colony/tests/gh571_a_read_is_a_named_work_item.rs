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
//!
//! GH #968 (quarantined until then: no trip within 30 s, once, under suite
//! load): the read declares itself with ONE labelled beat, sent with
//! `try_send`. A heartbeat channel that is full at that moment dropped it — the
//! boot leaves hundreds of beats queued behind a supervisor that is not armed
//! yet — so the relay below, which waited for that very beat, never went
//! silent, and the trip never came. In production the same drop turns a named
//! read into `starved=colony_loop`. The label now has a slot of its own
//! (`watchdog::LabelSlot`, last label wins); the relay watches the slot too,
//! and the third test fills the channel on purpose and still gets the name.

use meclaw_colony::watchdog::{
    Beat, HEARTBEAT_CAPACITY, LabelSlot, LabelSlotReader, WatchdogOnTrip, WatchdogTrip, label_slot,
};
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
    slot: Option<LabelSlot>,
) -> (
    mpsc::Sender<ColonyMsg>,
    mpsc::Sender<CellEmission>,
    Stub,
    tokio::task::JoinHandle<()>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel::<CellEmission>(256);
    let db = ColonyDb::open(&td.join("colony.db")).expect("open colony.db");
    let mut cfg = meclaw_colony::ColonyTaskConfig::new(
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
    .with_heartbeat(hb_tx);
    if let Some(slot) = slot {
        cfg = cfg.with_label_slot(slot);
    }
    let colony_join = tokio::spawn(colony_task(cfg));

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
    let (inbox_tx, outputs_tx, mut stub, colony_join) = boot(td.path(), hb_tx, None).await;

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

/// What the relay between the colony and the supervisor is told.
enum RelayCmd {
    /// Stop reading the colony's beats from now on, and say so.
    Pause(oneshot::Sender<()>),
}

/// The relay of halves 2 and 3: forwards the colony's real beats to the
/// supervisor and goes silent for good when the read has declared itself —
/// seen on the channel OR in the label slot, because under load the channel
/// may have refused the declaration (GH #968) — or when the test pauses it.
/// The sender is HELD, never dropped: a closed channel would be read as
/// `colony_task_gone`, a different finding.
fn spawn_relay(
    mut hb_rx: mpsc::Receiver<Beat>,
    relay_tx: mpsc::Sender<Beat>,
    mut slot: LabelSlotReader,
    mut cmd_rx: mpsc::Receiver<RelayCmd>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                cmd = cmd_rx.recv() => {
                    if let Some(RelayCmd::Pause(ack)) = cmd {
                        let _ = ack.send(());
                    }
                    break;
                }
                label = slot.changed() => {
                    if label.is_some_and(|w| w.as_str() == LABEL) {
                        break;
                    }
                }
                b = hb_rx.recv() => {
                    let Some(b) = b else { return };
                    let inside_the_read = matches!(&b, Beat::WorkingOn(w) if w.as_str() == LABEL);
                    if relay_tx.send(b).await.is_err() {
                        return;
                    }
                    if inside_the_read {
                        break;
                    }
                }
            }
        }
        // Silent from here: what a read that does not return looks like from
        // outside. Both channels stay open until the test aborts the relay.
        let _hold = (hb_rx, relay_tx);
        std::future::pending::<()>().await;
    })
}

/// The first trip that names the read, or a panic after the 30-s failure
/// marker. A trip before the declaration reached the supervisor (the loaded
/// host gave the window away before the read was picked up, GH #748) is not
/// the finding under test and is passed over; a supervisor that never hears
/// the name never produces one.
///
/// What this gives up (review M-3 of #968): the older claim "the FIRST trip
/// names the read" -- an unnamed `colony_loop` trip ahead of the named one no
/// longer fails the test, because under `LogOnly` follow-up trips are normal
/// (GH #748). The claim that stays is the positive one: a trip naming the read
/// arrives within 30 s.
async fn trip_naming_the_read(trip_rx: &mut mpsc::Receiver<WatchdogTrip>) -> WatchdogTrip {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let trip = trip_rx.recv().await.expect("the supervisor reports");
            if trip.work_item.as_ref().map(|w| w.as_str()) == Some(LABEL) {
                return trip;
            }
        }
    })
    .await
    .expect("a loop that stopped talking inside a declared read must trip under its name")
}

fn assert_named_read_trip(trip: &WatchdogTrip) {
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
}

/// The colony, its supervisor (slot wired) and the relay between them.
struct Rig {
    inbox_tx: mpsc::Sender<ColonyMsg>,
    outputs_tx: mpsc::Sender<CellEmission>,
    stub: Stub,
    colony_join: tokio::task::JoinHandle<()>,
    hb_tx: mpsc::Sender<Beat>,
    trip_rx: mpsc::Receiver<WatchdogTrip>,
    cmd_tx: mpsc::Sender<RelayCmd>,
    relay: tokio::task::JoinHandle<()>,
    watchdog: tokio::task::JoinHandle<()>,
    _td: tempfile::TempDir,
}

/// Five periods of `period_ms`: the window. A declared work item gets
/// `WORK_ITEM_BUDGET_FACTOR` × the window, an order of magnitude above it, so
/// `starved()` reads `slow_work_item` for the named trip.
async fn rig(period_ms: u64) -> Rig {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (hb_tx, hb_rx) = mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (slot, reader) = label_slot();
    let (inbox_tx, outputs_tx, stub, colony_join) =
        boot(td.path(), hb_tx.clone(), Some(slot)).await;

    let (relay_tx, relay_rx) = mpsc::channel::<Beat>(64);
    // The arming probe: the relay channel is filled before the supervisor is
    // armed, and the supervisor is its only reader -- and reads nothing until
    // armed. Room in it is therefore the event "the supervisor has armed and
    // dropped what came before". Without it, a read declared while a loaded
    // host had not yet run the arming step was discarded with the stale boot
    // beats, and no trip ever named it (load run 9/10 of strand X, GH #968).
    let probe_tx = relay_tx.clone();
    while probe_tx.try_send(Beat::Parked).is_ok() {}
    let (trip_tx, trip_rx) = mpsc::channel::<WatchdogTrip>(64);
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<RelayCmd>(1);
    let relay = spawn_relay(hb_rx, relay_tx, reader.observe(), cmd_rx);
    let watchdog = tokio::spawn(meclaw_colony::watchdog::run_watchdog_with_label_slot(
        relay_rx,
        trip_tx,
        5,
        Duration::from_millis(period_ms),
        armed_rx,
        // Log-only so a failure of this test is an assertion and not a process
        // that walks out; the fatality rule itself is asserted.
        WatchdogOnTrip::LogOnly,
        None,
        Some(reader),
    ));
    let _ = armed_tx.send(());
    let room = tokio::time::timeout(Duration::from_secs(30), probe_tx.reserve())
        .await
        .expect("the supervisor arms within the failure-marker timeout")
        .expect("the supervisor holds the relay channel");
    drop(room);
    drop(probe_tx);
    Rig {
        inbox_tx,
        outputs_tx,
        stub,
        colony_join,
        hb_tx,
        trip_rx,
        cmd_tx,
        relay,
        watchdog,
        _td: td,
    }
}

impl Rig {
    async fn finish(self) {
        drop(self.hb_tx);
        shutdown(self.inbox_tx, self.colony_join).await;
        self.relay.abort();
        self.watchdog.abort();
    }
}

/// Half 2 — a loop that goes quiet inside a read is reported under the endpoint.
///
/// The silence is produced deterministically instead of raced for, exactly as
/// `gh439_a_large_instantiation_keeps_beating` does it: a relay forwards the
/// colony's real beats to the supervisor and stops as soon as the read has
/// declared itself. From the supervisor's side that is a loop which announced an
/// operation and then said nothing more — the shape of the incident — and it
/// makes the trip a certainty rather than a timing accident.
///
/// Five periods of 50 ms: a 250 ms window, a 2 500 ms budget. The window used
/// to be 50 ms, BELOW the colony's own idle beat of 100 ms, and one 49 ms
/// scheduling gap on a shared CI runner tripped it before the read had been
/// picked up (GH #748). GH #968: the relay used to wait for the declaration on
/// the channel alone, and a full channel had dropped it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trip_inside_a_read_names_the_endpoint() {
    let mut rig = rig(50).await;
    emit_read(&rig.outputs_tx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), rig.stub.rx.recv()).await;

    let trip = trip_naming_the_read(&mut rig.trip_rx).await;
    assert_named_read_trip(&trip);
    rig.finish().await;
}

/// Half 3 (GH #968) — the same trip, with the heartbeat channel FULL when the
/// read declares itself: the relay is paused (nothing reads the channel any
/// more) and the test tops the channel up to its production capacity, so the
/// read's labelled beat is refused by construction. The supervisor learns the
/// name from the slot. Red before the slot: the last word stays the last beat
/// the relay forwarded, every trip says `colony_loop` and none names the read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trip_inside_a_read_names_the_endpoint_under_a_full_channel() {
    // 100 ms periods: the window (500 ms) holds the colony's idle beat five
    // times over while the relay still forwards.
    let mut rig = rig(100).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    rig.cmd_tx
        .send(RelayCmd::Pause(ack_tx))
        .await
        .expect("relay running");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("the relay pauses within the failure-marker timeout")
        .expect("pause ack");
    while rig.hb_tx.try_send(Beat::Parked).is_ok() {}
    assert_eq!(rig.hb_tx.capacity(), 0, "the heartbeat channel is full");

    emit_read(&rig.outputs_tx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), rig.stub.rx.recv()).await;

    let trip = trip_naming_the_read(&mut rig.trip_rx).await;
    assert_named_read_trip(&trip);
    rig.finish().await;
}
