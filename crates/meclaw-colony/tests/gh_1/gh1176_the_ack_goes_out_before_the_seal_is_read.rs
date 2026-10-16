//! GH #1176 — a committed mutation is acknowledged BEFORE the colony reads
//! the post-state seal and delivery boundaries for its slot rebuild.
//!
//! The outcome needs only the post-state required drains (a missing drain
//! rolls the diff back). The seal and the connect points of a sealed hive feed
//! the slot table and the delivery filter, which the colony loop rebuilds
//! after the door; reading them before the ack made every caller wait for a
//! scan its answer does not depend on (measured: +25.6 % median time to ack
//! per mutation, idle, strand report tb-s9b run2).
//!
//! The probe is real, not a seam: after boot the sealed hive `/mem`'s
//! `config.json` becomes a FIFO that a writer thread serves one read at a
//! time. The order is decided by events, never by the clock (AGENTS.md rule
//! 11): the writer holds the colony's ack itself and stamps every read, the
//! moment its reader has opened the FIFO, with whether the ack was out by
//! then. It serves the reads the ack depends on -- the validation's seal walk
//! and contract walk, and the post-state drains -- and holds every further
//! read that comes before the ack. A read beyond those before the ack is the
//! verdict once it is served -- at once when the ack does not wait for it,
//! after the colony's own read timeout when it does (the diff is then refused
//! with `hive_config_timeout`, or the ack waited for the slot rebuild).
//! After the ack the slot rebuild must read `/mem` twice -- once for the
//! seal, once for the connect points of the sealed hive.
//!
//! Each read gets its own FIFO: before the writer closes its end, a fresh FIFO
//! is renamed over the path, so the next open can never meet the reader that
//! is still draining the previous copy (two copies in one read do not parse,
//! and the diff was refused with `hive_config_unreadable` under load). The
//! second test pins that mechanism with a reader the test holds itself.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::json;
use meclaw_testing::deadline;
use meclaw_testing::factories::EchoCellFactory;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, oneshot};

/// The reads of `/mem` the ack depends on: the validation's seal walk and
/// contract walk, and the post-state drains. A fall-back that read the seal
/// and the connect points before the ack again would need two more (5 reads
/// on the commit before the split, strand report tb-s9b).
const ACK_READS: usize = 3;

/// `O_NONBLOCK` (Linux): the closing reader opens the FIFO without waiting
/// for a writer, so it releases a writer blocked in its open.
const O_NONBLOCK: i32 = 0o4000;

/// `ENXIO` (Linux): a non-blocking open for writing of a FIFO no reader has
/// open.
const ENXIO: i32 = 6;

const MEM_CONFIG: &str =
    r#"{"cell":{"type":"hive"},"params":{"ports":["glue"],"graph":{"edges":[]}}}"#;

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
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn mkfifo(path: &Path) {
    if let Err(e) = make_fifo(path) {
        panic!("{e}");
    }
}

/// `mkfifo` as a result, for the writer thread: it outlives a test whose
/// verdict already fell, and must not add a panic of its own then.
fn make_fifo(path: &Path) -> Result<(), String> {
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .map_err(|e| format!("mkfifo runs: {e}"))?;
    if made.success() {
        Ok(())
    } else {
        Err(format!("mkfifo {}", path.display()))
    }
}

/// Writes one copy into the open end `w`. A fresh FIFO takes the path before
/// this end closes: the reader still draining this copy keeps the old one to
/// itself, and the next open meets only a new reader. A reader that gave up
/// (the colony's own read timeout) is gone; the write then fails, and the
/// read was counted at its open.
fn write_copy(mut w: std::fs::File, cfg: &Path, spare: &Path, n: usize) -> Result<(), String> {
    let _ = w.write_all(MEM_CONFIG.as_bytes());
    let next = spare.join(format!("mem-config-{n}"));
    make_fifo(&next)?;
    std::fs::rename(&next, cfg).map_err(|e| format!("the fresh FIFO takes the path: {e}"))?;
    drop(w);
    Ok(())
}

/// Serves `cfg` one read per FIFO and reports each served read on `served`,
/// stamped the moment its reader opened the FIFO: `true` when the colony had
/// sent its ack by then. The report follows the copy and its rename, so a
/// test that stops on it finds the writer on its way to the newest FIFO.
/// The writer holds the ack receiver itself, so the stamp is the colony's own
/// order -- an ack sent before a read began is seen at that read, with no hop
/// through the test in between (re-check RC3 M2: a stamp taken before the
/// blocking open counted a read after the ack as one before it, so
/// `before_ack <= ACK_READS` held by construction). The first
/// [`ACK_READS`] reads are served at once; a further read that comes before
/// the ack is held until the ack is out. The ack goes on to the test on
/// `outcome` before the first read stamped after it.
fn serve_reads(
    cfg: PathBuf,
    spare: PathBuf,
    served: mpsc::UnboundedSender<bool>,
    ack: oneshot::Receiver<MutationOutcome>,
    outcome: mpsc::UnboundedSender<MutationOutcome>,
    stop: Arc<AtomicBool>,
) {
    let mut count = 0usize;
    // `None` once the ack is out and handed on.
    let mut pending = Some(ack);
    loop {
        // Blocks until a reader has opened the FIFO.
        let Ok(w) = std::fs::OpenOptions::new().write(true).open(&cfg) else {
            return;
        };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if let Some(rx) = pending.as_mut()
            && let Ok(o) = rx.try_recv()
        {
            pending = None;
            let _ = outcome.send(o);
        }
        count += 1;
        let after = pending.is_none();
        if count > ACK_READS
            && let Some(rx) = pending.take()
        {
            // Held: this read is not one the ack may wait for.
            let Ok(o) = rx.blocking_recv() else {
                return;
            };
            let _ = outcome.send(o);
        }
        if let Err(e) = write_copy(w, &cfg, &spare, count) {
            // The test is gone: its verdict fell and took the TempDir with it
            // (re-check RC-T4 M2). One red, not a second panic on top of it.
            if served.is_closed() {
                return;
            }
            panic!("the writer serves read {count}: {e}");
        }
        if served.send(after).is_err() {
            return;
        }
    }
}

/// Re-check RC3 M5: the reader of the previous copy is held open by the test
/// and has not read a byte, so it would take a second copy if the next open
/// could reach it -- no race decides it. With the fresh FIFO renamed over the
/// path, an open for the next read finds no reader at all.
#[test]
fn the_next_open_never_meets_the_reader_of_the_previous_copy() {
    let td = tempfile::TempDir::new().unwrap();
    let cfg = td.path().join("config.json");
    mkfifo(&cfg);
    let spare = td.path().join("fifos");
    std::fs::create_dir_all(&spare).unwrap();

    // The reader opens without waiting for a writer, and reads nothing yet.
    let mut reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(&cfg)
        .expect("the reader opens the FIFO");
    let w = std::fs::OpenOptions::new()
        .write(true)
        .open(&cfg)
        .expect("a reader is there, the writer opens at once");
    write_copy(w, &cfg, &spare, 1).expect("the writer serves the first copy");

    let next = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(&cfg);
    assert_eq!(
        next.as_ref().err().and_then(std::io::Error::raw_os_error),
        Some(ENXIO),
        "the open for the next read must not reach the reader of the previous copy: {next:?}"
    );
    let mut got = String::new();
    std::io::Read::read_to_string(&mut reader, &mut got).expect("the copy and its end");
    assert_eq!(got, MEM_CONFIG, "the reader gets exactly one copy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ack_goes_out_before_the_post_state_seal_is_read() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    echo_cell(td.path(), "main/caller", "/caller");
    echo_cell(td.path(), "main/sink", "/sink");
    std::fs::create_dir_all(td.path().join("main/mem")).unwrap();
    std::fs::write(td.path().join("main/mem/config.json"), MEM_CONFIG).unwrap();
    echo_cell(td.path(), "main/mem/glue", "/mem/glue");

    let h = meclaw_testing::ColonyHandle::new_with_factories_at(&td, echo_factories());
    deadline::boot_hang(
        "the colony boots",
        bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime()),
    )
    .await
    .expect("boot");

    // From here `/mem`'s config is a FIFO. Each open for writing blocks until
    // a reader has opened it, so a served read is one the colony asked for.
    let cfg = td.path().join("main/mem/config.json");
    std::fs::remove_file(&cfg).unwrap();
    mkfifo(&cfg);
    let spare = td.path().join("fifos");
    std::fs::create_dir_all(&spare).unwrap();

    let (served_tx, mut served_rx) = mpsc::unbounded_channel();
    let (ack_tx, ack_rx) = oneshot::channel();
    let (outcome_tx, mut outcome_rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (cfg, stop) = (cfg.clone(), stop.clone());
        std::thread::spawn(move || serve_reads(cfg, spare, served_tx, ack_rx, outcome_tx, stop))
    };

    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"diff": {"add_edges": [{"from": "./caller", "to": "./sink"}]}}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();

    // The slot rebuild runs on the loop's next pass, after the ack: the seal
    // and the connect points of `/mem`, read after the ack, not before it. A
    // read before the ack beyond validation and drains is the verdict as soon
    // as it is served, whether the ack waits for it or not.
    let (mut before_ack, mut after_ack) = (0usize, 0usize);
    while after_ack < 2 {
        let after = deadline::hang_with(
            "the colony answers and its slot rebuild reads `/mem` twice after the ack",
            served_rx.recv(),
            || (before_ack, after_ack),
        )
        .await
        .expect("the writer serves reads");
        if after {
            after_ack += 1;
        } else {
            before_ack += 1;
            assert!(
                before_ack <= ACK_READS,
                "only validation and drains read `/mem` before the ack: read {before_ack} \
                 came before it"
            );
        }
    }
    let outcome = outcome_rx
        .try_recv()
        .expect("a read stamped after the ack: the writer handed the ack on before it");
    // A held read the ack waited for ends in the colony's own read timeout:
    // `hive_config_timeout` here means the seal was read before the ack.
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the diff commits without waiting for a read beyond validation and drains: {outcome:?}"
    );
    // Release the writer: it is blocked in open until a reader comes.
    stop.store(true, Ordering::SeqCst);
    let released = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(&cfg)
        .expect("the closing reader opens the FIFO");
    deadline::hang(
        "the writer thread ends",
        tokio::task::spawn_blocking(move || writer.join()),
    )
    .await
    .unwrap()
    .unwrap();
    drop(released);
    std::fs::remove_file(&cfg).unwrap();
    std::fs::write(&cfg, MEM_CONFIG).unwrap();
    h.shutdown().await;
}
