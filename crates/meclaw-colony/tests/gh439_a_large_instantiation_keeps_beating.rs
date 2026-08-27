//! GH #439 — a build order must not stop the colony.
//!
//! Since v0.25.0 a manifest applies to the RUNNING colony, so instantiating a
//! template IS a core operation of the builder flow. Before this lane it was a
//! single silent stretch: the loop declared nothing while it copied template
//! directories, wrote `config.json` files and registered cells, and the
//! supervisor judged the silence as `starved=colony_loop` — fatal under the
//! shipped `on_trip = exit`.
//!
//! Two halves, both proved POSITIVELY here against a REAL `colony_task`:
//!
//! 1. The mutation is audible: it beats once per cell it stages and once per
//!    cell it registers, under a label that names the operation. The window
//!    stays untouched — nothing here widens it.
//! 2. A trip that happens INSIDE such a declared item is diagnosed as one: it
//!    names the mutation instead of reporting `colony_loop`, and it is not
//!    fatal under the shipped policy.

use meclaw_colony::watchdog::{Beat, WatchdogOnTrip, WatchdogTrip};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, MutationOutcome,
    colony_task,
};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// How many leaf cells the instantiated subtree carries. Big enough that the
/// per-cell beats cannot be confused with the loop's own top-of-iteration one.
const LEAVES: usize = 24;

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    f
}

/// A root cell dir plus a `big` subtree template with [`LEAVES`] echo cells.
fn setup(dir: &std::path::Path) {
    let root_cell = dir.join("main");
    std::fs::create_dir_all(&root_cell).unwrap();
    std::fs::write(
        root_cell.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let tpl = dir.join("templates").join("big");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"big"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    for i in 0..LEAVES {
        let leaf = tpl.join(format!("c{i}"));
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(
            leaf.join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/main/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
    }
}

async fn rescan(inbox_tx: &mpsc::Sender<ColonyMsg>, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

fn add_big_subtree() -> meclaw_core::JsonValue {
    json!({
        "scope": "/",
        "ctx": {},
        "diff": {"add_nodes": [{"name": "stack", "template": "big"}]}
    })
}

/// Half 1: the instantiation says what it is doing, cell by cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subtree_instantiation_declares_every_cell_it_builds() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(4096);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            factories(),
            td.path().to_path_buf(),
            ColonyConfig::default(),
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));
    rescan(&inbox_tx, td.path().join("templates")).await;
    while hb_rx.try_recv().is_ok() {}

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload: add_big_subtree(),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("the mutation must answer")
        .expect("an outcome");
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the instantiation must commit: {outcome:?}"
    );

    let mut labelled: Vec<String> = Vec::new();
    while let Ok(b) = hb_rx.try_recv() {
        if let Beat::WorkingOn(w) = b {
            labelled.push(w.as_str().to_string());
        }
    }
    assert!(
        !labelled.is_empty(),
        "a running mutation must declare itself; no labelled beat was emitted"
    );
    assert!(
        labelled
            .iter()
            .any(|l| l.contains("op=add_nodes") && l.contains("template=echo")),
        "a beat must name the cell being built; labels were {labelled:?}"
    );
    assert!(
        labelled.iter().all(|l| l.starts_with("mutation ")),
        "every label names the mutation it belongs to: {labelled:?}"
    );
    // Once per staged cell AND once per registered cell — comfortably above the
    // leaf count. Asserted as a lower bound so the exact staging shape can move.
    assert!(
        labelled.len() >= LEAVES,
        "one beat per cell at the very least: {} beats for {LEAVES} leaves",
        labelled.len()
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}

/// Half 2: a trip inside the mutation names the mutation.
///
/// The silence is produced deterministically instead of being raced for: a
/// relay forwards the colony's real beats to the supervisor and stops as soon
/// as it has passed on the first LABELLED one. That is exactly the shape of the
/// incident — the loop declared an operation and then went quiet inside it —
/// and it makes the trip a certainty rather than a timing accident. The label
/// under test is produced by the production mutation path, not by the test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trip_inside_a_mutation_names_the_mutation() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(4096);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            factories(),
            td.path().to_path_buf(),
            ColonyConfig::default(),
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));
    rescan(&inbox_tx, td.path().join("templates")).await;

    // The supervisor, on a tight window and `log-only` so a failure of this test
    // is an assertion and not a dead process.
    let (relay_tx, relay_rx) = mpsc::channel::<Beat>(64);
    let (trip_tx, mut trip_rx) = mpsc::channel::<WatchdogTrip>(8);
    let (armed_tx, armed_rx) = oneshot::channel::<()>();
    tokio::spawn(meclaw_colony::watchdog::run_watchdog(
        relay_rx,
        trip_tx,
        5,
        Duration::from_millis(10),
        armed_rx,
        WatchdogOnTrip::LogOnly,
        None,
    ));
    tokio::spawn(async move {
        while let Some(b) = hb_rx.recv().await {
            // Stop once the loop has declared a CELL of the instantiation — that
            // is the point at which the real incident went quiet, deep inside a
            // build order rather than at its first breath.
            let inside_a_cell =
                matches!(&b, Beat::WorkingOn(w) if w.as_str().contains("op=add_nodes"));
            if relay_tx.send(b).await.is_err() {
                return;
            }
            if inside_a_cell {
                // The loop has declared an operation. From here it is silent —
                // which is what a long build order looks like from outside. The
                // sender is HELD (not dropped): a closed channel would be read
                // as `colony_task_gone`, which is a different finding.
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(relay_tx);
                return;
            }
        }
    });
    let _ = armed_tx.send(());

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload: add_big_subtree(),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;

    let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
        .await
        .expect("a loop that stopped talking inside a declared item must trip")
        .expect("a trip");
    assert!(
        trip.in_flight_work,
        "the last word was a declared work item: {trip}"
    );
    assert_ne!(
        trip.starved(),
        "colony_loop",
        "a trip inside a declared mutation is never judged a parked loop: {trip}"
    );
    assert!(
        !trip.is_fatal(WatchdogOnTrip::Exit),
        "a slow declared item must not end the process under the shipped policy: {trip}"
    );
    let label = trip
        .work_item
        .as_ref()
        .map(|w| w.as_str().to_string())
        .unwrap_or_default();
    assert!(
        label.starts_with("mutation ") && label.contains("op=add_nodes"),
        "the trip must name the mutation it was inside, was: {trip}"
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
}
