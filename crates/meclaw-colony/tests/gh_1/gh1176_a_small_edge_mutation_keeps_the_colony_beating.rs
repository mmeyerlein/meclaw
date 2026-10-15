//! GH #1176 — a small `add_edges` mutation keeps the colony task beating while
//! a hive read it triggers does not come back.
//!
//! The lab trip: ten `add_edges` in one hive scope, no `add_nodes`, and the
//! colony task was silent for 5.2 s after the in-flight insert (`witness=kept`,
//! so not host starvation). The load is modelled deterministically: after the
//! hive is instantiated, its `config.json` becomes a FIFO with an open writer
//! and no data, so every read of it blocks in the kernel -- a disk that does
//! not answer under load, for as long as the test wants.
//!
//! The assert is on the colony task's own heartbeat, sequenced by events, not
//! by a clock: the mutation must get its answer (committed or refused, either
//! is fine here) AND the loop must report `Beat::Parked` after that answer,
//! i.e. come back to its select loop. A loop that awaits the hung read without
//! a timeout of its own never parks again; the 30-s failure marker then names
//! the last beat it heard. The FIFO is fed on every exit (guard), so a red run
//! does not leave a blocking-pool thread that keeps the test process alive.

use meclaw_colony::watchdog::Beat;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, colony_task,
};
use meclaw_core::{JsonValue, Uuid, serde_json::json};
use meclaw_testing::factories::echo::EchoCellFactory;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Failure marker (30-s convention), never a timing discriminator.
const MARKER: Duration = Duration::from_secs(30);

/// The cells of the hive; the mutation adds an edge between each neighbour pair.
const LEAVES: usize = 11;

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    f
}

fn setup(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("main")).unwrap();
    std::fs::write(dir.join("main/config.json"), HIVE).unwrap();
    let tpl = dir.join("templates").join("row");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"row"}"#).unwrap();
    std::fs::write(tpl.join("config.json"), HIVE).unwrap();
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

/// The instantiated hive's directory: the one `stack` dir with a `config.json`
/// outside `templates/`, wherever the colony placed it.
fn find_stack(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if !p.is_dir() || p.file_name().is_some_and(|n| n == "templates") {
            continue;
        }
        if p.file_name().is_some_and(|n| n == "stack") && p.join("config.json").is_file() {
            return Some(p);
        }
        if let Some(found) = find_stack(&p) {
            return Some(found);
        }
    }
    None
}

async fn mutate(
    inbox_tx: &mpsc::Sender<ColonyMsg>,
    payload: JsonValue,
) -> oneshot::Receiver<meclaw_colony::MutationOutcome> {
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
}

/// On every exit, red or green: the real file goes back over the FIFO (a
/// later open finds a regular file), then the FIFO is fed, so a read blocked
/// on it ends. Measured on master 10.10.: feeding alone left the next open of
/// the writer-less FIFO blocked and the test process alive past 60 s.
struct Release(Option<std::fs::File>, std::path::PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let real = self.1.with_extension("json.real");
        if std::fs::write(&real, HIVE).is_ok() {
            let _ = std::fs::rename(&real, &self.1);
        }
        if let Some(mut w) = self.0.take() {
            let _ = w.write_all(HIVE.as_bytes());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_small_edge_mutation_keeps_the_colony_beating_while_a_hive_read_hangs() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let (inbox_tx, inbox_rx) = mpsc::channel::<ColonyMsg>(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let (hb_tx, mut hb_rx) = mpsc::channel::<Beat>(65_536);
    let db = ColonyDb::open(&td.path().join("colony.db")).expect("open colony.db");
    // The production wiring of a short watchdog window (2 x 20 ms): a hive
    // read the colony bounds by its own window ends well inside the marker.
    let config = ColonyConfig {
        watchdog_threshold: 2,
        watchdog_period_ms: 20,
        ..ColonyConfig::default()
    };
    let colony_join = tokio::spawn(colony_task(
        meclaw_colony::ColonyTaskConfig::new(
            inbox_tx.clone(),
            inbox_rx,
            outputs_tx.clone(),
            outputs_rx,
            db,
            factories(),
            td.path().to_path_buf(),
            config,
            None,
            None,
        )
        .with_heartbeat(hb_tx),
    ));
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the rescan must not abort");

    let made = tokio::time::timeout(
        MARKER,
        mutate(
            &inbox_tx,
            json!({"scope": "/", "ctx": {}, "diff": {"add_nodes": [{"name": "stack", "template": "row"}]}}),
        )
        .await,
    )
    .await
    .expect("failure marker: the instantiation answers")
    .expect("an outcome");
    assert!(
        matches!(made, meclaw_colony::MutationOutcome::Committed { .. }),
        "the instantiation must commit: {made:?}"
    );
    let stack = find_stack(td.path()).expect("the instantiated hive has a directory");
    let main = td.path().join("main");
    let rel = stack
        .strip_prefix(&main)
        .or_else(|_| stack.strip_prefix(td.path()))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let scope = format!("/{rel}");

    // The hive's config.json becomes a read that does not come back.
    let cfg = stack.join("config.json");
    std::fs::remove_file(&cfg).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&cfg)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}", cfg.display());
    let _release = Release(
        Some(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&cfg)
                .unwrap(),
        ),
        cfg.clone(),
    );
    while hb_rx.try_recv().is_ok() {}

    let edges: Vec<JsonValue> = (0..LEAVES - 1)
        .map(|i| json!({"from": format!("c{i}"), "to": format!("c{}", i + 1)}))
        .collect();
    let mut ack = mutate(
        &inbox_tx,
        json!({"scope": scope, "ctx": {}, "diff": {"add_edges": edges}}),
    )
    .await;

    // Event order: the answer, then a park of the loop after it.
    let deadline = tokio::time::Instant::now() + MARKER;
    let mut answer = None;
    let mut last = String::from("<none>");
    let mut heard = 0usize;
    loop {
        tokio::select! {
            got = &mut ack, if answer.is_none() => {
                answer = Some(got.expect("the colony keeps the ack sender"));
            }
            beat = hb_rx.recv() => {
                heard += 1;
                match beat.expect("the colony task is alive") {
                    Beat::Parked if answer.is_some() => break,
                    Beat::WorkingOn(w) => last = w.as_str().to_string(),
                    Beat::Working => last = "Working".into(),
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => panic!(
                "failure marker: the colony task did not come back from a ten-edge \
                 mutation while a hive read hangs (answer: {answer:?}; {heard} beats \
                 after the send, last work item: {last})"
            ),
        }
    }

    drop(_release);
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(MARKER, ack_rx).await;
    let _ = tokio::time::timeout(MARKER, colony_join).await;
}
