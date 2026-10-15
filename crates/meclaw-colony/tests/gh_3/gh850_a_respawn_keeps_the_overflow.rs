//! GH #850 (R-SN-5) — a cell that dies in the middle of draining its overflow
//! loses nothing and reorders nothing.
//!
//! The cell reads ten messages, stops reading (its mailbox of four refills,
//! the rest waits in its overflow), then panics. Its mailbox guard hands the
//! four unread messages to the colony (GH #18) before the watcher reports the
//! death; the corridor respawns the cell; and at the call site after the
//! corridor the rescued four go to the FRONT of the overflow — they are older —
//! and the drain task is pointed at the new mailbox. The successor must see
//! every message after the tenth, in the order sent.
//!
//! Before the overflow the colony blocked on the full mailbox, and the dying
//! cell's closed receiver made that blocked send fail: the message was dropped
//! with a `warn!` (`route send failed (receiver dropped)`).

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, RespawnFn,
    colony_task,
};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const MARKER: Duration = Duration::from_secs(30);
const CAPACITY: usize = 4;
const READ_BEFORE_DEATH: usize = 10;
const TOTAL: usize = 100;

/// The first life of the cell: reads `READ_BEFORE_DEATH` messages, then waits
/// for the test's word and dies the way a real cell task does — its mailbox
/// guard closes the receiver, drains the rest and hands it to the colony, then
/// the task panics.
fn first_life(
    path: Path,
    mut rx: mpsc::Receiver<Message>,
    inbox: mpsc::Sender<ColonyMsg>,
    seen: mpsc::UnboundedSender<Uuid>,
    die: oneshot::Receiver<()>,
    stopped_reading: oneshot::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        for _ in 0..READ_BEFORE_DEATH {
            if let Some(m) = rx.recv().await {
                let _ = seen.send(m.id);
            }
        }
        let _ = stopped_reading.send(());
        let _ = die.await;
        rx.close();
        let mut rescued = Vec::new();
        while let Ok(m) = rx.try_recv() {
            rescued.push(m);
        }
        let _ = inbox
            .send(ColonyMsg::MailboxRescued {
                path,
                messages: rescued,
            })
            .await;
        panic!("the cell dies in the middle of its overflow (on purpose)");
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_dies_mid_drain_gets_its_rescued_mail_first_and_then_the_rest() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let join = tokio::spawn(colony_task(ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx,
        outputs_rx,
        db,
        CellFactoryRegistry::new(),
        td.path().to_path_buf(),
        ColonyConfig {
            shutdown_drain_timeout_ms: 0,
            ..ColonyConfig::default()
        },
        None,
        None,
    )));

    let path = Path::new("/c");
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<Uuid>();
    let (tx, rx) = mpsc::channel::<Message>(CAPACITY);
    let (die_tx, die_rx) = oneshot::channel::<()>();
    let (stopped_tx, stopped_rx) = oneshot::channel::<()>();
    let cell_join = first_life(
        path.clone(),
        rx,
        inbox_tx.clone(),
        seen_tx.clone(),
        die_rx,
        stopped_tx,
    );

    // The second life reads everything it gets.
    let respawn_seen = seen_tx.clone();
    let respawn: RespawnFn = Box::new(move || {
        let (tx, mut rx) = mpsc::channel::<Message>(CAPACITY);
        let seen = respawn_seen.clone();
        let (peace_tx, peace_rx) = oneshot::channel::<()>();
        let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            // Held for the life of the task, like a real cell task holds them.
            let _keep = (peace_tx, backstop_tx);
            while let Some(m) = rx.recv().await {
                let _ = seen.send(m.id);
            }
        });
        (tx, join, peace_rx, backstop_rx)
    });
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Register {
            path: path.clone(),
            sender: tx,
            join: cell_join,
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
    // No peace for the first life: with its sender gone the watcher awaits the
    // task's join and classifies the panic.
    drop(peace_tx);

    let msgs: Vec<Message> = (0..TOTAL)
        .map(|_| MessageBuilder::new(path.clone()).build())
        .collect();
    let sent: Vec<Uuid> = msgs.iter().map(|m| m.id).collect();
    for m in msgs {
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: m,
            })
            .await
            .expect("colony inbox closed");
    }
    tokio::time::timeout(MARKER, stopped_rx)
        .await
        .expect("the first life read its ten")
        .expect("stopped signal");
    // Let the mailbox refill and the rest queue up in the overflow.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = die_tx.send(());

    let mut got = Vec::with_capacity(TOTAL);
    for i in 0..TOTAL {
        let id = tokio::time::timeout(MARKER, seen_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("message {i} of {TOTAL} never arrived — lost in the death"))
            .expect("seen channel");
        got.push(id);
    }
    assert!(
        got == sent,
        "rescued mail first, then the rest of the overflow, in the order sent \
         (first mismatch at {:?})",
        got.iter().zip(&sent).position(|(a, b)| a != b)
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, join).await;
}
