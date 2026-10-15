//! GH #1176 — a slot rebuild whose read times out fails CLOSED at the hive
//! boundary.
//!
//! The delivery filter is rebuilt from the hives' `config.json` after every
//! topology change. When that read does not come back (a disk that does not
//! answer), the colony cannot know which hive was sealed since the last good
//! read. Keeping the old table would deliver an outside message into a hive
//! that sealed itself in the meantime -- a boundary that exists on disk and is
//! not enforced. So until a rebuild succeeds, every hive is treated as sealed
//! with no declared address: an outside message naming an interior path is
//! refused with `hive_boundary`. This holds under `watchdog_on_trip:
//! log-only`, where the process stays up through the hang.
//!
//! The hang is real: the new hive `/vault`'s `config.json` is a FIFO the test
//! holds open, so the rebuild's read blocks in the kernel. The read timeout
//! comes from the colony's own watchdog window (2 × 20 ms → 800 ms). Then the
//! disk answers, a rebuild succeeds, and the same message -- naming a port
//! `/vault` declared -- is delivered.
//!
//! The state is visible to an operator: closing logs `ingress closed: slot
//! rebuild timed out` (ERROR), reopening `ingress reopened: slot rebuild
//! succeeded` (WARN), each once per transition. The test records both from
//! a process-wide subscriber (nextest runs each test in its own process).

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, deadline};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const CLOSED: &str = "ingress closed: slot rebuild timed out";
const REOPENED: &str = "ingress reopened: slot rebuild succeeded";

/// How many lines at `level` say `line` about this test's colony (the
/// recorder is process-wide and the tests of the binary run concurrently).
fn count(root: &std::path::Path, level: tracing::Level, line: &str) -> usize {
    let root = format!("root={}", root.display());
    let message = format!("message={line}");
    meclaw_testing::log_recorder::global()
        .lines(level, &[&root, &message])
        .len()
}

/// How long a refused message is given to arrive anyway: well past the
/// 800 ms read timeout the rebuild ahead of it waits out.
const NOT_DELIVERED: Duration = Duration::from_millis(2_000);

/// `/vault` is sealed and declares its one cell `secret` as a port: once its
/// seal is read, `/vault/secret` is an address from outside.
const VAULT_CONFIG: &str =
    r#"{"cell":{"type":"hive"},"params":{"ports":["secret"],"graph":{"edges":[]}}}"#;

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

fn from_outside(target: &str) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(16)
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timed_out_rebuild_refuses_outside_ingress_into_a_fresh_seal_until_a_rebuild_succeeds() {
    let _ = meclaw_testing::log_recorder::global();
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"watchdog_threshold": 2, "watchdog_period_ms": 20, "watchdog_on_trip": "log-only"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let (tx, mut secret_rx) = mpsc::channel(8);
    h.spawn(Path::new("/vault/secret"), move || {
        CaptureCell::new(tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot");
    // One event through the loop: the boot's rebuild has read every hive.
    let _ = h.drain_dead_letters().await;

    // The new hive `/vault`: its config is a FIFO with nothing in it. The test
    // holds it open for reading AND writing, so a reader's open() returns and
    // its read() blocks.
    let dir = td.path().join("main/vault");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.json");
    let real = dir.join("config.json.real");
    std::fs::write(&real, VAULT_CONFIG).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&cfg)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}", cfg.display());
    let mut fifo = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&cfg)
        .unwrap();

    // The scope arrives; the loop's next pass rebuilds and hangs on `/vault`.
    h.add_hive_scope(Path::new("/vault")).await;
    h.send(from_outside("/vault/secret")).await;
    assert!(
        tokio::time::timeout(NOT_DELIVERED, secret_rx.recv())
            .await
            .is_err(),
        "a rebuild that timed out must not leave a fresh seal unenforced: the \
         outside message reached `/vault/secret`"
    );
    let dlq = deadline::hang("the colony answers the drain", h.drain_dead_letters()).await;
    let refused = dlq
        .iter()
        .find(|d| d.reason.as_code() == "hive_boundary")
        .unwrap_or_else(|| {
            let codes: Vec<&str> = dlq.iter().map(|d| d.reason.as_code()).collect();
            panic!("the outside message is refused at the boundary, got {codes:?}")
        });
    assert_eq!(refused.resolved_target, Path::new("/vault/secret"));
    assert_eq!(
        refused.detail(),
        Some("/vault"),
        "the refusing boundary is the unread hive itself"
    );
    assert_eq!(
        count(td.path(), tracing::Level::ERROR, CLOSED),
        1,
        "closing the boundary logs `{CLOSED}` once"
    );
    assert_eq!(
        count(td.path(), tracing::Level::WARN, REOPENED),
        0,
        "nothing reopened yet"
    );

    // Still hung: a second outside message, after a pass that went past the
    // first one, is refused too, and the boundary has not logged again.
    h.send(from_outside("/vault/secret")).await;
    assert!(
        tokio::time::timeout(NOT_DELIVERED, secret_rx.recv())
            .await
            .is_err(),
        "the boundary stays closed while the read hangs: the second outside \
         message reached `/vault/secret`"
    );
    let dlq = deadline::hang("the colony answers the drain", h.drain_dead_letters()).await;
    assert!(
        dlq.iter().any(|d| d.reason.as_code() == "hive_boundary"
            && d.resolved_target == Path::new("/vault/secret")),
        "the second outside message is refused at the boundary as well"
    );
    assert_eq!(
        count(td.path(), tracing::Level::ERROR, CLOSED),
        1,
        "a lasting hang logs the closing once, not once per pass"
    );

    // The disk answers again: the real file goes back for every later open,
    // and the hung read gets its bytes and its end of file.
    std::fs::rename(&real, &cfg).unwrap();
    fifo.write_all(VAULT_CONFIG.as_bytes()).unwrap();
    drop(fifo);
    // Events through the loop until a pass has rebuilt with the disk
    // answering. The rebuild runs at the top of a pass and skips while the
    // hung scan's blocking task has not finished (`ScanGate::still_stuck`);
    // that task wakes on the bytes above, on a blocking thread. One drain was
    // a race against that thread: under load the pass before the send still
    // saw it running, the send met the closed boundary and was refused
    // (re-check tb-s9b M2 round, build04: 2 of 6 runs red with `Elapsed` at
    // the delivery below). Waiting for the reopen is the condition the
    // delivery needs; the assertions below are unchanged. No hook announces
    // the reopen, so the wait polls it under the hang mark (TV-S1 rank 3,
    // TV-S3); each poll drives one more pass through the loop.
    let (h_ref, root) = (&h, td.path());
    meclaw_testing::wait::until("a rebuild succeeds once the disk answers", || async move {
        let _ = h_ref.drain_dead_letters().await;
        count(root, tracing::Level::WARN, REOPENED) > 0
    })
    .await;

    h.send(from_outside("/vault/secret")).await;
    let got = deadline::hang(
        "after a good rebuild the declared port is an address again",
        secret_rx.recv(),
    )
    .await
    .expect("channel closed");
    assert_eq!(got.target, Path::new("/vault/secret"));
    assert_eq!(
        count(td.path(), tracing::Level::WARN, REOPENED),
        1,
        "the good rebuild logs `{REOPENED}` once"
    );
    assert_eq!(
        count(td.path(), tracing::Level::ERROR, CLOSED),
        1,
        "the boundary closed once"
    );
    h.shutdown().await;
}
