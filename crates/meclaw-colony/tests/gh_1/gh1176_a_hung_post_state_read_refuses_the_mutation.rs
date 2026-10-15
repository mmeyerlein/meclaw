//! GH #1176 — a hive `config.json` read that does not come back in the
//! POST-STATE scan (the required drains, read after the apply) refuses the
//! mutation with `hive_config_timeout` and rolls it back.
//!
//! The validation scan has its own end-to-end test; this one pins the second
//! call site, where the drains decide the outcome. An empty drain list in
//! place of the timed-out read would commit a diff whose drains nobody read.
//!
//! The hang is real: after boot, `/mem`'s `config.json` becomes a FIFO that a
//! writer thread serves exactly as often as the validation scan reads it (the
//! seal walk and the contract walk), then stops serving -- the post-state
//! scan's open blocks in the kernel. The read timeout comes from the colony's
//! own watchdog window (2 × 20 ms → 800 ms).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::json;
use meclaw_testing::factories::EchoCellFactory;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure marker (AGENTS.md: 30 s convention).
const MARKER: Duration = Duration::from_secs(30);

/// Pause between two served reads, so the next open of the writer does not
/// meet the previous reader (see `gh1176_the_ack_goes_out_before_the_seal_is_read`).
const GAP: Duration = Duration::from_millis(50);

/// Reads of `/mem`'s config the validation scan makes before the apply: the
/// sealed-ports walk and the hive-contract walk.
const VALIDATION_READS: usize = 2;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_post_state_read_refuses_the_mutation() {
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
    echo_cell(td.path(), "main/caller", "/caller");
    echo_cell(td.path(), "main/sink", "/sink");
    std::fs::create_dir_all(td.path().join("main/mem")).unwrap();
    std::fs::write(td.path().join("main/mem/config.json"), MEM_CONFIG).unwrap();
    echo_cell(td.path(), "main/mem/glue", "/mem/glue");

    let h = meclaw_testing::ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot");
    // One event through the loop: the boot's rebuild has read every hive.
    let _ = h.drain_dead_letters().await;

    let cfg = td.path().join("main/mem/config.json");
    std::fs::remove_file(&cfg).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&cfg)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}", cfg.display());
    // Serve the validation scan, then nothing: the post-state open blocks.
    let writer = {
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            for _ in 0..VALIDATION_READS {
                let mut w = std::fs::OpenOptions::new().write(true).open(&cfg).unwrap();
                w.write_all(MEM_CONFIG.as_bytes()).unwrap();
                drop(w);
                std::thread::sleep(GAP);
            }
        })
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
    let outcome = tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("failure marker: the colony answers the mutation")
        .unwrap();
    writer.join().unwrap();
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_config_timeout", "{outcome:?}");
            assert!(
                details.contains("post-state") && details.contains("mem"),
                "the refusal is the post-state read of `/mem`: {details}"
            );
        }
        other => panic!(
            "a post-state read that does not come back refuses the mutation (fail-closed), \
             got {other:?}"
        ),
    }

    // The disk answers again: the hung open gets a writer (this open returns
    // because that reader is waiting), the real file goes back for every later
    // open, and the hung read gets its bytes and its end of file.
    let real = td.path().join("main/mem/config.json.real");
    std::fs::write(&real, MEM_CONFIG).unwrap();
    let mut w = std::fs::OpenOptions::new().write(true).open(&cfg).unwrap();
    std::fs::rename(&real, &cfg).unwrap();
    w.write_all(MEM_CONFIG.as_bytes()).unwrap();
    drop(w);
    h.shutdown().await;
}
