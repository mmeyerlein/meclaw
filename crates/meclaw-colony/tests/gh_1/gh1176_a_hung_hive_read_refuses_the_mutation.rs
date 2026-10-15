//! GH #1176 — a hive `config.json` whose read does not come back refuses the
//! mutation that needed it (`hive_config_timeout`), and the colony goes on.
//!
//! The hang is real, not a seam: after boot, `/mem`'s `config.json` is
//! replaced by a FIFO with no data in it, so the validation scan's read blocks
//! in the kernel. The read timeout comes from the colony's own watchdog
//! window (`colony.json`: 2 × 20 ms → 400 ms work-item budget → 800 ms read
//! timeout), the same wiring as production. Then the test releases the read,
//! puts the real file back, and the same diff commits.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure marker (AGENTS.md: 30 s convention); a worker that hangs on the
/// FIFO itself shows up here, not as a hung suite.
const MARKER: Duration = Duration::from_secs(30);

const MEM_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

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

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("failure marker: the colony answers the mutation")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_hive_read_refuses_the_mutation_and_the_colony_goes_on() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"watchdog_threshold": 2, "watchdog_period_ms": 20}"#,
    )
    .unwrap();
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

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot");

    // From here `/mem`'s config is a FIFO. The test holds it open for reading
    // AND writing, so a reader's open() returns and its read() blocks: a disk
    // that does not answer.
    let cfg = td.path().join("main/mem/config.json");
    let real = td.path().join("main/mem/config.json.real");
    std::fs::rename(&cfg, &real).unwrap();
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

    let diff = json!({"diff": {"add_edges": [{"from": "./caller", "to": "./sink"}]}});
    let started = std::time::Instant::now();
    let refused = send_mutation(&h, diff.clone()).await;
    match &refused {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_config_timeout", "{refused:?}");
            assert!(
                details.contains("mem"),
                "the refusal names the hive whose read hung: {details}"
            );
        }
        other => panic!("a hung hive read refuses the mutation (fail-closed), got {other:?}"),
    }
    // Semantic discriminator: the refusal is the read timeout (800 ms from the
    // watchdog window above), not an early error of some other stage.
    assert!(
        started.elapsed() >= Duration::from_millis(800),
        "refused only after the read timeout, took {:?}",
        started.elapsed()
    );

    // The disk answers again: the real file goes back for every later open,
    // and the hung read gets its bytes and its end of file.
    std::fs::rename(&real, &cfg).unwrap();
    fifo.write_all(MEM_CONFIG.as_bytes()).unwrap();
    drop(fifo);

    match send_mutation(&h, diff).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("once the read comes back the same diff commits, got {other:?}"),
    }
    h.shutdown().await;
}
