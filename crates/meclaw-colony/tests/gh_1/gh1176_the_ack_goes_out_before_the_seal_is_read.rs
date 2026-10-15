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
//! 11): the writer serves the reads the ack depends on -- the validation's
//! seal walk and contract walk, and the post-state drains -- and holds every
//! further read until the test has the ack in hand. A seal read the ack waited
//! for would stay held, the colony's own read timeout would refuse the diff
//! (`hive_config_timeout`), and the commit assert names it. After the ack the
//! slot rebuild must read `/mem` twice -- once for the seal, once for the
//! connect points of the sealed hive.
//!
//! Each read gets its own FIFO: before the writer closes its end, a fresh FIFO
//! is renamed over the path, so the next open can never meet the reader that
//! is still draining the previous copy (two copies in one read do not parse,
//! and the diff was refused with `hive_config_unreadable` under load).

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
use std::sync::mpsc as std_mpsc;
use tokio::sync::{mpsc, oneshot};

/// The reads of `/mem` the ack depends on: the validation's seal walk and
/// contract walk, and the post-state drains. A fall-back that read the seal
/// and the connect points before the ack again would need two more (5 reads
/// on the commit before the split, strand report tb-s9b).
const ACK_READS: usize = 3;

/// `O_NONBLOCK` (Linux): the closing reader opens the FIFO without waiting
/// for a writer, so it releases a writer blocked in its open.
const O_NONBLOCK: i32 = 0o4000;

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
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}", path.display());
}

/// Serves `cfg` one read per FIFO. Every served read is reported on `served`
/// as `true` when the test held the ack before the read was served. The
/// first [`ACK_READS`] reads are served at once; every further read waits
/// for `ack_seen`.
fn serve_reads(
    cfg: PathBuf,
    spare: PathBuf,
    served: mpsc::UnboundedSender<bool>,
    ack_seen: std_mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
) {
    let mut count = 0usize;
    let mut acked = false;
    loop {
        if !acked {
            if count >= ACK_READS {
                // Held: this read is not one the ack may wait for.
                if ack_seen.recv().is_err() {
                    return;
                }
                acked = true;
            } else if ack_seen.try_recv().is_ok() {
                acked = true;
            }
        }
        // Blocks until a reader has opened the FIFO.
        let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(&cfg) else {
            return;
        };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let copied = w.write_all(MEM_CONFIG.as_bytes()).is_ok();
        // A fresh FIFO takes the path before this end closes: the reader
        // still draining this copy keeps the old one to itself.
        let next = spare.join(format!("mem-config-{count}"));
        mkfifo(&next);
        std::fs::rename(&next, &cfg).expect("the fresh FIFO takes the path");
        drop(w);
        if copied {
            count += 1;
            if served.send(acked).is_err() {
                return;
            }
        }
    }
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
    let (ack_seen_tx, ack_seen_rx) = std_mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (cfg, stop) = (cfg.clone(), stop.clone());
        std::thread::spawn(move || serve_reads(cfg, spare, served_tx, ack_seen_rx, stop))
    };

    let (ack_tx, ack_rx) = oneshot::channel();
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
    let outcome = deadline::hang("the colony answers the mutation", ack_rx)
        .await
        .unwrap();
    ack_seen_tx.send(()).expect("the writer thread is alive");
    // A held read the ack waited for ends in the colony's own read timeout:
    // `hive_config_timeout` here means the seal was read before the ack.
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the diff commits without waiting for a read beyond validation and drains: {outcome:?}"
    );

    // The slot rebuild runs on the loop's next pass, after the ack: the seal
    // and the connect points of `/mem`, read after the ack, not before it.
    let (mut before_ack, mut after_ack) = (0usize, 0usize);
    while after_ack < 2 {
        let after = deadline::hang_with(
            "the slot rebuild reads `/mem` twice after the ack",
            served_rx.recv(),
            || (before_ack, after_ack),
        )
        .await
        .expect("the writer serves reads");
        if after {
            after_ack += 1;
        } else {
            before_ack += 1;
        }
    }
    assert!(
        before_ack <= ACK_READS,
        "only validation and drains read `/mem` before the ack: {before_ack} reads \
         before the ack, {after_ack} after"
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
