//! GH #47: a colony that is dying does not accept a build order.
//!
//! Since v0.25.0 a manifest applies to the RUNNING colony, so a builder cell can
//! legitimately emit one at any moment — including into a drain. Spawning cells
//! while the substrate tears itself down is the failure mode the roadmap named;
//! this is the refusal that prevents it, and it is a NAMED refusal, not a
//! dropped ack.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, MutationOutcome,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Cell, CellOutput, Message, OutputSink, Path, Uuid};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Failure marker, generous per the 30s convention (robust under cargo load).
const MARKER: Duration = Duration::from_secs(30);

/// The drain budget every colony here is given. Far larger than anything the
/// tests wait for, so "the door was closed" can never be confused with "the
/// drain had already ended".
const DRAIN_BUDGET_MS: u64 = 30_000;

/// SEMANTIC discriminator: the shutdown must still be running while the blocked
/// handler holds it. Tight on purpose — a sixtieth of the drain budget. Anything
/// longer would also pass on a colony that is merely slow.
const HELD_PROBE: Duration = Duration::from_millis(500);

// ── topology helpers (same shape as the gh276 suite) ─────────────────────────

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_cell(td: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(td.join(rel)).expect("create the cell dir");
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .expect("write the cell config");
}

fn hive(td: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(td.join(rel)).expect("create the hive dir");
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .expect("write the hive config");
}

/// A colony root that carries the drain budget plus one bootstrapped `echo`
/// cell at `/a` — the cell every "nothing was created" claim below is measured
/// against, because a boot that finds it is a boot that really read the tree.
fn colony_root() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#),
    )
    .expect("write the test colony.json");
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td.path(), "main/a", "/a");
    td
}

/// The build order the drain refuses: one node from the `echo` template. Nothing
/// in it is ever read — the door closes before the body is looked at — but it is
/// the shape a builder cell really emits, so what is refused is a real order.
fn add_one_node() -> Value {
    json!({"scope": "/", "diff": {"add_nodes": [{"name": "newcell", "template": "echo"}]}})
}

/// The same order as a cell EMITS it: one universal body, with the diff in its
/// own top-level slot next to the (empty) `messages` array every emission
/// carries.
fn add_one_node_as_emission() -> Value {
    json!({
        "messages": [],
        "scope": "/",
        "diff": {"add_nodes": [{"name": "newcell", "template": "echo"}]}
    })
}

/// Every directory named `newcell` anywhere under the root. The positive half of
/// the pair is [`registry_paths`]: a boot that lists `/a` is a boot that looked.
fn dirs_named(root: &std::path::Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if entry.file_name() == name {
                    found.push(entry.path());
                }
                stack.push(entry.path());
            }
        }
    }
    found
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("the colony inbox takes a read");
    let mut paths: Vec<String> = ack_rx
        .await
        .expect("the colony answers a registry read")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    paths.sort();
    paths
}

/// Puts the colony into its drain and hands back its inbox plus the task that
/// waits for the shutdown to finish.
///
/// The `Shutdown` goes into the inbox HERE, from the test's own task, and only
/// the WAITING is spawned. `ColonyHandle::shutdown` sends it as its first step,
/// so the older `tokio::spawn(async move { h.shutdown().await })` merely
/// PROMISED to enqueue it: under load the spawned task can still be waiting for
/// a worker while the barrier read below is already in the inbox. The colony
/// then answers that read — and everything queued behind it — before it has
/// ever seen the shutdown, and every "during the drain" assertion silently
/// measures a live colony instead. Measured on 2026-08-28 with the CPUs
/// saturated: 4 of 30 runs of the first test refused the build order with
/// `template_missing` (the live door's verdict) instead of `shutdown_draining`.
/// Sending it from here is what makes the FIFO argument of
/// [`wait_until_draining`] true.
async fn start_drain(h: ColonyHandle) -> (mpsc::Sender<ColonyMsg>, tokio::task::JoinHandle<()>) {
    let inbox_tx = h.inbox_tx.clone();
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .expect("the colony inbox takes the shutdown");
    let waiting = tokio::spawn(async move {
        // Same order as `ColonyHandle::shutdown`: the ack lands when the drain
        // is over, the join when the colony task is gone.
        let _ = ack_rx.await;
        let _ = h.join_result().await;
    });
    (inbox_tx, waiting)
}

/// Barrier: the colony inbox is FIFO and the loop is one task, so an ack for a
/// LATER message proves the shutdown was handled first — the colony is draining
/// from here on. `ReadLiveness` is a pure read and changes nothing the drain
/// sees.
async fn wait_until_draining(inbox: &mpsc::Sender<ColonyMsg>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox
        .send(ColonyMsg::ReadLiveness { ack: ack_tx })
        .await
        .expect("the inbox stays OPEN during the drain — that is the point of it");
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("the drain must keep answering reads within the failure marker")
        .expect("the colony answers the barrier read");
}

/// A cell that holds its first `handle()` open until the test releases it, then
/// emits one message; every LATER message is handed to `replies`.
///
/// The two jobs are one cell on purpose: the emission has to leave a handler
/// that is running INSIDE the drain, and the answer to it has to be caught by
/// the same cell it was addressed to.
///
/// The `Arc<Mutex<..>>` around the receiver is TEST scaffolding: `spawn` wants a
/// `Fn() -> C` factory that may run more than once, and a `oneshot::Receiver` is
/// not `Clone`. The no-lock rule of `AGENTS.md` governs cell and colony state in
/// the substrate, not a test's own handle onto its trigger.
struct EmitOnReleaseCell {
    entered: mpsc::Sender<()>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    emits: Vec<(Path, Value)>,
    replies: mpsc::Sender<Message>,
    emitted: bool,
}

impl Cell for EmitOnReleaseCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(
        &mut self,
        msg: Message,
        sink: &OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send {
        let first = !self.emitted;
        self.emitted = true;
        let entered = self.entered.clone();
        let release = self.release.clone();
        let emits = self.emits.clone();
        let replies = self.replies.clone();
        let sink = sink.clone();
        async move {
            if !first {
                let _ = replies.send(msg).await;
                return;
            }
            // Taken in its own scope: the guard must be gone before the first
            // `.await`, or this future would not be `Send`.
            let waiter = {
                let mut slot: MutexGuard<'_, Option<oneshot::Receiver<()>>> =
                    release.lock().expect("the release slot is never poisoned");
                slot.take()
            };
            let _ = entered.send(()).await;
            if let Some(waiter) = waiter {
                let _ = waiter.await;
            }
            for (target, content) in emits {
                let _ = sink.push(CellOutput { target, content }).await;
            }
        }
    }
}

fn body_of(msg: &Message) -> &Value {
    match &msg.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("the colony answers a read with an inline body"),
    }
}

/// Every dead letter `colony.db` holds, as `(error_code, sender_path)`. Read in
/// full rather than counted, so a miss says what WAS recorded instead of only
/// that the expected row is absent.
fn dead_letters(db: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db)
        .expect("a fresh connection must open the file the joined writer left behind");
    let mut stmt = conn
        .prepare("SELECT error_code, sender_path FROM dead_letters ORDER BY id")
        .expect("the dead_letters table must exist in colony.db");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query the dead letters")
        .map(|r| r.expect("read a dead-letter row"))
        .collect();
    rows
}

/// A mutation arriving during the drain is refused with `shutdown_draining`, and
/// nothing is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_that_arrives_during_the_drain_creates_nothing() {
    let td = colony_root();
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    // A cell whose handler is still running: the drain has real work to wait
    // for, so it is long, and the build order below meets a colony that is
    // genuinely mid-drain rather than one that is already gone.
    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (replies_tx, _replies_rx) = mpsc::channel::<Message>(4);
    let release = Arc::new(Mutex::new(Some(release_rx)));
    h.spawn(Path::new("/blocker"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        let replies = replies_tx.clone();
        move || EmitOnReleaseCell {
            entered: entered.clone(),
            release: release.clone(),
            emits: vec![(Path::new("/a"), json!({"messages": []}))],
            replies: replies.clone(),
            emitted: false,
        }
    })
    .await;
    h.send_from(Path::new("/"), MessageBuilder::new("/blocker").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the blocking handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let (inbox_tx, mut shutting_down) = start_drain(h).await;
    wait_until_draining(&inbox_tx).await;

    // The build order, at the door that a `--apply` run and the HTTP POST both
    // knock at.
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload: add_one_node(),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the inbox takes a mutation during the drain — and refuses it");
    let outcome = tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("a refusal is an ANSWER: the door must reply within the marker")
        .expect("the colony answers the mutation door");

    match &outcome {
        MutationDoorOutcome::Single(MutationOutcome::Rejected {
            error_code,
            details,
            ..
        }) => {
            assert_eq!(
                error_code, "shutdown_draining",
                "a build order in a dying colony is refused BY NAME, not by \
                 whatever the diff would have failed on: {details}"
            );
        }
        other => panic!("the drain must refuse a build order, got {other:?}"),
    }

    // The drain was still running while all of that happened — otherwise the
    // refusal above would say nothing about a draining colony.
    assert!(
        tokio::time::timeout(HELD_PROBE, &mut shutting_down)
            .await
            .is_err(),
        "the shutdown must still be running while the blocked handler holds it"
    );
    release_tx
        .send(())
        .expect("the blocked handler still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the handler is released")
        .expect("the shutdown task must end normally, never by panic");

    // Spurless, on disk: the refusal happens before anything is staged, so there
    // is nothing to roll back and nothing left over.
    let created = dirs_named(td.path(), "newcell");
    assert!(
        created.is_empty(),
        "a refused build order leaves no cell directory behind: {created:?}"
    );

    // And spurless to the next boot, which is the reading that matters: a fresh
    // colony over the same root finds exactly the tree that was there before.
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("the second boot reads the same root");
    let paths = registry_paths(&h2).await;
    assert!(
        paths.iter().any(|p| p == "/a"),
        "the fresh boot must really have read the tree — /a is the proof: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("newcell")),
        "the refused node reaches no registry of the next boot either: {paths:?}"
    );
    h2.shutdown().await;
}

/// The read half must NOT be refused: a cell mid-turn may still ask
/// `/colony/graph`, and answering it is not new work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_read_during_the_drain_is_still_answered() {
    let td = colony_root();
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (replies_tx, mut replies_rx) = mpsc::channel::<Message>(4);
    let release = Arc::new(Mutex::new(Some(release_rx)));
    h.spawn(Path::new("/probe"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        let replies = replies_tx.clone();
        move || EmitOnReleaseCell {
            entered: entered.clone(),
            release: release.clone(),
            emits: vec![(Path::new("/colony/graph"), json!({"messages": []}))],
            replies: replies.clone(),
            emitted: false,
        }
    })
    .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/probe").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the probe handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let (inbox_tx, shutting_down) = start_drain(h).await;
    wait_until_draining(&inbox_tx).await;

    // The cell is mid-turn INSIDE the drain and asks the colony where it is.
    release_tx
        .send(())
        .expect("the probe still holds the release receiver");
    let reply = tokio::time::timeout(MARKER, replies_rx.recv())
        .await
        .expect("a read during the drain must be answered within the failure marker")
        .expect("the reply channel sender lives in the cell");
    let body = body_of(&reply);
    assert!(
        body.get("graph").is_some(),
        "the answer arrives in the slot the question named: {body}"
    );

    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the answered turn is over")
        .expect("the shutdown task must end normally, never by panic");
}

/// The same door, reached the other way: a builder cell emits its build order
/// into the drain. That emission has a parent — it is the work being drained,
/// so the source refusal of task 13 lets it through — and it is refused here,
/// where the task would otherwise be started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_emitted_mutation_during_the_drain_is_dead_lettered() {
    let td = colony_root();
    let db_path = td.path().join("colony.db");
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    let (entered_tx, mut entered_rx) = mpsc::channel::<()>(4);
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (replies_tx, _replies_rx) = mpsc::channel::<Message>(4);
    let release = Arc::new(Mutex::new(Some(release_rx)));
    h.spawn(Path::new("/builder"), {
        let entered = entered_tx.clone();
        let release = release.clone();
        let replies = replies_tx.clone();
        move || EmitOnReleaseCell {
            entered: entered.clone(),
            release: release.clone(),
            // TWO orders, one per call site of `dispatch_colony_endpoint`:
            // the first is re-enqueued as a `Route` (a cell emission to a
            // `/colony/*` endpoint is dispatched before `apply_edges`), the
            // second travels as an ordinary emission and only becomes a build
            // order through the out-edge below. Both doors are the same
            // function; both are shut.
            emits: vec![
                (Path::new("/colony/mutations"), add_one_node_as_emission()),
                (Path::new("/sink"), add_one_node_as_emission()),
            ],
            replies: replies.clone(),
            emitted: false,
        }
    })
    .await;
    // The out-edge that turns the second, ordinary emission into a build order:
    // it resolves to the mutations endpoint only AFTER `apply_edges`, which is
    // the other call site of the dispatcher.
    h.add_edge(
        Uuid::now_v7(),
        Path::new("/builder"),
        Path::new("/colony/mutations"),
    )
    .await;

    h.send_from(Path::new("/"), MessageBuilder::new("/builder").build())
        .await;
    tokio::time::timeout(MARKER, entered_rx.recv())
        .await
        .expect("the builder handler must be entered within the failure marker")
        .expect("the entered-channel sender lives in the cell");

    let (inbox_tx, shutting_down) = start_drain(h).await;
    wait_until_draining(&inbox_tx).await;

    release_tx
        .send(())
        .expect("the builder still holds the release receiver");
    tokio::time::timeout(MARKER, shutting_down)
        .await
        .expect("the shutdown must return once the builder's turn is over")
        .expect("the shutdown task must end normally, never by panic");

    let recorded = dead_letters(&db_path);
    assert_eq!(
        recorded,
        vec![
            ("shutdown_draining".to_string(), "/builder".to_string()),
            ("shutdown_draining".to_string(), "/builder".to_string()),
        ],
        "BOTH orders are refused at the endpoint door — the one dispatched \
         straight from the emission and the one an out-edge turned into a \
         mutation — and each is recorded under the builder's own name"
    );
    let created = dirs_named(td.path(), "newcell");
    assert!(
        created.is_empty(),
        "and the refused order builds nothing: {created:?}"
    );
}
