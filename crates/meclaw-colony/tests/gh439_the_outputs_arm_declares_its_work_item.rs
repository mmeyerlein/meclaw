//! GH #439 — regression lock: an event taken from the OUTPUTS channel is a work
//! item like any other, and the loop has to say so before it blocks.
//!
//! Before the fix the loop beat `Parked` right before the `select!`, entered the
//! `outputs_rx` arm and did the whole handling — a cell-emitted mutation (the
//! builder/`submit` flow) included — without ever declaring `Working`. The
//! supervisor's last observed phase therefore stayed `Parked`, `in_flight_work`
//! was `false`, `WatchdogTrip::starved()` returned `colony_loop`, and that
//! verdict is fatal under the shipped `on_trip = exit`: a build order killed the
//! colony. That is the line from the issue body.
//!
//! The proof is POSITIVE and structural, not a sleep and not "the DLQ stayed
//! empty": the loop beats `Working` at the top of every iteration and `Parked`
//! before every `select!`, so an idle stream STRICTLY ALTERNATES (that is what
//! `gh165_the_loop_declares_its_work_item` pins). A select ARM that declares its
//! own work item is therefore the only thing that can produce two `Working`-class
//! beats in a row: the arm's declaration, then the top of the next iteration.
//! This test observes exactly that adjacency, and it observes it only after an
//! emission was pushed through the outputs channel.

use meclaw_colony::watchdog::Beat;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, colony_task,
};
use meclaw_core::{CellEmission, Headers, Path, Uuid, serde_json::json};
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

/// Every beat that declares work, whatever its label (Task 3 turns some of them
/// into `Beat::WorkingOn`).
fn is_working(b: &Beat) -> bool {
    !matches!(b, Beat::Parked)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emission_taken_from_the_outputs_channel_is_declared_as_work() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel::<CellEmission>(64);
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(1024);
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

    // Phase 1 — the idle baseline. Collect a stretch of beats with NOTHING in the
    // outputs channel; it must alternate strictly, so the adjacency asserted in
    // phase 2 cannot come from the idle loop.
    let mut idle: Vec<Beat> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while tokio::time::Instant::now() < deadline && idle.len() < 12 {
        match tokio::time::timeout_at(deadline, hb_rx.recv()).await {
            Ok(Some(b)) => idle.push(b),
            _ => break,
        }
    }
    assert!(
        idle.len() >= 8,
        "an idle colony must keep beating; got {} beats: {idle:?}",
        idle.len()
    );
    assert!(
        !idle
            .windows(2)
            .any(|p| is_working(&p[0]) && is_working(&p[1])),
        "an idle loop alternates Working/Parked — a doubled declaration here \
         would make phase 2 meaningless: {idle:?}"
    );

    // Phase 2 — exactly one emission through the production outputs channel. It
    // is unroutable (no sender in the registry), so it takes the arm's `no_route`
    // path and `continue`s; the declaration must have happened before that.
    outputs_tx
        .send(CellEmission {
            sender_path: Path::new("/nobody"),
            parent_message_id: None,
            trace_id: Uuid::now_v7(),
            input_ttl: 8,
            input_headers: Headers::default(),
            input_reply_to: None,
            target: Path::new("/nowhere"),
            content: json!({"body": {"messages": [{"role": "user", "text": "x"}]}}),
            direct_reply: false,
        })
        .await
        .expect("the outputs channel is the production emission path");

    let mut after: Vec<Beat> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while tokio::time::Instant::now() < deadline && after.len() < 24 {
        match tokio::time::timeout_at(deadline, hb_rx.recv()).await {
            Ok(Some(b)) => after.push(b),
            _ => break,
        }
    }
    assert!(
        after
            .windows(2)
            .any(|p| is_working(&p[0]) && is_working(&p[1])),
        "the outputs arm must declare its work item before it blocks — the only \
         way two Working-class beats can follow each other is an arm that beat \
         and then the top of the next iteration; beats were {after:?}"
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}
