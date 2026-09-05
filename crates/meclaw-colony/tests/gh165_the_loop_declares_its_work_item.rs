//! GH #165: the colony loop says which phase it is in, and it says it BEFORE it
//! can block.
//!
//! The watchdog's whole false-positive problem was that silence carried no cause:
//! a loop wedged in a deadlock and a loop halfway through a legitimate half-second
//! operation produced the identical observation, and `on_trip=exit` killed both.
//! The supervisor can only tell them apart if the loop declares its work item
//! while it is still able to speak — a blocked loop reports nothing, so the report
//! has to happen on the way in.
//!
//! This is the receipt that the declaration is real and comes from the production
//! loop, not from a test fixture: a REAL `colony_task` is booted with a heartbeat
//! channel and its own beats are read off the wire.
//!
//! The load-bearing half is the SECOND assertion. `Working` is easy — the loop
//! emits it at the top of every iteration. `Parked` is what keeps the watchdog a
//! watchdog: an idle loop must end every iteration by saying it has nothing in
//! flight, otherwise the work-item grace period would cover a colony that is
//! merely quiet and the detector would be off.

use meclaw_colony::watchdog::Beat;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, colony_task,
};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    f
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_colony_loop_alternates_working_and_parked() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(256);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            factories(),
            root.to_path_buf(),
            ColonyConfig::default(),
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));

    // WANTED beats: twelve full iterations of an idle loop. The wait is bounded
    // by the repo's 30 s failure-marker convention, NOT by the cadence — the
    // loop wakes itself ~10×/s while idle, so this is a fifth of a second of
    // work inside a thirty-second window.
    //
    // The earlier spelling counted whatever arrived inside a 1.2 s wall-clock
    // window and demanded at least eight. That made the ASSERTION a cadence
    // measurement, and on a host under parallel build load it read six beats
    // and called a healthy loop broken (GH #579). Cadence is not what this test
    // is about: the claim in the name is that the loop ALTERNATES, and the
    // supporting claim is that an idle loop keeps beating at all.
    //
    // **Why a real defect still fails this.** The failure this guards is a loop
    // that stops beating — wedged in an `.await`, or gone with its task. Such a
    // loop delivers nothing, so it can never reach `WANTED` and the wait ends
    // red at the marker. The same holds for a loop that beats once and then
    // stops. What no longer fails it is a loop that beats late.
    const WANTED: usize = 24;
    let mut beats: Vec<Beat> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while beats.len() < WANTED {
        match tokio::time::timeout_at(deadline, hb_rx.recv()).await {
            Ok(Some(b)) => beats.push(b),
            Ok(None) => panic!(
                "the colony loop dropped its heartbeat sender after {} beats: {beats:?}",
                beats.len()
            ),
            Err(_) => panic!(
                "an idle colony must keep beating; only {} of {WANTED} beats arrived \
                 within the failure marker: {beats:?}",
                beats.len()
            ),
        }
    }

    assert!(
        beats.contains(&Beat::Working),
        "the loop must declare the work item it is about to enter: {beats:?}"
    );
    assert!(
        beats.contains(&Beat::Parked),
        "an idle loop must declare that it has NOTHING in flight — without this \
         the work-item grace period would cover a merely quiet colony: {beats:?}"
    );
    // Strict alternation: every declared work item is closed by a park, and every
    // park is opened by a declaration. A repeated `Working` would mean the loop
    // entered a second item without closing the first (the supervisor would keep
    // granting the larger budget); a repeated `Parked` would mean an iteration
    // ran without ever declaring the work it did.
    for pair in beats.windows(2) {
        assert_ne!(
            pair[0], pair[1],
            "phases must alternate, got {pair:?} inside {beats:?}"
        );
    }

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}
