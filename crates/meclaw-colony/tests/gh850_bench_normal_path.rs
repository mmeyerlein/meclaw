//! GH #850 (OR-SN-41) — the normal path costs nothing extra.
//!
//! Not a test of behaviour but a measurement, hence `#[ignore]`: 200 000
//! messages through a booted colony to a cell that reads as fast as it can,
//! printed as messages per second. Run it on the branch and on the release
//! before the overflow (`v0.45.0`) with
//! `scripts/test-tier.sh filter 'binary(~gh850_bench)' --run-ignored all`;
//! the acceptance is branch >= 97 % of the base, median of three runs each.
//!
//! It uses only interfaces that exist on both sides, so the same file runs on
//! the base unchanged.

use meclaw_colony::{
    CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyTaskConfig, RespawnFn,
    colony_task,
};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const MESSAGES: usize = 200_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark — run explicitly, see the module docs"]
async fn the_normal_path_throughput() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(1024);
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

    let (tx, mut rx) = mpsc::channel::<Message>(1000);
    let (_peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let cell = tokio::spawn(async { std::future::pending::<()>().await });
    let respawn: RespawnFn = Box::new(|| unreachable!("never respawned"));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Register {
            path: Path::new("/sink"),
            sender: tx,
            join: cell,
            peace_rx,
            backstop_rx,
            stop_tx: None,
            death_ack_rx: None,
            respawn,
            wake: None,
            restart_limit: None,
            cell_id: Uuid::now_v7(),
            cell_type: "bench-sink".into(),
            active: true,
            ack: ack_tx,
        })
        .await
        .expect("inbox");
    ack_rx.await.expect("register ack");

    let msgs: Vec<Message> = (0..MESSAGES)
        .map(|_| MessageBuilder::new(Path::new("/sink")).build())
        .collect();
    let reader = tokio::spawn(async move {
        let mut n = 0usize;
        while n < MESSAGES {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
                Ok(Some(_)) => n += 1,
                _ => break,
            }
        }
        n
    });
    let started = Instant::now();
    for m in msgs {
        inbox_tx
            .send(ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: m,
            })
            .await
            .expect("inbox");
    }
    let n = reader.await.expect("reader");
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(n, MESSAGES, "every message arrives");
    println!(
        "gh850-bench: {MESSAGES} messages in {secs:.3} s = {:.0} msg/s",
        MESSAGES as f64 / secs
    );
    eprintln!(
        "gh850-bench: {MESSAGES} messages in {secs:.3} s = {:.0} msg/s",
        MESSAGES as f64 / secs
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}
