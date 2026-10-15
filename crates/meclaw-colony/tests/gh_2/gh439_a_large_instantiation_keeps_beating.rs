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
//!    stays untouched — nothing here widens it. Since GH #1176 the slot-table
//!    rebuild after the ack beats too, as its own `slot-rebuild …` work item;
//!    it is the only other label allowed, and only after the ack.
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

/// One beat as the test hears it, in channel order.
#[derive(Debug, Clone)]
pub enum Heard {
    /// A plain `Beat::Working`: the loop itself, at one of its structural
    /// points (top of an iteration, after taking an event). The mutation door
    /// never sends one — it only beats through its labelled pulse.
    Loop,
    /// A `Beat::WorkingOn` with its label.
    Label(String),
}

/// The two work items that may speak while a mutation is applied and acked.
const MUTATION: &str = "mutation ";
/// GH #1176: the slot-table rebuild after a topology change. It is its own
/// work item, run by the loop at the top of the iteration AFTER the one that
/// handled the mutation and sent its ack.
const SLOT_REBUILD: &str = "slot-rebuild ";

/// Instantiates the `big` subtree against a real `colony_task` and returns
/// every beat sent from just before the mutation until the test held its ack,
/// in channel order.
pub async fn beats_of_one_instantiation() -> Vec<Heard> {
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

    let mut heard: Vec<Heard> = Vec::new();
    while let Ok(b) = hb_rx.try_recv() {
        match b {
            Beat::WorkingOn(w) => heard.push(Heard::Label(w.as_str().to_string())),
            Beat::Working => heard.push(Heard::Loop),
            _ => {}
        }
    }

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), colony_join).await;
    heard
}

/// What half 1 demands of the beats of one instantiation.
///
/// Every labelled beat belongs to the mutation (`mutation …`) or to the slot
/// rebuild that follows it (`slot-rebuild …`, GH #1176); any other prefix is a
/// failure. The mutation alone carries the per-cell beats: they are counted
/// without the rebuild's, so the rebuild cannot stand in for a missing one.
///
/// **Order against the ack.** The ack itself is not a beat, but its place in
/// the stream is fixed by the loop: the mutation arm ends with `ack.send`, and
/// the next thing the loop sends is the plain `Working` at the top of the next
/// iteration — the door beats only through its labelled pulse. So a rebuild
/// beat after the ack sits after the last `mutation …` beat AND after a plain
/// loop beat that follows it. A rebuild beat anywhere before that point is a
/// rebuild that ran before the caller had its ack, and fails here.
pub fn assert_every_cell_is_declared(heard: &[Heard]) {
    let labelled: Vec<(usize, &str)> = heard
        .iter()
        .enumerate()
        .filter_map(|(i, h)| match h {
            Heard::Label(l) => Some((i, l.as_str())),
            Heard::Loop => None,
        })
        .collect();
    assert!(
        !labelled.is_empty(),
        "a running mutation must declare itself; no labelled beat was emitted"
    );
    assert!(
        labelled.iter().any(|(_, l)| l.starts_with(MUTATION)
            && l.contains("op=add_nodes")
            && l.contains("template=echo")),
        "a beat must name the cell being built; beats were {heard:?}"
    );
    assert!(
        labelled
            .iter()
            .all(|(_, l)| l.starts_with(MUTATION) || l.starts_with(SLOT_REBUILD)),
        "every label names the mutation it belongs to, or the slot rebuild after \
         it: {heard:?}"
    );
    // Once per staged cell AND once per registered cell — comfortably above the
    // leaf count. Asserted as a lower bound so the exact staging shape can move.
    // Only the per-cell beats count (`op=add_nodes[i/n]`): the template scan
    // (GH #1193) beats under the mutation's label too, once per template file
    // (`op=template-scan`), and its 25 announcements alone reached the leaf
    // count -- a mutation that dropped all but one cell beat stayed green
    // (re-check tb-s9b m1).
    let cell_beats = labelled
        .iter()
        .filter(|(_, l)| l.starts_with(MUTATION) && l.contains("op=add_nodes["))
        .count();
    assert!(
        cell_beats >= LEAVES,
        "one cell beat per cell at the very least: {cell_beats} `op=add_nodes[` \
         beats for {LEAVES} leaves"
    );
    let last_mutation = labelled
        .iter()
        .rev()
        .find(|(_, l)| l.starts_with(MUTATION))
        .map(|(i, _)| *i)
        .expect("counted above");
    let next_iteration = heard
        .iter()
        .enumerate()
        .skip(last_mutation + 1)
        .find(|(_, h)| matches!(h, Heard::Loop))
        .map(|(i, _)| i);
    for (i, l) in labelled.iter().filter(|(_, l)| l.starts_with(SLOT_REBUILD)) {
        assert!(
            next_iteration.is_some_and(|n| n < *i),
            "a slot rebuild speaks only after the mutation's ack (after the last \
             mutation beat and the loop's next-iteration beat); `{l}` came at \
             {i}, last mutation beat at {last_mutation}, next loop beat at \
             {next_iteration:?}: {heard:?}"
        );
    }
}

/// Half 1: the instantiation says what it is doing, cell by cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subtree_instantiation_declares_every_cell_it_builds() {
    let heard = beats_of_one_instantiation().await;
    assert_every_cell_is_declared(&heard);
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
    // Room for the trips the wait below skips: the supervisor `try_send`s and
    // drops a trip on a full channel, so a short channel filled by skipped
    // trips could swallow the one under test.
    let (trip_tx, mut trip_rx) = mpsc::channel::<WatchdogTrip>(64);
    // The exact label the relay went silent inside — the positive signal the
    // trip under test is matched against.
    let (silent_tx, silent_rx) = oneshot::channel::<String>();
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
            let inside_a_cell = match &b {
                Beat::WorkingOn(w) if w.as_str().contains("op=add_nodes") => {
                    Some(w.as_str().to_string())
                }
                _ => None,
            };
            if relay_tx.send(b).await.is_err() {
                return;
            }
            if let Some(label) = inside_a_cell {
                let _ = silent_tx.send(label);
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let declared = tokio::time::timeout_at(deadline, silent_rx)
        .await
        .expect("the mutation must declare a cell (`op=add_nodes`) within the failure marker")
        .expect("the relay names the cell it went silent inside");

    // The trip under test is the one that happened INSIDE the declared item, and
    // it is recognised by a positive signal: it carries the work item the loop
    // declared. Trips that arrive before the mutation has declared anything are
    // a different observation — the supervisor watching an idle loop on a busy
    // host — and they are skipped rather than asserted on. `LogOnly` keeps the
    // watchdog supervising after a non-fatal trip, so the labelled one still
    // comes.
    //
    // The earlier spelling asserted on the FIRST trip. On a loaded host the
    // colony task was scheduled late between the door taking the mutation and
    // its first `WorkingOn` beat, the 5 × 10 ms window closed on that gap, and
    // the test read `armed_for=51ms beats_seen=2 work_item=none` — a
    // host-scheduling observation presented as a broken diagnosis (GH #580).
    //
    // GH #1164: since GH #1176/#1193 the mutation also beats under its own
    // label BEFORE the first cell — the template scan and the validation's
    // hive-config scan (`mutation op=template-scan|hive-scan hive=…`), each
    // handed to `spawn_blocking` and awaited on the colony worker. On a loaded host the 5 × 10 ms window closes
    // on such a hand-off too, and that trip carries a work item; the earlier
    // spelling took the first trip WITH a work item and read
    // `op=template-scan` where it demanded `op=add_nodes`. Those trips are
    // correct diagnoses of a different moment. The trip under test is the one
    // whose work item is exactly the cell the relay went silent inside.
    //
    // **Why a real defect still fails this.** Nothing here is widened: the
    // window stays at 5 × 10 ms and every assertion below is unchanged. What is
    // demanded is the POSITIVE outcome — a trip that names the mutation. If the
    // mutation path stopped labelling its beats, or the supervisor lost the
    // label before the trip, no labelled trip is ever produced and the wait ends
    // red at the 30 s failure marker.
    let mut skipped = 0usize;
    let trip = loop {
        let t = tokio::time::timeout_at(deadline, trip_rx.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "a loop that stopped talking inside a declared item must trip with \
                     the item it was inside (`{declared}`); {skipped} trips arrived, \
                     none of them carrying it"
                )
            })
            .expect("a trip");
        if t.work_item.as_ref().map(|w| w.as_str()) == Some(declared.as_str()) {
            break t;
        }
        skipped += 1;
    };
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
