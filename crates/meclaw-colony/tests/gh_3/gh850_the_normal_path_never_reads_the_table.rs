//! GH #850 (R-SN-5) — the normal path never touches the overflow.
//!
//! The ruling: "the table is NEVER read when delivering". A message for a cell
//! whose overflow is empty and whose mailbox has room goes through `route()`
//! as before; only a cell whose mailbox is full gets an overflow, and only its
//! drain task ever reads `mailbox_overflow`. The colony's overflow probe
//! (test-only, `ColonyTaskConfig::with_overflow_probe`) reports every overflow
//! that comes into being and every read, write and delete of the table — so
//! "never" is a count of zero here, with a positive control in the same colony
//! that proves the probe is live.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, OverflowProbe,
    RespawnFn, colony_task,
};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const MARKER: Duration = Duration::from_secs(30);

async fn register_stub(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    path: &str,
    capacity: usize,
) -> (
    mpsc::Receiver<Message>,
    oneshot::Sender<()>,
    oneshot::Sender<()>,
) {
    let (tx, rx) = mpsc::channel::<Message>(capacity);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("the stub is never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Register {
            path: Path::new(path),
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
    (rx, peace_tx, backstop_tx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_with_room_never_meets_the_overflow() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (probe_tx, mut probe_rx) = mpsc::unbounded_channel::<OverflowProbe>();
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(256);
    let (outputs_tx, outputs_rx) = mpsc::channel(256);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    let join = tokio::spawn(colony_task(
        ColonyTaskConfig::new(
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
        )
        .with_overflow_probe(probe_tx),
    ));

    // A cell that reads everything, with the default mailbox.
    let (mut fast_rx, _p1, _b1) = register_stub(&inbox_tx, "/fast", 1000).await;
    let reader = tokio::spawn(async move {
        let mut n = 0usize;
        while n < 1000 {
            match tokio::time::timeout(MARKER, fast_rx.recv()).await {
                Ok(Some(_)) => n += 1,
                _ => break,
            }
        }
        n
    });
    for _ in 0..1000 {
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/fast")).build(),
            })
            .await
            .expect("colony inbox closed");
    }
    assert_eq!(reader.await.expect("reader"), 1000, "every message arrives");

    // Positive control: a cell whose mailbox of one never drains.
    let (_jam_rx, _p2, _b2) = register_stub(&inbox_tx, "/jam", 1).await;
    for _ in 0..3 {
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: MessageBuilder::new(Path::new("/jam")).build(),
            })
            .await
            .expect("colony inbox closed");
    }
    let first = tokio::time::timeout(MARKER, probe_rx.recv())
        .await
        .expect("the probe is live: the jammed cell enters its overflow")
        .expect("probe channel");
    assert_eq!(
        first,
        OverflowProbe::Entered(Path::new("/jam")),
        "the FIRST thing the overflow ever did is the jammed cell — nothing for the \
         cell with room came before it"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut rest = Vec::new();
    while let Ok(e) = probe_rx.try_recv() {
        rest.push(e);
    }
    assert!(
        !rest.iter().any(|e| matches!(
            e,
            OverflowProbe::Entered(p)
                | OverflowProbe::TableRead(p, _)
                | OverflowProbe::TableWrite(p, _)
                | OverflowProbe::TableDelete(p, _)
                if p.as_str() == "/fast"
        )),
        "the cell with room never met the overflow: {rest:?}"
    );
    assert!(
        !rest.iter().any(|e| matches!(
            e,
            OverflowProbe::TableRead(..) | OverflowProbe::TableWrite(..)
        )),
        "a jam below the spill threshold never touches the table either: {rest:?}"
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, join).await;
}
